#![deny(unsafe_op_in_unsafe_fn)]

mod app_state;
mod selection;

use app_state::EditableState;
use editable_csv_core::SortDirection;
use objc2::ffi::{NSInteger, NSUInteger};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly, Message};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSAutoresizingMaskOptions,
    NSBackingStoreType, NSBorderType, NSButton, NSButtonType, NSColor, NSControlStateValueOff,
    NSControlStateValueOn, NSControlTextEditingDelegate, NSEvent, NSEventModifierFlags, NSFont,
    NSModalResponseOK, NSOpenPanel, NSScrollView, NSTableColumn, NSTableView,
    NSTableViewColumnAutoresizingStyle, NSTableViewDataSource, NSTableViewDelegate,
    NSTableViewGridLineStyle, NSTableViewSelectionHighlightStyle, NSTextAlignment, NSTextField,
    NSTextFieldCell, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
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

#[derive(Default)]
struct AppDelegateIvars {
    window: OnceCell<Retained<NSWindow>>,
    table: OnceCell<Retained<EditableTableView>>,
    status: OnceCell<Retained<NSTextField>>,
    header_checkbox: OnceCell<Retained<NSButton>>,
    skip_field: OnceCell<Retained<NSTextField>>,
    filter_field: OnceCell<Retained<NSTextField>>,
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

            {
                let mut state = self.ivars().state.borrow_mut();
                if state.document.is_some() {
                    state.last_error = None;
                } else if let Some(path) = launch_path_arg() {
                    if let Err(err) = state.open_path(path) {
                        state.last_error = Some(err.to_string());
                    }
                } else if let Some(path) = choose_startup_file(mtm) {
                    if let Err(err) = state.open_path(path) {
                        state.last_error = Some(err.to_string());
                    }
                } else {
                    app.terminate(None);
                    return;
                }
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

            let content = window.contentView().expect("window must have a content view");
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

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window_closed(&self, _app: &NSApplication) -> bool {
            true
        }

        #[unsafe(method(application:openFile:))]
        fn open_file(&self, _sender: &NSApplication, filename: &NSString) -> Bool {
            let result = self
                .ivars()
                .state
                .borrow_mut()
                .open_path(PathBuf::from(filename.to_string()));
            let ok = result.is_ok();
            self.handle_result(result);
            self.rebuild_columns();
            self.refresh_table();
            ok.into()
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
                .document
                .as_mut()
                .map(|doc| doc.reorder_column(column_index as usize - 1, new_column_index as usize - 1));
            if let Some(result) = result {
                self.handle_result(result);
            }
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

            if selected {
                text_cell.setDrawsBackground(true);
                text_cell.setBackgroundColor(Some(&NSColor::selectedTextBackgroundColor()));
                text_cell.setTextColor(Some(&NSColor::alternateSelectedControlTextColor()));
            } else if matches!(visible_column, Some(VisibleColumn::RowNumber)) {
                text_cell.setDrawsBackground(true);
                text_cell.setBackgroundColor(Some(&NSColor::controlBackgroundColor()));
                text_cell.setTextColor(Some(&NSColor::secondaryLabelColor()));
            } else {
                text_cell.setDrawsBackground(false);
                text_cell.setTextColor(Some(&NSColor::textColor()));
            }
        }
    }

    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }
    }

    impl Delegate {
        #[unsafe(method(toggleHeader:))]
        fn toggle_header(&self, sender: &AnyObject) {
            let checked = sender
                .downcast_ref::<NSButton>()
                .map(|button| button.state() == NSControlStateValueOn)
                .unwrap_or(true);
            let result = {
                let mut state = self.ivars().state.borrow_mut();
                state.first_row_is_header = checked;
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
            let result = self.ivars().state.borrow_mut().save(None);
            self.handle_result(result);
            self.update_status();
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
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AppDelegateIvars::default());
        unsafe { msg_send![super(this), init] }
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
        let header = button("Header", target, sel!(toggleHeader:), mtm, 16.0, 10.0, 86.0);
        header.setButtonType(NSButtonType::Switch);
        header.setState(if self.ivars().state.borrow().first_row_is_header {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        toolbar.addSubview(&header);
        self.ivars().header_checkbox.set(header).ok();

        let skip_label = NSTextField::labelWithString(&NSString::from_str("Skip rows"), mtm);
        skip_label.setFrame(NSRect::new(
            NSPoint::new(110.0, 13.0),
            NSSize::new(62.0, 18.0),
        ));
        skip_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        skip_label.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        toolbar.addSubview(&skip_label);

        let skip = NSTextField::textFieldWithString(
            &NSString::from_str(&self.ivars().state.borrow().skip_rows.to_string()),
            mtm,
        );
        skip.setFrame(NSRect::new(
            NSPoint::new(178.0, 10.0),
            NSSize::new(58.0, 24.0),
        ));
        skip.setPlaceholderString(Some(&NSString::from_str("Skip")));
        unsafe {
            skip.setTarget(Some(target));
            skip.setAction(Some(sel!(applySkipRows:)));
        }
        toolbar.addSubview(&skip);
        self.ivars().skip_field.set(skip).ok();

        let filter = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
        filter.setFrame(NSRect::new(
            NSPoint::new(250.0, 10.0),
            NSSize::new(170.0, 24.0),
        ));
        filter.setPlaceholderString(Some(&NSString::from_str("Filter active column")));
        unsafe {
            filter.setTarget(Some(target));
            filter.setAction(Some(sel!(applyFilter:)));
        }
        toolbar.addSubview(&filter);
        self.ivars().filter_field.set(filter).ok();

        let controls = [
            ("A-Z", sel!(sortAscending:), 430.0, 46.0),
            ("Z-A", sel!(sortDescending:), 480.0, 46.0),
            ("+ Row", sel!(addRow:), 532.0, 56.0),
            ("+ Col", sel!(addColumn:), 592.0, 54.0),
            ("Delete", sel!(deleteSelection:), 650.0, 60.0),
            ("Row Up", sel!(rowUp:), 718.0, 62.0),
            ("Row Down", sel!(rowDown:), 784.0, 76.0),
            ("Col Left", sel!(columnLeft:), 866.0, 70.0),
            ("Col Right", sel!(columnRight:), 944.0, 76.0),
            ("Save", sel!(saveDocument:), 1028.0, 54.0),
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
        scroll.setHasVerticalScroller(true);
        scroll.setHasHorizontalScroller(true);
        scroll.setAutohidesScrollers(false);
        scroll.setDocumentView(Some(&table));

        self.ivars().table.set(table).ok();
        scroll
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
            window.setTitle(&NSString::from_str(&self.ivars().state.borrow().title()));
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

    fn handle_result(&self, result: editable_csv_core::Result<()>) {
        if let Err(err) = result {
            self.ivars().state.borrow_mut().last_error = Some(err.to_string());
        } else {
            self.ivars().state.borrow_mut().last_error = None;
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
