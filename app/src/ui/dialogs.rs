use super::*;
use objc2::runtime::AnyObject;
use objc2::{msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSAlertStyle,
    NSAlertThirdButtonReturn, NSApplication,
    NSApplicationLaunchIsDefaultLaunchKey, NSEventModifierFlags, NSImage,
    NSImageNameApplicationIcon, NSMenu,
    NSModalResponseOK, NSOpenPanel,
    NSSavePanel, NSWindow,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSNotification, NSString,
};
use std::env;
use std::path::PathBuf;
use std::ptr;

// Modal flows and the main menu: opening and saving files, the welcome
// prompt, and the unsaved-changes confirmation.

impl Delegate {
    pub(crate) fn present_welcome_window(&self) {
        match choose_welcome_action(self.mtm()) {
            WelcomeChoice::New => {
                let result = self.ivars().state.borrow_mut().open_blank();
                if result.is_ok() {
                    self.show_window(self.mtm());
                }
                self.handle_result(result);
            }
            WelcomeChoice::Open => {
                if let Some(path) = choose_startup_file(self.mtm()) {
                    self.open_window_for_path(path);
                }
            }
            WelcomeChoice::Cancel => {}
            WelcomeChoice::Quit => {
                NSApplication::sharedApplication(self.mtm()).terminate(None);
            }
        }
    }

    pub(crate) fn open_window_for_path(&self, path: PathBuf) -> bool {
        let mtm = self.mtm();
        let delegate = Delegate::new(mtm);
        let result = delegate.ivars().state.borrow_mut().open_path(path);
        if result.is_err() {
            delegate.handle_result(result);
            return false;
        }
        delegate.show_window(mtm);
        self.ivars().window_delegates.borrow_mut().push(delegate);
        true
    }

    pub(crate) fn save_document_with_prompt(&self) -> bool {
        if self.is_saving() {
            return false;
        }
        if let Some(window) = self.ivars().window.get() {
            unsafe { window.endEditingFor(None) };
        }
        let target = {
            let state = self.ivars().state.borrow();
            let Some(doc) = state.document.as_ref() else {
                return true;
            };
            if doc.path().is_some() {
                None
            } else {
                Some(match choose_save_target(self.mtm(), &state.title()) {
                    Some(path) => path,
                    None => return false,
                })
            }
        };

        self.set_saving(true);
        let result = self.ivars().state.borrow_mut().save(target);
        let ok = result.is_ok();
        self.set_saving(false);
        self.handle_result(result);
        self.refresh_table();
        ok
    }

    pub(crate) fn install_main_menu(&self, app: &NSApplication) {
        let mtm = self.mtm();
        let app_target = unsafe { any_ref(app) };
        let delegate_target = unsafe { any_ref(self) };
        let main_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(""));

        let app_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Editable"));
        app_menu.addItem(&menu_item(
            "About Editable",
            app_target,
            Some(sel!(orderFrontStandardAboutPanel:)),
            "",
            NSEventModifierFlags::empty(),
            mtm,
        ));
        add_separator(&app_menu, mtm);
        app_menu.addItem(&menu_item(
            "Hide Editable",
            app_target,
            Some(sel!(hide:)),
            "h",
            NSEventModifierFlags::Command,
            mtm,
        ));
        app_menu.addItem(&menu_item(
            "Hide Others",
            app_target,
            Some(sel!(hideOtherApplications:)),
            "h",
            NSEventModifierFlags::Command | NSEventModifierFlags::Option,
            mtm,
        ));
        app_menu.addItem(&menu_item(
            "Show All",
            app_target,
            Some(sel!(unhideAllApplications:)),
            "",
            NSEventModifierFlags::empty(),
            mtm,
        ));
        add_separator(&app_menu, mtm);
        app_menu.addItem(&menu_item(
            "Quit Editable",
            app_target,
            Some(sel!(terminate:)),
            "q",
            NSEventModifierFlags::Command,
            mtm,
        ));
        add_submenu(&main_menu, "Editable", &app_menu, mtm);

