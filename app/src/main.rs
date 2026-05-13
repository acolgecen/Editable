#![deny(unsafe_op_in_unsafe_fn)]

mod app_state;
mod selection;

use app_state::EditableState;
use editable_csv_core::{FilterOperator, FilterRule, NumberFormat, SortDirection, SortKey};
use objc2::ffi::{NSInteger, NSUInteger};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly, Message};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSAlertStyle,
    NSAlertThirdButtonReturn, NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSAutoresizingMaskOptions, NSBackingStoreType, NSBorderType, NSButton, NSButtonType, NSColor,
    NSControlStateValueOff, NSControlStateValueOn, NSControlTextEditingDelegate, NSEvent,
    NSEventModifierFlags, NSFont, NSMenu, NSMenuItem, NSModalResponseCancel, NSModalResponseOK,
    NSOpenPanel, NSPopUpButton, NSSavePanel, NSScrollView, NSTableColumn, NSTableView,
    NSTableViewColumnAutoresizingStyle, NSTableViewDataSource, NSTableViewDelegate,
    NSTableViewGridLineStyle, NSTableViewSelectionHighlightStyle, NSTextAlignment, NSTextField,
    NSTextFieldCell, NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSLocale, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect,
    NSSize, NSString, NSUserDefaults,
};
use selection::Cell;
use std::cell::{OnceCell, RefCell};
use std::env;
use std::path::PathBuf;
use std::ptr;

const TOOLBAR_HEIGHT: f64 = 44.0;
const STATUS_HEIGHT: f64 = 24.0;
const MAX_VISIBLE_COLUMNS: usize = 1_024;
const ROW_NUMBER_COLUMN: &str = "__row_number__";
const APPLE_ACCENT_COLOR_KEY: &str = "AppleAccentColor";

#[derive(Default)]
struct TableIvars {
    owner: OnceCell<*const Delegate>,
}

define_class!(
    // SAFETY:
    // - NSTableView supports subclassing for event handling.
    // - Delegate owns the table, so the stored owner pointer is valid for the
    //   lifetime of the table.
    #[unsafe(super = NSTableView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = TableIvars]
    struct EditableTableView;

    impl EditableTableView {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if let Some(owner) = self.owner() {
                owner.table_mouse_down(self, event);
            }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            if let Some(owner) = self.owner() {
                owner.table_mouse_dragged(self, event);
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            if let Some(owner) = self.owner() {
                owner.table_mouse_up();
            }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if self.owner().is_some_and(|owner| owner.table_key_down(event)) {
                return;
            }
            unsafe {
                let _: () = msg_send![super(self), keyDown: event];
            }
        }
    }
);

impl EditableTableView {
    fn init_with_frame(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TableIvars::default());
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn set_owner(&self, owner: &Delegate) {
        self.ivars().owner.set(owner as *const Delegate).ok();
    }

    fn owner(&self) -> Option<&Delegate> {
        self.ivars()
            .owner
            .get()
            .and_then(|owner| unsafe { owner.as_ref() })
    }
}

#[derive(Debug, Clone, Copy)]
enum DragSelection {
    Cells { anchor: Cell },
    Range { anchor: Cell, selected: bool },
    ToggleCells { selected: bool, last: Cell },
    Rows,
}

struct SortRuleControls {
    column: Retained<NSPopUpButton>,
    direction: Retained<NSPopUpButton>,
}

struct FilterRuleControls {
    column: Retained<NSPopUpButton>,
    operator: Retained<NSPopUpButton>,
    value: Retained<NSTextField>,
}

struct SortFilterPanel {
    window: Retained<NSWindow>,
    sort_rows: Vec<SortRuleControls>,
    filter_rows: Vec<FilterRuleControls>,
    error_label: Retained<NSTextField>,
}

struct FormattingPanel {
    header: Retained<NSButton>,
    skip_rows: Retained<NSTextField>,
    delimiter: Retained<NSPopUpButton>,
    custom_delimiter: Retained<NSTextField>,
    grouping_separator: Retained<NSTextField>,
    decimal_separator: Retained<NSTextField>,
    error_label: Retained<NSTextField>,
}

#[derive(Default)]
struct AppDelegateIvars {
    window: OnceCell<Retained<NSWindow>>,
    window_delegates: RefCell<Vec<Retained<Delegate>>>,
    table: OnceCell<Retained<EditableTableView>>,
    status: OnceCell<Retained<NSTextField>>,
    sort_filter_panel: RefCell<Option<SortFilterPanel>>,
    formatting_panel: RefCell<Option<FormattingPanel>>,
    drag_selection: RefCell<Option<DragSelection>>,
    state: RefCell<EditableState>,
}

