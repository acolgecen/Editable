use objc2::ffi::NSInteger;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadOnly;
use objc2_app_kit::{
    NSButton, NSColor, NSEventModifierFlags, NSFont, NSMenu, NSMenuItem, NSPopUpButton, NSTextField,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

// Reusable AppKit control builders (buttons, popups, labels, menu items)
// shared by the panels and dialogs.

pub(crate) fn button(
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

pub(crate) fn menu_item(
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

pub(crate) fn add_separator(menu: &NSMenu, mtm: MainThreadMarker) {
    menu.addItem(&NSMenuItem::separatorItem(mtm));
}

pub(crate) fn add_submenu(parent: &NSMenu, title: &str, submenu: &NSMenu, mtm: MainThreadMarker) {
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

pub(crate) fn section_label(
    title: &str,
    mtm: MainThreadMarker,
    x: f64,
    y: f64,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(title), mtm);
    label.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(220.0, 20.0)));
    label.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
    label.setTextColor(Some(&NSColor::labelColor()));
    label
}

/// A small, secondary-colored label used for field captions and muted hints.
pub(crate) fn secondary_label(
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

pub(crate) fn popup(
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