        let file_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("File"));
        file_menu.addItem(&menu_item(
            "Open...",
            delegate_target,
            Some(sel!(menuOpenDocument:)),
            "o",
            NSEventModifierFlags::Command,
            mtm,
        ));
        file_menu.addItem(&menu_item(
            "Save",
            delegate_target,
            Some(sel!(menuSaveDocument:)),
            "s",
            NSEventModifierFlags::Command,
            mtm,
        ));
        add_separator(&file_menu, mtm);
        file_menu.addItem(&menu_item(
            "Close Window",
            delegate_target,
            Some(sel!(menuCloseWindow:)),
            "w",
            NSEventModifierFlags::Command,
            mtm,
        ));
        add_submenu(&main_menu, "File", &file_menu, mtm);

        let edit_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Edit"));
        edit_menu.addItem(&menu_item(
            "Undo",
            delegate_target,
            Some(sel!(menuUndoChange:)),
            "z",
            NSEventModifierFlags::Command,
            mtm,
        ));
        edit_menu.addItem(&menu_item(
            "Redo",
            delegate_target,
            Some(sel!(menuRedoChange:)),
            "z",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
            mtm,
        ));
        add_separator(&edit_menu, mtm);
        edit_menu.addItem(&menu_item(
            "Edit Cell",
            delegate_target,
            Some(sel!(menuEditSelectedCell:)),
            "e",
            NSEventModifierFlags::Command,
            mtm,
        ));
        edit_menu.addItem(&menu_item(
            "Copy",
            delegate_target,
            Some(sel!(menuCopySelection:)),
            "c",
            NSEventModifierFlags::Command,
            mtm,
        ));
        edit_menu.addItem(&menu_item(
            "Find",
            delegate_target,
            Some(sel!(menuFind:)),
            "f",
            NSEventModifierFlags::Command,
            mtm,
        ));
        edit_menu.addItem(&menu_item(
            "Delete Selection",
            delegate_target,
            Some(sel!(menuDeleteSelection:)),
            "",
            NSEventModifierFlags::empty(),
            mtm,
        ));
        add_submenu(&main_menu, "Edit", &edit_menu, mtm);

        let table_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Table"));
        table_menu.addItem(&menu_item(
            "Formatting...",
            delegate_target,
            Some(sel!(menuOpenFormatting:)),
            "",
            NSEventModifierFlags::empty(),
            mtm,
        ));
        table_menu.addItem(&menu_item(
            "Sort and Filter...",
            delegate_target,
            Some(sel!(menuOpenSortFilter:)),
            "f",
            NSEventModifierFlags::Command | NSEventModifierFlags::Option,
            mtm,
        ));
        table_menu.addItem(&menu_item(
            "Sort Ascending",
            delegate_target,
            Some(sel!(menuSortAscending:)),
            "",
            NSEventModifierFlags::empty(),
            mtm,
        ));
        table_menu.addItem(&menu_item(
            "Sort Descending",
            delegate_target,
            Some(sel!(menuSortDescending:)),
            "",
            NSEventModifierFlags::empty(),
            mtm,
        ));
        add_separator(&table_menu, mtm);
        table_menu.addItem(&menu_item(
            "Add Row",
            delegate_target,
            Some(sel!(menuAddRow:)),
            "",
            NSEventModifierFlags::empty(),
            mtm,
        ));
        table_menu.addItem(&menu_item(
            "Add Column",
            delegate_target,
            Some(sel!(menuAddColumn:)),
            "",
            NSEventModifierFlags::empty(),
            mtm,
        ));
        table_menu.addItem(&menu_item(
            "Move Row Up",
            delegate_target,
            Some(sel!(menuMoveRowUp:)),
            "",
            NSEventModifierFlags::empty(),
            mtm,
        ));
        table_menu.addItem(&menu_item(
            "Move Row Down",
            delegate_target,
            Some(sel!(menuMoveRowDown:)),
            "",
            NSEventModifierFlags::empty(),
            mtm,
        ));
        table_menu.addItem(&menu_item(
            "Move Column Left",
            delegate_target,
            Some(sel!(menuMoveColumnLeft:)),
            "",
            NSEventModifierFlags::empty(),
            mtm,
        ));
        table_menu.addItem(&menu_item(
            "Move Column Right",
            delegate_target,
            Some(sel!(menuMoveColumnRight:)),
            "",
            NSEventModifierFlags::empty(),
            mtm,
        ));
        add_submenu(&main_menu, "Table", &table_menu, mtm);

        let window_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Window"));
        window_menu.addItem(&menu_item(
            "Minimize",
            delegate_target,
            Some(sel!(menuMinimizeWindow:)),
            "m",
            NSEventModifierFlags::Command,
            mtm,
        ));
        window_menu.addItem(&menu_item(
            "Bring All to Front",
            app_target,
            Some(sel!(arrangeInFront:)),
            "",
            NSEventModifierFlags::empty(),
            mtm,
        ));
        add_submenu(&main_menu, "Window", &window_menu, mtm);

        app.setWindowsMenu(Some(&window_menu));
        app.setMainMenu(Some(&main_menu));
    }

    pub(crate) fn confirm_close_with_unsaved_changes(&self) -> CloseChoice {
        let filename = self.ivars().state.borrow().title();
        let alert = NSAlert::new(self.mtm());
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str(&format!(
            "Do you want to save the changes made to \"{filename}\"?"
        )));
        alert.setInformativeText(&NSString::from_str(
            "Your changes will be lost if you don't save them.",
        ));
        alert.addButtonWithTitle(&NSString::from_str("Save"));
        alert.addButtonWithTitle(&NSString::from_str("Don't Save"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));

        match alert.runModal() {
            response if response == NSAlertFirstButtonReturn => CloseChoice::Save,
            response if response == NSAlertSecondButtonReturn => CloseChoice::Discard,
            response if response == NSAlertThirdButtonReturn => CloseChoice::Cancel,
            _ => CloseChoice::Cancel,
        }
    }
}