define_class!(
    // SAFETY:
    // - NSObject has no special subclassing requirements for this delegate.
    // - Delegate does not implement Drop.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = AppDelegateIvars]
    struct Delegate;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: Editable does not customize control-text editing callbacks; the
    // conformance is required because NSTableViewDelegate refines this protocol.
    unsafe impl NSControlTextEditingDelegate for Delegate {}

    // SAFETY: NSApplicationDelegate method signatures match AppKit.
    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, notification: &NSNotification) {
            let mtm = self.mtm();
            let app = notification
                .object()
                .expect("launch notification has application")
                .downcast::<NSApplication>()
                .expect("notification object is NSApplication");
            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            self.install_main_menu(&app);

            {
                let mut state = self.ivars().state.borrow_mut();
                if state.document.is_some() {
                    state.last_error = None;
                } else if let Some(path) = launch_path_arg() {
                    if let Err(err) = state.open_path(path) {
                        state.last_error = Some(err.to_string());
                    }
                } else {
                    if let Err(err) = state.open_blank() {
                        state.last_error = Some(err.to_string());
                    }
                }
            }

            self.show_window(mtm);
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window_closed(&self, _app: &NSApplication) -> bool {
            false
        }

        #[unsafe(method(application:openFile:))]
        fn open_file(&self, _sender: &NSApplication, filename: &NSString) -> Bool {
            self.open_window_for_path(PathBuf::from(filename.to_string()))
                .into()
        }
    }

    // SAFETY: NSTableViewDataSource method signatures match AppKit.
    unsafe impl NSTableViewDataSource for Delegate {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _table_view: &NSTableView) -> NSInteger {
            self.ivars()
                .state
                .borrow()
                .document
                .as_ref()
                .map(|doc| doc.row_count() as NSInteger)
                .unwrap_or(0)
        }

        #[unsafe(method(tableView:objectValueForTableColumn:row:))]
        fn object_value(
            &self,
            _table_view: &NSTableView,
            table_column: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> *mut AnyObject {
            match table_column.and_then(visible_column_from_table_column) {
                Some(VisibleColumn::RowNumber) => {
                    return Retained::autorelease_return(
                        NSString::from_str(&(row.max(0) + 1).to_string()).into(),
                    );
                }
                Some(VisibleColumn::Data(column)) => {
                    let value = self
                        .ivars()
                        .state
                        .borrow()
                        .document
                        .as_ref()
                        .and_then(|doc| doc.cell(row.max(0) as usize, column))
                        .unwrap_or_default();
                    Retained::autorelease_return(NSString::from_str(&value).into())
                }
                None => ptr::null_mut(),
            }
        }

        #[unsafe(method(tableView:setObjectValue:forTableColumn:row:))]
        unsafe fn set_object_value(
            &self,
            table_view: &NSTableView,
            object: Option<&AnyObject>,
            table_column: Option<&NSTableColumn>,
            row: NSInteger,
        ) {
            let Some(VisibleColumn::Data(column)) =
                table_column.and_then(visible_column_from_table_column)
            else {
                return;
            };
            let value = object
                .and_then(|object| object.downcast_ref::<NSString>())
                .map(ToString::to_string)
                .unwrap_or_default();
            let result = {
                let mut state = self.ivars().state.borrow_mut();
                state.select_cell(row.max(0) as usize, column);
                state.set_cell(value)
            };
            self.handle_result(result);
            table_view.reloadData();
            self.update_status();
        }
    }

    // SAFETY: NSTableViewDelegate and NSWindowDelegate method signatures match AppKit.
    unsafe impl NSTableViewDelegate for Delegate {
        #[unsafe(method(tableViewSelectionDidChange:))]
        fn table_selection_changed(&self, _notification: &NSNotification) {
            // Selection is owned by EditableState; AppKit's row selection is suppressed.
        }

        #[unsafe(method(tableView:didClickTableColumn:))]
        fn table_column_clicked(&self, _table_view: &NSTableView, table_column: &NSTableColumn) {
            if let Some(VisibleColumn::Data(column)) = visible_column_from_table_column(table_column) {
                self.ivars().state.borrow_mut().select_column(column);
                self.refresh_table();
            }
        }

        #[unsafe(method(tableViewColumnDidMove:))]
        fn table_column_moved(&self, _notification: &NSNotification) {
            self.rebuild_columns();
            self.refresh_table();
        }

        #[unsafe(method(tableView:shouldReorderColumn:toColumn:))]
        fn should_reorder_column(
            &self,
            _table_view: &NSTableView,
            column_index: NSInteger,
            new_column_index: NSInteger,
        ) -> Bool {
            if column_index < 0 || new_column_index < 0 || column_index == new_column_index {
                return false.into();
            }
            if column_index == 0 || new_column_index == 0 {
                return false.into();
            }
            let result = self
                .ivars()
                .state
                .borrow_mut()
                .reorder_column(column_index as usize - 1, new_column_index as usize - 1);
            self.handle_result(result);
            true.into()
        }

        #[unsafe(method(tableView:shouldEditTableColumn:row:))]
        fn should_edit(
            &self,
            _table_view: &NSTableView,
            table_column: Option<&NSTableColumn>,
            _row: NSInteger,
        ) -> bool {
            matches!(
                table_column.and_then(visible_column_from_table_column),
                Some(VisibleColumn::Data(_))
            )
        }

        #[unsafe(method(tableView:willDisplayCell:forTableColumn:row:))]
        unsafe fn will_display_cell(
            &self,
            _table_view: &NSTableView,
            cell: &AnyObject,
            table_column: Option<&NSTableColumn>,
            row: NSInteger,
        ) {
            let Some(text_cell) = cell.downcast_ref::<NSTextFieldCell>() else {
                return;
            };
            let row = row.max(0) as usize;
            let visible_column = table_column.and_then(visible_column_from_table_column);
            let selected = match visible_column {
                Some(VisibleColumn::RowNumber) => self.ivars().state.borrow().selection.contains_row(row),
                Some(VisibleColumn::Data(column)) => {
                    self.ivars().state.borrow().selection.contains_cell(row, column)
                }
                None => false,
            };

            let active = match visible_column {
                Some(VisibleColumn::Data(column)) => {
                    self.ivars().state.borrow().selection.active_cell()
                        == Cell { row, column }
                }
                _ => false,
            };

            apply_table_cell_style(text_cell, visible_column, selected, active);
        }
    }

    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: &NSWindow) -> Bool {
            if !self
                .ivars()
                .state
                .borrow()
                .document
                .as_ref()
                .is_some_and(editable_csv_core::CsvDocument::is_dirty)
            {
                return true.into();
            }

            let should_close = match self.confirm_close_with_unsaved_changes() {
                CloseChoice::Save => self.save_document_with_prompt(),
                CloseChoice::Discard => true,
                CloseChoice::Cancel => false,
            };
            should_close.into()
        }

        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            self.ivars().sort_filter_panel.replace(None);
            self.ivars().formatting_panel.replace(None);
        }
    }

    impl Delegate {
        #[unsafe(method(menuOpenDocument:))]
        fn menu_open_document(&self, _sender: &AnyObject) {
            if let Some(path) = choose_startup_file(self.mtm()) {
                self.open_window_for_path(path);
            }
        }

        #[unsafe(method(menuSaveDocument:))]
        fn menu_save_document(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| delegate.save_document(sel!(saveDocument:), sender));
        }

        #[unsafe(method(menuCloseWindow:))]
        fn menu_close_window(&self, sender: &AnyObject) {
            if let Some(window) = NSApplication::sharedApplication(self.mtm()).keyWindow() {
                window.performClose(Some(sender));
            }
        }

        #[unsafe(method(menuMinimizeWindow:))]
        fn menu_minimize_window(&self, sender: &AnyObject) {
            if let Some(window) = NSApplication::sharedApplication(self.mtm()).keyWindow() {
                window.miniaturize(Some(sender));
            }
        }

        #[unsafe(method(menuUndoChange:))]
        fn menu_undo_change(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| delegate.undo_change(sel!(undoChange:), sender));
        }

        #[unsafe(method(menuRedoChange:))]
        fn menu_redo_change(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| delegate.redo_change(sel!(redoChange:), sender));
        }

        #[unsafe(method(menuToggleHeader:))]
        fn menu_toggle_header(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| delegate.toggle_header(sel!(toggleHeader:), sender));
        }

        #[unsafe(method(menuSetSkipRows:))]
        fn menu_set_skip_rows(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| {
                delegate.set_skip_rows_from_menu(sel!(setSkipRowsFromMenu:), sender)
            });
        }

        #[unsafe(method(menuOpenFormatting:))]
        fn menu_open_formatting(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| {
                delegate.open_formatting(sel!(openFormatting:), sender)
            });
        }

        #[unsafe(method(menuOpenSortFilter:))]
        fn menu_open_sort_filter(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| {
                delegate.open_sort_filter(sel!(openSortFilter:), sender)
            });
        }

        #[unsafe(method(menuSortAscending:))]
        fn menu_sort_ascending(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| {
                delegate.sort_ascending(sel!(sortAscending:), sender)
            });
        }

        #[unsafe(method(menuSortDescending:))]
        fn menu_sort_descending(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| {
                delegate.sort_descending(sel!(sortDescending:), sender)
            });
        }

        #[unsafe(method(menuAddRow:))]
        fn menu_add_row(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| delegate.add_row(sel!(addRow:), sender));
        }

        #[unsafe(method(menuAddColumn:))]
        fn menu_add_column(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| delegate.add_column(sel!(addColumn:), sender));
        }

        #[unsafe(method(menuDeleteSelection:))]
        fn menu_delete_selection(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| {
                delegate.delete_selection(sel!(deleteSelection:), sender)
            });
        }

        #[unsafe(method(menuMoveRowUp:))]
        fn menu_move_row_up(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| delegate.row_up(sel!(rowUp:), sender));
        }

        #[unsafe(method(menuMoveRowDown:))]
        fn menu_move_row_down(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| delegate.row_down(sel!(rowDown:), sender));
        }

        #[unsafe(method(menuMoveColumnLeft:))]
        fn menu_move_column_left(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| delegate.column_left(sel!(columnLeft:), sender));
        }

        #[unsafe(method(menuMoveColumnRight:))]
        fn menu_move_column_right(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| {
                delegate.column_right(sel!(columnRight:), sender)
            });
        }

        #[unsafe(method(menuEditSelectedCell:))]
        fn menu_edit_selected_cell(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| {
                delegate.edit_selected_cell(sel!(editSelectedCell:), sender)
            });
        }

        #[unsafe(method(toggleHeader:))]
        fn toggle_header(&self, sender: &AnyObject) {
            let checked = sender
                .downcast_ref::<NSButton>()
                .map(|button| button.state() == NSControlStateValueOn)
                .unwrap_or_else(|| !self.ivars().state.borrow().first_row_is_header);
            let result = {
                let mut state = self.ivars().state.borrow_mut();
                state.first_row_is_header = checked;
                state.reopen_with_options()
            };
            self.handle_result(result);
            self.rebuild_columns();
            self.refresh_table();
        }

        #[unsafe(method(setSkipRowsFromMenu:))]
        fn set_skip_rows_from_menu(&self, _sender: &AnyObject) {
            let mtm = self.mtm();
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str("Set Skip Rows"));
            alert.setInformativeText(&NSString::from_str(
                "Choose how many rows to ignore before reading the CSV data.",
            ));
            alert.addButtonWithTitle(&NSString::from_str("Apply"));
            alert.addButtonWithTitle(&NSString::from_str("Cancel"));

            let field = NSTextField::textFieldWithString(
                &NSString::from_str(&self.ivars().state.borrow().skip_rows.to_string()),
                mtm,
            );
            field.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(180.0, 24.0),
            ));
            alert.setAccessoryView(Some(&field));

            if alert.runModal() != NSAlertFirstButtonReturn {
                return;
            }

            let value = field
                .stringValue()
                .to_string()
                .trim()
                .parse::<usize>()
                .unwrap_or(0);
            let result = {
                let mut state = self.ivars().state.borrow_mut();
                state.skip_rows = value;
                state.reopen_with_options()
            };
            self.handle_result(result);
            self.rebuild_columns();
            self.refresh_table();
        }

        #[unsafe(method(applySkipRows:))]
        fn apply_skip_rows(&self, sender: &AnyObject) {
            let value = sender
                .downcast_ref::<NSTextField>()
                .map(|field| field.stringValue().to_string())
                .unwrap_or_default()
                .trim()
                .parse::<usize>()
                .unwrap_or(0);
            let result = {
                let mut state = self.ivars().state.borrow_mut();
                state.skip_rows = value;
                state.reopen_with_options()
            };
            self.handle_result(result);
            self.rebuild_columns();
            self.refresh_table();
        }

        #[unsafe(method(applyFilter:))]
        fn apply_filter(&self, sender: &AnyObject) {
            let needle = sender
                .downcast_ref::<NSTextField>()
                .map(|field| field.stringValue().to_string())
                .unwrap_or_default();
            let result = self
                .ivars()
                .state
                .borrow_mut()
                .filter_active_column_contains(needle);
            self.handle_result(result);
            self.refresh_table();
        }

        #[unsafe(method(openSortFilter:))]
        fn open_sort_filter(&self, _sender: &AnyObject) {
            self.present_sort_filter_panel();
        }

        #[unsafe(method(openFormatting:))]
        fn open_formatting(&self, _sender: &AnyObject) {
            self.present_formatting_panel();
        }

        #[unsafe(method(addSortRule:))]
        fn add_sort_rule(&self, _sender: &AnyObject) {
            let (mut sorts, filters) = self.collect_sort_filter_draft();
            let active_column = self.ivars().state.borrow().selection.active_cell().column;
            sorts.push(SortKey {
                column: active_column,
                direction: SortDirection::Ascending,
            });
            self.rebuild_sort_filter_panel(sorts, filters, "");
        }

        #[unsafe(method(addFilterRule:))]
        fn add_filter_rule(&self, _sender: &AnyObject) {
            let (sorts, mut filters) = self.collect_sort_filter_draft();
            let active_column = self.ivars().state.borrow().selection.active_cell().column;
            filters.push(FilterRule {
                column: active_column,
                operator: FilterOperator::Contains,
                value: String::new(),
            });
            self.rebuild_sort_filter_panel(sorts, filters, "");
        }

        #[unsafe(method(deleteSortRule:))]
        fn delete_sort_rule(&self, sender: &AnyObject) {
            let index = sender
                .downcast_ref::<NSButton>()
                .map(|button| button.tag())
                .unwrap_or(-1);
            let (mut sorts, filters) = self.collect_sort_filter_draft();
            if index >= 0 && (index as usize) < sorts.len() {
                sorts.remove(index as usize);
            }
            self.rebuild_sort_filter_panel(sorts, filters, "");
        }

        #[unsafe(method(deleteFilterRule:))]
        fn delete_filter_rule(&self, sender: &AnyObject) {
            let index = sender
                .downcast_ref::<NSButton>()
                .map(|button| button.tag())
                .unwrap_or(-1);
            let (sorts, mut filters) = self.collect_sort_filter_draft();
            if index >= 0 && (index as usize) < filters.len() {
                filters.remove(index as usize);
            }
            self.rebuild_sort_filter_panel(sorts, filters, "");
        }

        #[unsafe(method(resetSorting:))]
        fn reset_sorting(&self, _sender: &AnyObject) {
            let (_, filters) = self.collect_sort_filter_draft();
            self.rebuild_sort_filter_panel(Vec::new(), filters, "");
        }

        #[unsafe(method(resetFilters:))]
        fn reset_filters(&self, _sender: &AnyObject) {
            let (sorts, _) = self.collect_sort_filter_draft();
            self.rebuild_sort_filter_panel(sorts, Vec::new(), "");
        }

        #[unsafe(method(doneSortFilter:))]
        fn done_sort_filter(&self, _sender: &AnyObject) {
            let (sorts, filters) = self.collect_sort_filter_draft();
            let result = self
                .ivars()
                .state
                .borrow_mut()
                .apply_sort_filter_rules(sorts, filters);
            if let Err(err) = result {
                if let Some(panel) = self.ivars().sort_filter_panel.borrow().as_ref() {
                    panel
                        .error_label
                        .setStringValue(&NSString::from_str(&err.to_string()));
                }
                self.ivars().state.borrow_mut().last_error = Some(err.to_string());
                return;
            }
            self.ivars().state.borrow_mut().last_error = None;
            self.refresh_table();
            NSApplication::sharedApplication(self.mtm()).stopModalWithCode(NSModalResponseOK);
        }

        #[unsafe(method(cancelSortFilter:))]
        fn cancel_sort_filter(&self, _sender: &AnyObject) {
            NSApplication::sharedApplication(self.mtm()).stopModalWithCode(NSModalResponseCancel);
        }

        #[unsafe(method(doneFormatting:))]
        fn done_formatting(&self, _sender: &AnyObject) {
            let Some(options) = self.collect_formatting_draft() else {
                return;
            };
            let previous = {
                let state = self.ivars().state.borrow();
                FormattingDraft {
                    first_row_is_header: state.first_row_is_header,
                    skip_rows: state.skip_rows,
                    delimiter: state.delimiter,
                    number_format: state.number_format,
                }
            };
            let result = {
                let mut state = self.ivars().state.borrow_mut();
                state.first_row_is_header = options.first_row_is_header;
                state.skip_rows = options.skip_rows;
                state.delimiter = options.delimiter;
                state.number_format = options.number_format;
                state.reopen_with_options()
            };
            if let Err(err) = result {
                {
                    let mut state = self.ivars().state.borrow_mut();
                    state.first_row_is_header = previous.first_row_is_header;
                    state.skip_rows = previous.skip_rows;
                    state.delimiter = previous.delimiter;
                    state.number_format = previous.number_format;
                }
                if let Some(panel) = self.ivars().formatting_panel.borrow().as_ref() {
                    panel
                        .error_label
                        .setStringValue(&NSString::from_str(&err.to_string()));
                }
                self.ivars().state.borrow_mut().last_error = Some(err.to_string());
                return;
            }
            self.ivars().state.borrow_mut().last_error = None;
            self.rebuild_columns();
            self.refresh_table();
            NSApplication::sharedApplication(self.mtm()).stopModalWithCode(NSModalResponseOK);
        }

        #[unsafe(method(cancelFormatting:))]
        fn cancel_formatting(&self, _sender: &AnyObject) {
            NSApplication::sharedApplication(self.mtm()).stopModalWithCode(NSModalResponseCancel);
        }

        #[unsafe(method(sortAscending:))]
        fn sort_ascending(&self, _sender: &AnyObject) {
            let result = self
                .ivars()
                .state
                .borrow_mut()
                .sort_active_column(SortDirection::Ascending);
            self.handle_result(result);
            self.refresh_table();
        }

        #[unsafe(method(undoChange:))]
        fn undo_change(&self, _sender: &AnyObject) {
            if self.ivars().state.borrow_mut().undo() {
                self.rebuild_columns();
                self.refresh_table();
            } else {
                self.update_status();
            }
        }

        #[unsafe(method(redoChange:))]
        fn redo_change(&self, _sender: &AnyObject) {
            if self.ivars().state.borrow_mut().redo() {
                self.rebuild_columns();
                self.refresh_table();
            } else {
                self.update_status();
            }
        }

        #[unsafe(method(sortDescending:))]
        fn sort_descending(&self, _sender: &AnyObject) {
            let result = self
                .ivars()
                .state
                .borrow_mut()
                .sort_active_column(SortDirection::Descending);
            self.handle_result(result);
            self.refresh_table();
        }

        #[unsafe(method(addRow:))]
        fn add_row(&self, _sender: &AnyObject) {
            let result = self.ivars().state.borrow_mut().insert_row();
            self.handle_result(result);
            self.refresh_table();
        }

        #[unsafe(method(addColumn:))]
        fn add_column(&self, _sender: &AnyObject) {
            let result = self.ivars().state.borrow_mut().insert_column();
            self.handle_result(result);
            self.rebuild_columns();
            self.refresh_table();
        }

        #[unsafe(method(deleteSelection:))]
        fn delete_selection(&self, _sender: &AnyObject) {
            let result = self.ivars().state.borrow_mut().delete_selection();
            self.handle_result(result);
            self.rebuild_columns();
            self.refresh_table();
        }

        #[unsafe(method(rowUp:))]
        fn row_up(&self, _sender: &AnyObject) {
            let result = self.ivars().state.borrow_mut().move_active_row(-1);
            self.handle_result(result);
            self.refresh_table();
            self.restore_selection();
        }

        #[unsafe(method(rowDown:))]
        fn row_down(&self, _sender: &AnyObject) {
            let result = self.ivars().state.borrow_mut().move_active_row(1);
            self.handle_result(result);
            self.refresh_table();
            self.restore_selection();
        }

        #[unsafe(method(columnLeft:))]
        fn column_left(&self, _sender: &AnyObject) {
            let result = self.ivars().state.borrow_mut().move_active_column(-1);
            self.handle_result(result);
            self.rebuild_columns();
            self.refresh_table();
            self.restore_selection();
        }

        #[unsafe(method(columnRight:))]
        fn column_right(&self, _sender: &AnyObject) {
            let result = self.ivars().state.borrow_mut().move_active_column(1);
            self.handle_result(result);
            self.rebuild_columns();
            self.refresh_table();
            self.restore_selection();
        }

        #[unsafe(method(saveDocument:))]
        fn save_document(&self, _sender: &AnyObject) {
            self.save_document_with_prompt();
        }

        #[unsafe(method(editClickedCell:))]
        fn edit_clicked_cell(&self, _sender: &AnyObject) {
            let Some(table) = self.ivars().table.get() else {
                return;
            };
            let row = table.clickedRow();
            let column = table.clickedColumn();
            let table_column = if column >= 0 {
                table.tableColumns().objectAtIndex(column as NSUInteger)
            } else {
                return;
            };
            if row >= 0 {
                let Some(VisibleColumn::Data(column)) = visible_column_from_table_column(&table_column)
                else {
                    return;
                };
                self.ivars()
                    .state
                    .borrow_mut()
                    .select_cell(row as usize, column as usize);
                table.editColumn_row_withEvent_select(column as NSInteger, row, None, true);
            }
        }

        #[unsafe(method(editSelectedCell:))]
        fn edit_selected_cell(&self, _sender: &AnyObject) {
            let Some(table) = self.ivars().table.get() else {
                return;
            };
            let active = self.ivars().state.borrow().selection.active_cell();
            let table_column = active.column + 1;
            if table.numberOfRows() <= active.row as NSInteger
                || table.numberOfColumns() <= table_column as NSInteger
            {
                return;
            }
            table.editColumn_row_withEvent_select(
                table_column as NSInteger,
                active.row as NSInteger,
                None,
                true,
            );
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AppDelegateIvars::default());
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.ivars().state.borrow_mut().number_format = user_locale_number_format();
        this
    }

    fn show_window(&self, mtm: MainThreadMarker) {
        if let Some(window) = self.ivars().window.get() {
            window.makeKeyAndOrderFront(None);
            return;
        }

        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1120.0, 740.0)),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::Resizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(&self.ivars().state.borrow().title()));
        window.setContentMinSize(NSSize::new(860.0, 520.0));

        let content = window
            .contentView()
            .expect("window must have a content view");
        let toolbar = self.make_toolbar(mtm);
        let status = self.make_status(mtm);
        let scroll = self.make_table_area(mtm);
        content.addSubview(&toolbar);
        content.addSubview(&scroll);
        content.addSubview(&status);

        window.center();
        window.setDelegate(Some(ProtocolObject::from_ref(self)));
        window.makeKeyAndOrderFront(None);

        self.ivars().window.set(window).ok();
        self.rebuild_columns();
        self.refresh_table();
    }

    fn open_window_for_path(&self, path: PathBuf) -> bool {
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

    fn save_document_with_prompt(&self) -> bool {
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

        let result = self.ivars().state.borrow_mut().save(target);
        let ok = result.is_ok();
        self.handle_result(result);
        self.update_status();
        ok
    }

    fn with_active_window_delegate(&self, action: impl FnOnce(&Delegate)) -> bool {
        let app = NSApplication::sharedApplication(self.mtm());
        if let Some(key_window) = app.keyWindow() {
            if window_delegate_matches(self, &key_window) {
                action(self);
                return true;
            }

            let target = self
                .ivars()
                .window_delegates
                .borrow()
                .iter()
                .find(|delegate| window_delegate_matches(delegate, &key_window))
                .cloned();
            if let Some(delegate) = target {
                action(&delegate);
                return true;
            }
        }

        if let Some(main_window) = app.mainWindow() {
            if window_delegate_matches(self, &main_window) {
                action(self);
                return true;
            }

            let target = self
                .ivars()
                .window_delegates
                .borrow()
                .iter()
                .find(|delegate| window_delegate_matches(delegate, &main_window))
                .cloned();
            if let Some(delegate) = target {
                action(&delegate);
                return true;
            }
        }

        if window_delegate_is_visible(self) {
            action(self);
            return true;
        }

        let target = self
            .ivars()
            .window_delegates
            .borrow()
            .iter()
            .find(|delegate| window_delegate_is_visible(delegate))
            .cloned();
        if let Some(delegate) = target {
            action(&delegate);
            true
        } else {
            false
        }
    }

    fn install_main_menu(&self, app: &NSApplication) {
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

    fn make_toolbar(&self, mtm: MainThreadMarker) -> Retained<objc2_app_kit::NSView> {
        let toolbar = objc2_app_kit::NSView::initWithFrame(
            objc2_app_kit::NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 696.0),
                NSSize::new(1120.0, TOOLBAR_HEIGHT),
            ),
        );
        toolbar.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );

        let target = unsafe { any_ref(self) };
        toolbar.addSubview(&button(
            "Formatting",
            target,
            sel!(openFormatting:),
            mtm,
            16.0,
            10.0,
            96.0,
        ));

        toolbar.addSubview(&button(
            "Sort/Filter",
            target,
            sel!(openSortFilter:),
            mtm,
            124.0,
            10.0,
            92.0,
        ));

        let controls = [
            ("Undo", sel!(undoChange:), 230.0, 56.0),
            ("Redo", sel!(redoChange:), 290.0, 56.0),
            ("+ Row", sel!(addRow:), 362.0, 56.0),
            ("+ Col", sel!(addColumn:), 422.0, 54.0),
            ("Delete", sel!(deleteSelection:), 480.0, 60.0),
            ("Row Up", sel!(rowUp:), 548.0, 62.0),
            ("Row Down", sel!(rowDown:), 614.0, 76.0),
            ("Col Left", sel!(columnLeft:), 696.0, 70.0),
            ("Col Right", sel!(columnRight:), 774.0, 76.0),
            ("Save", sel!(saveDocument:), 858.0, 54.0),
        ];
        for (title, action, x, width) in controls {
            toolbar.addSubview(&button(title, target, action, mtm, x, 10.0, width));
        }

        toolbar
    }

    fn make_status(&self, mtm: MainThreadMarker) -> Retained<NSTextField> {
        let status = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        status.setFrame(NSRect::new(
            NSPoint::new(12.0, 2.0),
            NSSize::new(1096.0, STATUS_HEIGHT),
        ));
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        status.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        status.setAlignment(NSTextAlignment::Left);
        status.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        self.ivars().status.set(status.clone()).ok();
        status
    }

    fn make_table_area(&self, mtm: MainThreadMarker) -> Retained<NSScrollView> {
        let table = EditableTableView::init_with_frame(
            mtm,
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1120.0, 672.0)),
        );
        table.set_owner(self);
        table.setBackgroundColor(&NSColor::textBackgroundColor());
        table.setGridColor(&NSColor::separatorColor());
        table.setUsesAlternatingRowBackgroundColors(true);
        table.setAllowsEmptySelection(true);
        table.setAllowsMultipleSelection(false);
        table.setAllowsColumnSelection(false);
        table.setAllowsColumnReordering(true);
        table.setAllowsColumnResizing(true);
        table.setSelectionHighlightStyle(NSTableViewSelectionHighlightStyle::None);
        table.setColumnAutoresizingStyle(NSTableViewColumnAutoresizingStyle::NoColumnAutoresizing);
        table.setGridStyleMask(
            NSTableViewGridLineStyle::SolidHorizontalGridLineMask
                | NSTableViewGridLineStyle::SolidVerticalGridLineMask,
        );
        table.setIntercellSpacing(NSSize::new(0.0, 0.0));
        table.setRowHeight(22.0);
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(self)));
            table.setDelegate(Some(ProtocolObject::from_ref(self)));
            table.setTarget(Some(any_ref(self)));
        }

        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, STATUS_HEIGHT),
                NSSize::new(1120.0, 740.0 - TOOLBAR_HEIGHT - STATUS_HEIGHT),
            ),
        );
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setBackgroundColor(&NSColor::textBackgroundColor());
        scroll.setHasVerticalScroller(true);
        scroll.setHasHorizontalScroller(true);
        scroll.setAutohidesScrollers(false);
        scroll.setDocumentView(Some(&table));

        self.ivars().table.set(table).ok();
        scroll
    }

    fn present_formatting_panel(&self) {
        let mtm = self.mtm();
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(460.0, 306.0)),
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("Formatting"));

        let panel = self.build_formatting_panel(window.clone(), "");
        self.ivars().formatting_panel.replace(Some(panel));

        window.center();
        window.makeKeyAndOrderFront(None);
        NSApplication::sharedApplication(mtm).runModalForWindow(&window);
        window.orderOut(None);
        self.ivars().formatting_panel.replace(None);
    }

    fn build_formatting_panel(&self, window: Retained<NSWindow>, error: &str) -> FormattingPanel {
        let mtm = self.mtm();
        let target = unsafe { any_ref(self) };
        let width = 460.0;
        let height = 306.0;
        window.setContentSize(NSSize::new(width, height));

        let state = self.ivars().state.borrow();
        let content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height)),
        );

        let header = button(
            "Header row",
            target,
            sel!(toggleHeader:),
            mtm,
            24.0,
            250.0,
            120.0,
        );
        header.setButtonType(NSButtonType::Switch);
        unsafe {
            header.setTarget(None);
            header.setAction(None);
        }
        header.setState(if state.first_row_is_header {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        content.addSubview(&header);

        content.addSubview(&field_label("Skip rows", mtm, 24.0, 214.0, 110.0));
        let skip_rows = NSTextField::textFieldWithString(
            &NSString::from_str(&state.skip_rows.to_string()),
            mtm,
        );
        skip_rows.setFrame(NSRect::new(
            NSPoint::new(150.0, 210.0),
            NSSize::new(92.0, 24.0),
        ));
        skip_rows.setPlaceholderString(Some(&NSString::from_str("0")));
        content.addSubview(&skip_rows);

        content.addSubview(&section_label("Separators", mtm, 24.0, 170.0));
        content.addSubview(&field_label("CSV separator", mtm, 24.0, 136.0, 110.0));
        let delimiter = popup(
            &delimiter_titles(),
            delimiter_index(state.delimiter),
            mtm,
            150.0,
            132.0,
            132.0,
        );
        content.addSubview(&delimiter);

        let custom_delimiter = NSTextField::textFieldWithString(
            &NSString::from_str(&custom_delimiter_text(state.delimiter)),
            mtm,
        );
        custom_delimiter.setFrame(NSRect::new(
            NSPoint::new(294.0, 132.0),
            NSSize::new(48.0, 24.0),
        ));
        custom_delimiter.setPlaceholderString(Some(&NSString::from_str("x")));
        content.addSubview(&custom_delimiter);

        content.addSubview(&field_label("Thousands", mtm, 24.0, 100.0, 110.0));
        let grouping_separator = NSTextField::textFieldWithString(
            &NSString::from_str(&state.number_format.grouping_separator.to_string()),
            mtm,
        );
        grouping_separator.setFrame(NSRect::new(
            NSPoint::new(150.0, 96.0),
            NSSize::new(92.0, 24.0),
        ));
        content.addSubview(&grouping_separator);

        content.addSubview(&field_label("Decimals", mtm, 252.0, 100.0, 76.0));
        let decimal_separator = NSTextField::textFieldWithString(
            &NSString::from_str(&state.number_format.decimal_separator.to_string()),
            mtm,
        );
        decimal_separator.setFrame(NSRect::new(
            NSPoint::new(336.0, 96.0),
            NSSize::new(92.0, 24.0),
        ));
        content.addSubview(&decimal_separator);

        let error_label = muted_label(error, mtm, 24.0, 58.0, 280.0);
        error_label.setTextColor(Some(&NSColor::systemRedColor()));
        content.addSubview(&error_label);

        let cancel = button(
            "Cancel",
            target,
            sel!(cancelFormatting:),
            mtm,
            276.0,
            24.0,
            76.0,
        );
        let done = button(
            "Done",
            target,
            sel!(doneFormatting:),
            mtm,
            364.0,
            24.0,
            72.0,
        );
        done.setKeyEquivalent(&NSString::from_str("\r"));
        cancel.setKeyEquivalent(&NSString::from_str("\u{1b}"));
        content.addSubview(&cancel);
        content.addSubview(&done);

        drop(state);
        window.setContentView(Some(&content));
        FormattingPanel {
            header,
            skip_rows,
            delimiter,
            custom_delimiter,
            grouping_separator,
            decimal_separator,
            error_label,
        }
    }

    fn collect_formatting_draft(&self) -> Option<FormattingDraft> {
        let panel_ref = self.ivars().formatting_panel.borrow();
        let panel = panel_ref.as_ref()?;

        let skip_rows = match panel
            .skip_rows
            .stringValue()
            .to_string()
            .trim()
            .parse::<usize>()
        {
            Ok(value) => value,
            Err(_) => {
                panel
                    .error_label
                    .setStringValue(&NSString::from_str("Skip rows must be a whole number."));
                return None;
            }
        };
        let delimiter = match delimiter_at(
            panel.delimiter.indexOfSelectedItem(),
            &panel.custom_delimiter.stringValue().to_string(),
        ) {
            Ok(value) => value,
            Err(err) => {
                panel.error_label.setStringValue(&NSString::from_str(&err));
                return None;
            }
        };
        let grouping_separator = match single_character(
            &panel.grouping_separator.stringValue().to_string(),
            "Thousands separator",
        ) {
            Ok(value) => value,
            Err(err) => {
                panel.error_label.setStringValue(&NSString::from_str(&err));
                return None;
            }
        };
        let decimal_separator = match single_character(
            &panel.decimal_separator.stringValue().to_string(),
            "Decimal separator",
        ) {
            Ok(value) => value,
            Err(err) => {
                panel.error_label.setStringValue(&NSString::from_str(&err));
                return None;
            }
        };
        if grouping_separator == decimal_separator {
            panel.error_label.setStringValue(&NSString::from_str(
                "Thousands and decimal separators must differ.",
            ));
            return None;
        }

        Some(FormattingDraft {
            first_row_is_header: panel.header.state() == NSControlStateValueOn,
            skip_rows,
            delimiter,
            number_format: NumberFormat {
                grouping_separator,
                decimal_separator,
            },
        })
    }

    fn present_sort_filter_panel(&self) {
        let mtm = self.mtm();
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(680.0, 390.0)),
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("Sort and Filter"));

        let (sorts, filters) = {
            let state = self.ivars().state.borrow();
            (state.sort_keys(), state.filter_rules())
        };
        let panel = self.build_sort_filter_panel(window.clone(), sorts, filters, "");
        self.ivars().sort_filter_panel.replace(Some(panel));

        window.center();
        window.makeKeyAndOrderFront(None);
        NSApplication::sharedApplication(mtm).runModalForWindow(&window);
        window.orderOut(None);
        self.ivars().sort_filter_panel.replace(None);
    }

    fn rebuild_sort_filter_panel(
        &self,
        sort_rules: Vec<SortKey>,
        filter_rules: Vec<FilterRule>,
        error: &str,
    ) {
        let Some(window) = self
            .ivars()
            .sort_filter_panel
            .borrow()
            .as_ref()
            .map(|panel| panel.window.clone())
        else {
            return;
        };
        let panel = self.build_sort_filter_panel(window, sort_rules, filter_rules, error);
        self.ivars().sort_filter_panel.replace(Some(panel));
    }

    fn build_sort_filter_panel(
        &self,
        window: Retained<NSWindow>,
        sort_rules: Vec<SortKey>,
        filter_rules: Vec<FilterRule>,
        error: &str,
    ) -> SortFilterPanel {
        let mtm = self.mtm();
        let row_count = sort_rules.len().max(1) + filter_rules.len().max(1);
        let height = (252.0 + row_count as f64 * 34.0).min(720.0).max(390.0);
        window.setContentSize(NSSize::new(680.0, height));

        let target = unsafe { any_ref(self) };
        let content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(680.0, height)),
        );
        let columns = self.column_titles();
        let mut y = height - 64.0;
        content.addSubview(&section_label("Sorting", mtm, 24.0, y));
        content.addSubview(&button(
            "+",
            target,
            sel!(addSortRule:),
            mtm,
            526.0,
            y - 4.0,
            28.0,
        ));
        content.addSubview(&button(
            "Reset sorting",
            target,
            sel!(resetSorting:),
            mtm,
            560.0,
            y - 4.0,
            96.0,
        ));

        y -= 34.0;
        let mut sort_rows = Vec::new();
        if sort_rules.is_empty() {
            content.addSubview(&muted_label("No sort rules", mtm, 24.0, y + 4.0, 220.0));
            y -= 34.0;
        } else {
            for (index, rule) in sort_rules.iter().enumerate() {
                let column = popup(&columns, rule.column, mtm, 24.0, y, 270.0);
                let direction = popup(
                    &["Ascending".to_string(), "Descending".to_string()],
                    match rule.direction {
                        SortDirection::Ascending => 0,
                        SortDirection::Descending => 1,
                    },
                    mtm,
                    306.0,
                    y,
                    130.0,
                );
                let delete = button("Delete", target, sel!(deleteSortRule:), mtm, 584.0, y, 72.0);
                delete.setTag(index as NSInteger);
                content.addSubview(&column);
                content.addSubview(&direction);
                content.addSubview(&delete);
                sort_rows.push(SortRuleControls { column, direction });
                y -= 34.0;
            }
        }

        y -= 12.0;
        content.addSubview(&section_label("Filters", mtm, 24.0, y));
        content.addSubview(&button(
            "+",
            target,
            sel!(addFilterRule:),
            mtm,
            526.0,
            y - 4.0,
            28.0,
        ));
        content.addSubview(&button(
            "Reset Filters",
            target,
            sel!(resetFilters:),
            mtm,
            560.0,
            y - 4.0,
            96.0,
        ));

        y -= 34.0;
        let mut filter_rows = Vec::new();
        if filter_rules.is_empty() {
            content.addSubview(&muted_label("No filter rules", mtm, 24.0, y + 4.0, 220.0));
        } else {
            for (index, rule) in filter_rules.iter().enumerate() {
                let column = popup(
                    &columns,
                    rule.column.min(columns.len().saturating_sub(1)),
                    mtm,
                    24.0,
                    y,
                    180.0,
                );
                let operator = popup(
                    &filter_operator_titles(),
                    filter_operator_index(rule.operator),
                    mtm,
                    216.0,
                    y,
                    154.0,
                );
                let value = NSTextField::textFieldWithString(&NSString::from_str(&rule.value), mtm);
                value.setFrame(NSRect::new(
                    NSPoint::new(382.0, y),
                    NSSize::new(150.0, 24.0),
                ));
                value.setPlaceholderString(Some(&NSString::from_str("Value")));
                let delete = button(
                    "Delete",
                    target,
                    sel!(deleteFilterRule:),
                    mtm,
                    548.0,
                    y,
                    72.0,
                );
                delete.setTag(index as NSInteger);
                content.addSubview(&column);
                content.addSubview(&operator);
                content.addSubview(&value);
                content.addSubview(&delete);
                filter_rows.push(FilterRuleControls {
                    column,
                    operator,
                    value,
                });
                y -= 34.0;
            }
        }

        let error_label = muted_label(error, mtm, 24.0, 58.0, 450.0);
        error_label.setTextColor(Some(&NSColor::systemRedColor()));
        content.addSubview(&error_label);

        let cancel = button(
            "Cancel",
            target,
            sel!(cancelSortFilter:),
            mtm,
            492.0,
            24.0,
            76.0,
        );
        let done = button(
            "Done",
            target,
            sel!(doneSortFilter:),
            mtm,
            580.0,
            24.0,
            76.0,
        );
        done.setKeyEquivalent(&NSString::from_str("\r"));
        cancel.setKeyEquivalent(&NSString::from_str("\u{1b}"));
        content.addSubview(&cancel);
        content.addSubview(&done);

        if columns.is_empty() {
            content.addSubview(&muted_label(
                "Open a table with at least one column.",
                mtm,
                24.0,
                height - 74.0,
                300.0,
            ));
        }

        window.setContentView(Some(&content));
        SortFilterPanel {
            window,
            sort_rows,
            filter_rows,
            error_label,
        }
    }

    fn collect_sort_filter_draft(&self) -> (Vec<SortKey>, Vec<FilterRule>) {
        let panel_ref = self.ivars().sort_filter_panel.borrow();
        let Some(panel) = panel_ref.as_ref() else {
            return (Vec::new(), Vec::new());
        };
        let sorts = panel
            .sort_rows
            .iter()
            .filter_map(|row| {
                let column = row.column.indexOfSelectedItem();
                if column < 0 {
                    return None;
                }
                Some(SortKey {
                    column: column as usize,
                    direction: if row.direction.indexOfSelectedItem() == 1 {
                        SortDirection::Descending
                    } else {
                        SortDirection::Ascending
                    },
                })
            })
            .collect::<Vec<_>>();

        let filters = panel
            .filter_rows
            .iter()
            .filter_map(|row| {
                let column = row.column.indexOfSelectedItem();
                if column < 0 {
                    return None;
                }
                let operator = filter_operator_at(row.operator.indexOfSelectedItem());
                let value = row.value.stringValue().to_string();
                if value.is_empty()
                    && !matches!(
                        operator,
                        FilterOperator::IsEmpty | FilterOperator::IsNotEmpty
                    )
                {
                    return None;
                }
                Some(FilterRule {
                    column: column as usize,
                    operator,
                    value,
                })
            })
            .collect::<Vec<_>>();
        (sorts, filters)
    }

    fn column_titles(&self) -> Vec<String> {
        let count = self
            .ivars()
            .state
            .borrow()
            .document
            .as_ref()
            .map(|doc| doc.column_count())
            .unwrap_or(0);
        (0..count)
            .map(|column| self.ivars().state.borrow().column_title(column))
            .collect()
    }

    fn rebuild_columns(&self) {
        let Some(table) = self.ivars().table.get() else {
            return;
        };
        let mtm = self.mtm();
        let columns = table.tableColumns();
        for idx in (0..columns.count()).rev() {
            let column = columns.objectAtIndex(idx);
            table.removeTableColumn(&column);
        }

        let row_identifier = NSString::from_str(ROW_NUMBER_COLUMN);
        let row_column =
            NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), &row_identifier);
        row_column.setTitle(&NSString::from_str("#"));
        row_column.setWidth(54.0);
        row_column.setMinWidth(42.0);
        row_column.setEditable(false);
        table.addTableColumn(&row_column);

        let count = self
            .ivars()
            .state
            .borrow()
            .document
            .as_ref()
            .map(|doc| doc.column_count().min(MAX_VISIBLE_COLUMNS))
            .unwrap_or(0);
        for column in 0..count {
            let identifier = NSString::from_str(&format!("c:{column}"));
            let table_column =
                NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), &identifier);
            table_column.setTitle(&NSString::from_str(
                &self.ivars().state.borrow().column_title(column),
            ));
            table_column.setWidth(160.0);
            table_column.setMinWidth(48.0);
            table_column.setEditable(true);
            table.addTableColumn(&table_column);
        }
    }

    fn refresh_table(&self) {
        if let Some(table) = self.ivars().table.get() {
            table.reloadData();
        }
        self.update_status();
        self.restore_selection();
    }

    fn update_status(&self) {
        if let Some(status) = self.ivars().status.get() {
            status.setStringValue(&NSString::from_str(
                &self.ivars().state.borrow().status_text(),
            ));
        }
        if let Some(window) = self.ivars().window.get() {
            let state = self.ivars().state.borrow();
            window.setTitle(&NSString::from_str(&state.title()));
            window.setDocumentEdited(
                state
                    .document
                    .as_ref()
                    .is_some_and(editable_csv_core::CsvDocument::is_dirty),
            );
        }
    }

    fn restore_selection(&self) {
        let Some(table) = self.ivars().table.get() else {
            return;
        };
        unsafe { table.deselectAll(None) };
        let active = self.ivars().state.borrow().selection.active_cell();
        if table.numberOfRows() > active.row as NSInteger {
            table.scrollRowToVisible(active.row as NSInteger);
        }
        let visible_column = active.column + 1;
        if table.numberOfColumns() > visible_column as NSInteger {
            table.scrollColumnToVisible(visible_column as NSInteger);
        }
    }

    fn table_mouse_down(&self, table: &EditableTableView, event: &NSEvent) {
        let Some(hit) = table_hit(table, event) else {
            return;
        };
        let modifiers = event.modifierFlags();
        match hit {
            TableHit::RowNumber(row) => {
                let mut state = self.ivars().state.borrow_mut();
                if modifiers.contains(NSEventModifierFlags::Shift) {
                    state.select_row_range_to(row);
                } else {
                    state.select_row(row);
                }
                self.ivars()
                    .drag_selection
                    .replace(Some(DragSelection::Rows));
            }
            TableHit::Data { cell, table_column } => {
                if event.clickCount() >= 2 {
                    self.ivars()
                        .state
                        .borrow_mut()
                        .select_cell(cell.row, cell.column);
                    table.reloadData();
                    table.editColumn_row_withEvent_select(
                        table_column,
                        cell.row as NSInteger,
                        Some(event),
                        true,
                    );
                    self.ivars().drag_selection.replace(None);
                    return;
                }

                if modifiers.contains(NSEventModifierFlags::Command) {
                    let selected = self
                        .ivars()
                        .state
                        .borrow_mut()
                        .toggle_cell_selection(cell.row, cell.column);
                    self.ivars()
                        .drag_selection
                        .replace(Some(DragSelection::ToggleCells {
                            selected,
                            last: cell,
                        }));
                } else if modifiers.contains(NSEventModifierFlags::Shift) {
                    let anchor = self.ivars().state.borrow().selection.anchor_cell();
                    let selected = !self
                        .ivars()
                        .state
                        .borrow()
                        .selection
                        .contains_cell(cell.row, cell.column);
                    self.ivars()
                        .state
                        .borrow_mut()
                        .set_cell_range_selection_from(anchor, cell.row, cell.column, selected);
                    self.ivars()
                        .drag_selection
                        .replace(Some(DragSelection::Range { anchor, selected }));
                } else {
                    self.ivars()
                        .state
                        .borrow_mut()
                        .select_cell(cell.row, cell.column);
                    self.ivars()
                        .drag_selection
                        .replace(Some(DragSelection::Cells { anchor: cell }));
                }
            }
        }
        table.reloadData();
        self.update_status();
    }

    fn table_mouse_dragged(&self, table: &EditableTableView, event: &NSEvent) {
        let Some(hit) = table_hit(table, event) else {
            return;
        };
        let Some(drag) = *self.ivars().drag_selection.borrow() else {
            return;
        };
        match (drag, hit) {
            (
                DragSelection::Rows,
                TableHit::RowNumber(row)
                | TableHit::Data {
                    cell: Cell { row, .. },
                    ..
                },
            ) => {
                self.ivars().state.borrow_mut().select_row_range_to(row);
            }
            (DragSelection::Cells { anchor }, TableHit::Data { cell, .. }) => {
                self.ivars().state.borrow_mut().select_cell_range_from(
                    anchor,
                    cell.row,
                    cell.column,
                );
            }
            (DragSelection::Range { anchor, selected }, TableHit::Data { cell, .. }) => {
                self.ivars()
                    .state
                    .borrow_mut()
                    .set_cell_range_selection_from(anchor, cell.row, cell.column, selected);
            }
            (DragSelection::ToggleCells { selected, last }, TableHit::Data { cell, .. }) => {
                if cell != last {
                    self.ivars().state.borrow_mut().set_cell_selection(
                        cell.row,
                        cell.column,
                        selected,
                    );
                    self.ivars()
                        .drag_selection
                        .replace(Some(DragSelection::ToggleCells {
                            selected,
                            last: cell,
                        }));
                }
            }
            _ => {}
        }
        table.reloadData();
        self.update_status();
    }

    fn table_mouse_up(&self) {
        self.ivars().drag_selection.replace(None);
    }

    fn table_key_down(&self, event: &NSEvent) -> bool {
        let modifiers = event.modifierFlags();
        if !modifiers.contains(NSEventModifierFlags::Command) {
            return false;
        }
        let characters = event
            .charactersIgnoringModifiers()
            .map(|value| value.to_string())
            .unwrap_or_default();
        if !characters.eq_ignore_ascii_case("z") {
            return false;
        }

        if modifiers.contains(NSEventModifierFlags::Shift) {
            if self.ivars().state.borrow_mut().redo() {
                self.rebuild_columns();
                self.refresh_table();
            }
        } else if self.ivars().state.borrow_mut().undo() {
            self.rebuild_columns();
            self.refresh_table();
        }
        true
    }

    fn handle_result(&self, result: editable_csv_core::Result<()>) {
        if let Err(err) = result {
            self.ivars().state.borrow_mut().last_error = Some(err.to_string());
        } else {
            self.ivars().state.borrow_mut().last_error = None;
        }
    }

    fn confirm_close_with_unsaved_changes(&self) -> CloseChoice {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisibleColumn {
    RowNumber,
    Data(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableHit {
    RowNumber(usize),
    Data { cell: Cell, table_column: NSInteger },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseChoice {
    Save,
    Discard,
    Cancel,
}

struct FormattingDraft {
    first_row_is_header: bool,
    skip_rows: usize,
    delimiter: u8,
    number_format: NumberFormat,
}

fn apply_table_cell_style(
    cell: &NSTextFieldCell,
    visible_column: Option<VisibleColumn>,
    selected: bool,
    active: bool,
) {
    cell.setFont(Some(&table_data_font()));

    if selected {
        cell.setDrawsBackground(true);
        cell.setBackgroundColor(Some(&selection_background(active)));
        cell.setTextColor(Some(&NSColor::labelColor()));
    } else if matches!(visible_column, Some(VisibleColumn::RowNumber)) {
        cell.setDrawsBackground(true);
        cell.setBackgroundColor(Some(&NSColor::quaternarySystemFillColor()));
        cell.setTextColor(Some(&NSColor::secondaryLabelColor()));
    } else {
        cell.setDrawsBackground(false);
        cell.setTextColor(Some(&NSColor::labelColor()));
    }
}

fn table_data_font() -> Retained<NSFont> {
    NSFont::userFixedPitchFontOfSize(13.0).unwrap_or_else(|| NSFont::systemFontOfSize(13.0))
}

fn selection_background(active: bool) -> Retained<NSColor> {
    user_accent_color().colorWithAlphaComponent(if active { 0.34 } else { 0.22 })
}

fn user_accent_color() -> Retained<NSColor> {
    let defaults = NSUserDefaults::standardUserDefaults();
    let accent_key = NSString::from_str(APPLE_ACCENT_COLOR_KEY);
    match defaults.objectForKey(&accent_key) {
        None => NSColor::systemBlueColor(),
        Some(_) if defaults.integerForKey(&accent_key) == -1 => NSColor::systemBlueColor(),
        Some(_) => NSColor::controlAccentColor(),
    }
}

fn user_locale_number_format() -> NumberFormat {
    let locale = NSLocale::currentLocale();
    let grouping_separator = locale
        .groupingSeparator()
        .to_string()
        .chars()
        .next()
        .unwrap_or(',');
    let decimal_separator = locale
        .decimalSeparator()
        .to_string()
        .chars()
        .next()
        .unwrap_or('.');
    if grouping_separator == decimal_separator {
        NumberFormat::default()
    } else {
        NumberFormat {
            grouping_separator,
            decimal_separator,
        }
    }
}

fn table_hit(table: &EditableTableView, event: &NSEvent) -> Option<TableHit> {
    let point = table.convertPoint_fromView(event.locationInWindow(), None);
    let row = table.rowAtPoint(point);
    let table_column = table.columnAtPoint(point);
    if row < 0 || table_column < 0 {
        return None;
    }

    let column = table
        .tableColumns()
        .objectAtIndex(table_column as NSUInteger);
    match visible_column_from_table_column(&column)? {
        VisibleColumn::RowNumber => Some(TableHit::RowNumber(row as usize)),
        VisibleColumn::Data(column) => Some(TableHit::Data {
            cell: Cell {
                row: row as usize,
                column,
            },
            table_column,
        }),
    }
}

fn visible_column_from_table_column(table_column: &NSTableColumn) -> Option<VisibleColumn> {
    let identifier = table_column.identifier().to_string();
    if identifier == ROW_NUMBER_COLUMN {
        return Some(VisibleColumn::RowNumber);
    }
    identifier
        .strip_prefix("c:")
        .and_then(|value| value.parse::<usize>().ok())
        .map(VisibleColumn::Data)
}

fn launch_path_arg() -> Option<PathBuf> {
    env::args_os().skip(1).find_map(|arg| {
        let value = arg.to_string_lossy();
        if value.starts_with("-psn_") {
            None
        } else {
            Some(PathBuf::from(arg))
        }
    })
}

fn choose_startup_file(mtm: MainThreadMarker) -> Option<PathBuf> {
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(false);
    panel.setResolvesAliases(true);
    panel.setTitle(Some(&NSString::from_str("Open File")));
    panel.setMessage(Some(&NSString::from_str(
        "Choose a file to open in Editable.",
    )));
    panel.setPrompt(Some(&NSString::from_str("Open")));

    if panel.runModal() == NSModalResponseOK {
        panel.URL().and_then(|url| url.to_file_path())
    } else {
        None
    }
}

fn choose_save_target(mtm: MainThreadMarker, filename: &str) -> Option<PathBuf> {
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

fn button(
    title: &str,
    target: &AnyObject,
    action: objc2::runtime::Sel,
    mtm: MainThreadMarker,
    x: f64,
    y: f64,
    width: f64,
) -> Retained<NSButton> {
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(action),
            mtm,
        )
    };
    button.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, 24.0)));
    button
}

