//! The AppKit user interface.
//!
//! One [`Delegate`] objc object backs each window and owns an [`EditableState`].
//! The work is split by concern so a change usually touches a single file:
//!
//! - [`delegate`] — the `define_class!` objc surface (protocol methods, menu and
//!   toolbar actions) plus core routing between windows.
//! - [`views`] — building and laying out the window, toolbar, table, status bar,
//!   and saving overlay, and styling cells.
//! - [`panels`] — the Find, Formatting, and Sort & Filter panels.
//! - [`input`] — mouse, keyboard, in-cell editing, and clipboard.
//! - [`dialogs`] — the main menu and the open/save/welcome/close modal flows.
//! - [`widgets`] — small reusable AppKit control builders.
//!
//! The submodules reach each other's items through `use super::*`; the
//! `pub(crate) use` re-exports below make that possible without per-item imports.

mod delegate;
mod dialogs;
mod input;
mod panels;
mod views;
mod widgets;

pub(crate) use delegate::Delegate;
pub(crate) use dialogs::*;
pub(crate) use input::*;
pub(crate) use panels::*;
pub(crate) use views::*;
pub(crate) use widgets::*;

use crate::selection::Cell;
use crate::state::{EditableState, SelectionMetric};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::Message;
use objc2_app_kit::{
    NSBox, NSPopUpButton, NSProgressIndicator, NSScrollView, NSTextField, NSView, NSWindow,
};
use std::cell::{OnceCell, RefCell};

pub(crate) const TOOLBAR_HEIGHT: f64 = 44.0;
pub(crate) const TOOLBAR_BUTTON_HEIGHT: f64 = 28.0;
pub(crate) const TOOLBAR_HORIZONTAL_PADDING: f64 = 12.0;
pub(crate) const TOOLBAR_VERTICAL_PADDING: f64 = 8.0;
pub(crate) const TOOLBAR_BUTTON_GAP: f64 = 8.0;
pub(crate) const TOOLBAR_ROW_GAP: f64 = 6.0;
pub(crate) const STATUS_HEIGHT: f64 = 36.0;
pub(crate) const STATUS_LABEL_HEIGHT: f64 = 20.0;
pub(crate) const STATUS_CONTROL_HEIGHT: f64 = 24.0;
pub(crate) const STATUS_SEPARATOR_HEIGHT: f64 = 1.0;
pub(crate) const STATUS_SIDE_PADDING: f64 = 12.0;
pub(crate) const STATUS_LABEL_GAP: f64 = 12.0;
pub(crate) const FIND_WINDOW_WIDTH: f64 = 540.0;
pub(crate) const FIND_WINDOW_HEIGHT: f64 = 112.0;
pub(crate) const MAX_VISIBLE_COLUMNS: usize = 1_024;
pub(crate) const ROW_NUMBER_COLUMN: &str = "__row_number__";
pub(crate) const APPLE_ACCENT_COLOR_KEY: &str = "AppleAccentColor";
pub(crate) const KEY_RETURN: u16 = 36;
pub(crate) const KEY_KEYPAD_ENTER: u16 = 76;
pub(crate) const KEY_BACKSPACE: u16 = 51;
pub(crate) const KEY_FORWARD_DELETE: u16 = 117;
pub(crate) const KEY_LEFT_ARROW: u16 = 123;
pub(crate) const KEY_RIGHT_ARROW: u16 = 124;
pub(crate) const KEY_DOWN_ARROW: u16 = 125;
pub(crate) const KEY_UP_ARROW: u16 = 126;

/// Backing storage for a window's [`Delegate`]. Fields are private to the `ui`
/// module tree, so every submodule can read them through `self.ivars()`.
#[derive(Default)]
pub(crate) struct AppDelegateIvars {
    window: OnceCell<Retained<NSWindow>>,
    window_delegates: RefCell<Vec<Retained<Delegate>>>,
    table: OnceCell<Retained<EditableTableView>>,
    toolbar: OnceCell<Retained<NSView>>,
    scroll: OnceCell<Retained<NSScrollView>>,
    saving_overlay: OnceCell<Retained<NSBox>>,
    saving_progress: OnceCell<Retained<NSProgressIndicator>>,
    saving_label: OnceCell<Retained<NSTextField>>,
    status_bar: OnceCell<Retained<NSBox>>,
    status_separator: OnceCell<Retained<NSBox>>,
    status: OnceCell<Retained<NSTextField>>,
    selection_status: OnceCell<Retained<NSPopUpButton>>,
    selected_selection_metric: RefCell<SelectionMetric>,
    sort_filter_panel: RefCell<Option<SortFilterPanel>>,
    formatting_panel: RefCell<Option<FormattingPanel>>,
    find_panel: RefCell<Option<FindPanel>>,
    find_query: RefCell<String>,
    find_matches: RefCell<Vec<Cell>>,
    find_index: RefCell<Option<usize>>,
    drag_selection: RefCell<Option<DragSelection>>,
    is_saving: RefCell<bool>,
    state: RefCell<EditableState>,
}

/// Which kind of column an `NSTableColumn` maps to: the leading row-number
/// gutter or a zero-based data column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VisibleColumn {
    RowNumber,
    Data(usize),
}

/// The user's response to the unsaved-changes prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseChoice {
    Save,
    Discard,
    Cancel,
}

/// The user's response to the welcome prompt shown on a bare launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WelcomeChoice {
    New,
    Open,
    Cancel,
    Quit,
}

/// Validated values collected from the Formatting panel before they are applied.
pub(crate) struct FormattingDraft {
    pub(crate) first_row_is_header: bool,
    pub(crate) skip_rows: usize,
    pub(crate) delimiter: u8,
}

/// Reinterpret a typed objc reference as an untyped `AnyObject`, for the AppKit
/// target/action APIs that take `&AnyObject`.
///
/// # Safety
/// `value` must point to a live objc object for the duration of the borrow.
pub(crate) unsafe fn any_ref<T: ?Sized + Message>(value: &T) -> &AnyObject {
    unsafe { &*(value as *const T as *const AnyObject) }
}