pub(crate) fn launch_is_default_launch(notification: &NSNotification) -> bool {
    let Some(user_info) = notification.userInfo() else {
        return true;
    };
    let user_info = unsafe { user_info.cast_unchecked::<NSString, AnyObject>() };
    let launch_key = unsafe { NSApplicationLaunchIsDefaultLaunchKey };
    let Some(value) = user_info.objectForKey(launch_key) else {
        return true;
    };
    unsafe { msg_send![&*value, boolValue] }
}

pub(crate) fn launch_path_arg() -> Option<PathBuf> {
    env::args_os().skip(1).find_map(|arg| {
        let value = arg.to_string_lossy();
        if value.starts_with("-psn_") {
            None
        } else {
            Some(PathBuf::from(arg))
        }
    })
}

#[allow(deprecated)] // setAllowedFileTypes: is deprecated in favour of setAllowedContentTypes:
pub(crate) fn choose_startup_file(mtm: MainThreadMarker) -> Option<PathBuf> {
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(false);
    panel.setResolvesAliases(true);
    panel.setTitle(Some(&NSString::from_str("Open CSV File")));
    panel.setMessage(Some(&NSString::from_str(
        "Choose a CSV file to open in Editable.",
    )));
    panel.setPrompt(Some(&NSString::from_str("Open")));
    panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&[
        NSString::from_str("csv"),
    ])));

    if panel.runModal() == NSModalResponseOK {
        panel.URL().and_then(|url| url.to_file_path())
    } else {
        None
    }
}

pub(crate) fn choose_welcome_action(mtm: MainThreadMarker) -> WelcomeChoice {
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Informational);
    alert.setMessageText(&NSString::from_str("Welcome to Editable"));
    alert.setInformativeText(&NSString::from_str(
        "Create a fresh CSV file or open an existing one.",
    ));
    let application_icon = unsafe { NSImageNameApplicationIcon };
    if let Some(icon) = NSImage::imageNamed(application_icon) {
        unsafe { alert.setIcon(Some(&icon)) };
    }
    alert.addButtonWithTitle(&NSString::from_str("Create New File"));
    alert.addButtonWithTitle(&NSString::from_str("Open Existing File"));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    // Hidden 4th button captures Cmd+Q so it works while the alert modal is running.
    alert.addButtonWithTitle(&NSString::from_str("Quit Editable"));
    let buttons = alert.buttons();
    if buttons.count() >= 3 {
        let cancel_button = buttons.objectAtIndex(2);
        cancel_button.setKeyEquivalent(&NSString::from_str("\u{1b}"));
        cancel_button.setKeyEquivalentModifierMask(NSEventModifierFlags::empty());
    }
    if buttons.count() >= 4 {
        let quit_button = buttons.objectAtIndex(3);
        quit_button.setKeyEquivalent(&NSString::from_str("q"));
        quit_button.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
        quit_button.setHidden(true);
    }

    match alert.runModal() {
        response if response == NSAlertFirstButtonReturn => WelcomeChoice::New,
        response if response == NSAlertSecondButtonReturn => WelcomeChoice::Open,
        response if response == NSAlertThirdButtonReturn => WelcomeChoice::Cancel,
        _ => WelcomeChoice::Quit,
    }
}

pub(crate) fn choose_save_target(mtm: MainThreadMarker, filename: &str) -> Option<PathBuf> {
    let panel = NSSavePanel::savePanel(mtm);
    panel.setCanCreateDirectories(true);
    panel.setTitle(Some(&NSString::from_str("Save CSV")));
    panel.setMessage(Some(&NSString::from_str(
        "Choose where to save this CSV file.",
    )));
    panel.setPrompt(Some(&NSString::from_str("Save")));
    panel.setNameFieldStringValue(&NSString::from_str(filename));

    if panel.runModal() == NSModalResponseOK {
        panel.URL().and_then(|url| url.to_file_path())
    } else {
        None
    }
}

pub(crate) fn window_delegate_matches(delegate: &Delegate, target: &NSWindow) -> bool {
    let Some(window) = delegate.ivars().window.get() else {
        return false;
    };
    let window: &NSWindow = window;
    ptr::eq(window, target)
}

pub(crate) fn window_delegate_is_visible(delegate: &Delegate) -> bool {
    delegate
        .ivars()
        .window
        .get()
        .is_some_and(|window| window.isVisible())
}