fn menu_item(
    title: &str,
    target: &AnyObject,
    action: Option<objc2::runtime::Sel>,
    key: &str,
    modifiers: NSEventModifierFlags,
    mtm: MainThreadMarker,
) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    };
    unsafe { item.setTarget(Some(target)) };
    item.setKeyEquivalentModifierMask(modifiers);
    item
}

fn add_separator(menu: &NSMenu, mtm: MainThreadMarker) {
    menu.addItem(&NSMenuItem::separatorItem(mtm));
}

fn add_submenu(parent: &NSMenu, title: &str, submenu: &NSMenu, mtm: MainThreadMarker) {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            None,
            &NSString::from_str(""),
        )
    };
    item.setSubmenu(Some(submenu));
    parent.addItem(&item);
}

fn window_delegate_matches(delegate: &Delegate, target: &NSWindow) -> bool {
    let Some(window) = delegate.ivars().window.get() else {
        return false;
    };
    let window: &NSWindow = window;
    ptr::eq(window, target)
}

fn window_delegate_is_visible(delegate: &Delegate) -> bool {
    delegate
        .ivars()
        .window
        .get()
        .is_some_and(|window| window.isVisible())
}

fn section_label(title: &str, mtm: MainThreadMarker, x: f64, y: f64) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(title), mtm);
    label.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(220.0, 20.0)));
    label.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
    label.setTextColor(Some(&NSColor::labelColor()));
    label
}

fn field_label(
    title: &str,
    mtm: MainThreadMarker,
    x: f64,
    y: f64,
    width: f64,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(title), mtm);
    label.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, 20.0)));
    label.setFont(Some(&NSFont::systemFontOfSize(12.0)));
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    label
}

fn muted_label(
    title: &str,
    mtm: MainThreadMarker,
    x: f64,
    y: f64,
    width: f64,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(title), mtm);
    label.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, 20.0)));
    label.setFont(Some(&NSFont::systemFontOfSize(12.0)));
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    label
}

fn popup(
    titles: &[String],
    selected: usize,
    mtm: MainThreadMarker,
    x: f64,
    y: f64,
    width: f64,
) -> Retained<NSPopUpButton> {
    let popup = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        NSRect::new(NSPoint::new(x, y), NSSize::new(width, 24.0)),
        false,
    );
    if titles.is_empty() {
        popup.addItemWithTitle(&NSString::from_str("No columns"));
    } else {
        for title in titles {
            popup.addItemWithTitle(&NSString::from_str(title));
        }
        popup.selectItemAtIndex(selected.min(titles.len() - 1) as NSInteger);
    }
    popup
}

fn delimiter_titles() -> Vec<String> {
    ["Comma", "Semicolon", "Tab", "Pipe", "Colon", "Custom"]
        .iter()
        .map(|title| title.to_string())
        .collect()
}

fn delimiter_index(delimiter: u8) -> usize {
    match delimiter {
        b',' => 0,
        b';' => 1,
        b'\t' => 2,
        b'|' => 3,
        b':' => 4,
        _ => 5,
    }
}

fn custom_delimiter_text(delimiter: u8) -> String {
    if delimiter_index(delimiter) == 5 {
        (delimiter as char).to_string()
    } else {
        String::new()
    }
}

fn delimiter_at(index: NSInteger, custom: &str) -> std::result::Result<u8, String> {
    match index {
        0 => Ok(b','),
        1 => Ok(b';'),
        2 => Ok(b'\t'),
        3 => Ok(b'|'),
        4 => Ok(b':'),
        _ => custom_delimiter(custom),
    }
}

fn custom_delimiter(value: &str) -> std::result::Result<u8, String> {
    let delimiter = if value == r"\t" {
        '\t'
    } else {
        single_character(value, "Custom separator")?
    };
    if delimiter == '\n' || delimiter == '\r' || delimiter == '"' {
        return Err("Custom separator cannot be a quote or line break.".to_string());
    }
    if !delimiter.is_ascii() {
        return Err("Custom separator must be one ASCII character.".to_string());
    }
    Ok(delimiter as u8)
}

fn single_character(value: &str, label: &str) -> std::result::Result<char, String> {
    let mut chars = value.chars();
    let Some(ch) = chars.next() else {
        return Err(format!("{label} must be one character."));
    };
    if chars.next().is_some() {
        return Err(format!("{label} must be one character."));
    }
    Ok(ch)
}

fn filter_operator_titles() -> Vec<String> {
    [
        "Contains",
        "Does not contain",
        "Equals",
        "Does not equal",
        "Starts with",
        "Ends with",
        "Greater than",
        "Greater than or equal",
        "Less than",
        "Less than or equal",
        "Is empty",
        "Is not empty",
    ]
    .iter()
    .map(|title| title.to_string())
    .collect()
}

fn filter_operator_index(operator: FilterOperator) -> usize {
    match operator {
        FilterOperator::Contains => 0,
        FilterOperator::DoesNotContain => 1,
        FilterOperator::Equals => 2,
        FilterOperator::DoesNotEqual => 3,
        FilterOperator::StartsWith => 4,
        FilterOperator::EndsWith => 5,
        FilterOperator::GreaterThan => 6,
        FilterOperator::GreaterThanOrEqual => 7,
        FilterOperator::LessThan => 8,
        FilterOperator::LessThanOrEqual => 9,
        FilterOperator::IsEmpty => 10,
        FilterOperator::IsNotEmpty => 11,
    }
}

fn filter_operator_at(index: NSInteger) -> FilterOperator {
    match index {
        1 => FilterOperator::DoesNotContain,
        2 => FilterOperator::Equals,
        3 => FilterOperator::DoesNotEqual,
        4 => FilterOperator::StartsWith,
        5 => FilterOperator::EndsWith,
        6 => FilterOperator::GreaterThan,
        7 => FilterOperator::GreaterThanOrEqual,
        8 => FilterOperator::LessThan,
        9 => FilterOperator::LessThanOrEqual,
        10 => FilterOperator::IsEmpty,
        11 => FilterOperator::IsNotEmpty,
        _ => FilterOperator::Contains,
    }
}

unsafe fn any_ref<T: ?Sized + Message>(value: &T) -> &AnyObject {
    unsafe { &*(value as *const T as *const AnyObject) }
}

fn main() {
    let mtm = MainThreadMarker::new().expect("Editable must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let delegate = Delegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}
