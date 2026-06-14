use super::*;
use objc2::ffi::NSInteger;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSBezelStyle, NSBorderType, NSBox, NSBoxType,
    NSButton, NSCellImagePosition, NSColor, NSControl, NSEvent, NSFont, NSImage, NSImageScaling,
    NSPopUpButton, NSProgressIndicator, NSProgressIndicatorStyle, NSScrollView, NSTableColumn,
    NSTableView, NSTableViewColumnAutoresizingStyle, NSTableViewGridLineStyle,
    NSTableViewSelectionHighlightStyle, NSTextAlignment, NSTextField, NSTextFieldCell, NSView,
    NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString, NSUserDefaults};
use std::cell::OnceCell;

// Window chrome and the table view: constructing the AppKit view tree
// (toolbar, table, status bar, saving overlay), laying it out, building the
// columns, and styling cells.

#[derive(Clone, Copy)]
pub(crate) enum ToolbarIcon {
    Formatting,
    SortFilter,
    Undo,
    Redo,
    Copy,
    Find,
    AddRow,
    AddColumn,
    DeleteSelection,
    MoveRowUp,
    MoveRowDown,
    MoveColumnLeft,
    MoveColumnRight,
    Save,
}

impl ToolbarIcon {
    pub(crate) fn symbols(self) -> &'static [&'static str] {
        match self {
            Self::Formatting => &["tablecells", "table"],
            Self::SortFilter => &[
                "line.3.horizontal.decrease.circle",
                "line.3.horizontal.decrease",
            ],
            Self::Undo => &["arrow.uturn.backward", "gobackward"],
            Self::Redo => &["arrow.uturn.forward", "goforward"],
            Self::Copy => &["doc.on.doc", "doc"],
            Self::Find => &["magnifyingglass"],
            Self::AddRow => &["plus.rectangle.on.rectangle", "plus.rectangle", "plus"],
            Self::AddColumn => &["plus.square.on.square", "plus.square", "plus"],
            Self::DeleteSelection => &["xmark.square", "trash", "minus.square"],
            Self::MoveRowUp => &["arrow.up.to.line.compact", "arrow.up"],
            Self::MoveRowDown => &["arrow.down.to.line.compact", "arrow.down"],
            Self::MoveColumnLeft => &["arrow.left.to.line.compact", "arrow.left"],
            Self::MoveColumnRight => &["arrow.right.to.line.compact", "arrow.right"],
            Self::Save => &["square.and.arrow.down", "tray.and.arrow.down"],
        }
    }

    pub(crate) fn accessibility_label(self) -> &'static str {
        match self {
            Self::Formatting => "table formatting",
            Self::SortFilter => "sort and filter",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Copy => "copy",
            Self::Find => "find",
            Self::AddRow => "add row",
            Self::AddColumn => "add column",
            Self::DeleteSelection => "delete selection",
            Self::MoveRowUp => "move row up",
            Self::MoveRowDown => "move row down",
            Self::MoveColumnLeft => "move column left",
            Self::MoveColumnRight => "move column right",
            Self::Save => "save",
        }
    }
}

#[derive(Default)]
pub(crate) struct TableIvars {
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
    pub(crate) struct EditableTableView;

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
    pub(crate) fn init_with_frame(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TableIvars::default());
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    pub(crate) fn set_owner(&self, owner: &Delegate) {
        self.ivars().owner.set(owner as *const Delegate).ok();
    }

    pub(crate) fn owner(&self) -> Option<&Delegate> {
        self.ivars()
            .owner
            .get()
            .and_then(|owner| unsafe { owner.as_ref() })
    }
}

impl Delegate {
    pub(crate) fn show_window(&self, mtm: MainThreadMarker) {
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
        window.setRestorable(false);
        window.disableSnapshotRestoration();
        window.setTitle(&NSString::from_str(&self.ivars().state.borrow().title()));
        window.setContentMinSize(NSSize::new(520.0, 520.0));

        let content = window
            .contentView()
            .expect("window must have a content view");
        let toolbar = self.make_toolbar(mtm);
        let status_bar = self.make_status_bar(mtm);
        let status_separator = self.make_status_separator(mtm);
        let status = self.make_status(mtm);
        let selection_status = self.make_selection_status(mtm);
        let scroll = self.make_table_area(mtm);
        let saving_overlay = self.make_saving_overlay(mtm);
        content.addSubview(&toolbar);
        content.addSubview(&scroll);
        content.addSubview(&status_bar);
        content.addSubview(&status_separator);
        content.addSubview(&status);
        content.addSubview(&selection_status);
        content.addSubview(&saving_overlay);

        window.center();
        window.setDelegate(Some(ProtocolObject::from_ref(self)));
        window.makeKeyAndOrderFront(None);

        self.ivars().window.set(window).ok();
        self.layout_main_views();
        self.rebuild_columns();
        self.refresh_table();
    }

    pub(crate) fn make_toolbar(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let toolbar = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 696.0),
                NSSize::new(1120.0, TOOLBAR_HEIGHT),
            ),
        );
        toolbar.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );

        let target = unsafe { any_ref(self) };
        let controls = [
            (
                ToolbarIcon::Formatting,
                "Format",
                sel!(openFormatting:),
                12.0,
                112.0,
            ),
            (
                ToolbarIcon::SortFilter,
                "Sort/Filter",
                sel!(openSortFilter:),
                128.0,
                108.0,
            ),
            (ToolbarIcon::Undo, "Undo", sel!(undoChange:), 248.0, 72.0),
            (ToolbarIcon::Redo, "Redo", sel!(redoChange:), 324.0, 72.0),
            (ToolbarIcon::Copy, "Copy", sel!(copySelection:), 408.0, 72.0),
            (ToolbarIcon::Find, "Find", sel!(openFind:), 484.0, 72.0),
            (ToolbarIcon::AddRow, "Add Row", sel!(addRow:), 560.0, 82.0),
            (
                ToolbarIcon::AddColumn,
                "Add Col",
                sel!(addColumn:),
                646.0,
                82.0,
            ),
            (
                ToolbarIcon::DeleteSelection,
                "Delete",
                sel!(deleteSelection:),
                732.0,
                82.0,
            ),
            (ToolbarIcon::MoveRowUp, "Row Up", sel!(rowUp:), 826.0, 80.0),
            (
                ToolbarIcon::MoveRowDown,
                "Row Down",
                sel!(rowDown:),
                910.0,
                94.0,
            ),
            (
                ToolbarIcon::MoveColumnLeft,
                "Col Left",
                sel!(columnLeft:),
                1016.0,
                84.0,
            ),
            (
                ToolbarIcon::MoveColumnRight,
                "Col Right",
                sel!(columnRight:),
                1104.0,
                96.0,
            ),
            (ToolbarIcon::Save, "Save", sel!(saveDocument:), 1210.0, 62.0),
        ];
        for (icon, title, action, x, width) in controls {
            toolbar.addSubview(&toolbar_button(
                icon, title, target, action, mtm, x, 8.0, width,
            ));
        }

        self.ivars().toolbar.set(toolbar.clone()).ok();
        toolbar
    }

    pub(crate) fn make_status_bar(&self, mtm: MainThreadMarker) -> Retained<NSBox> {
        let bar = NSBox::initWithFrame(
            NSBox::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1120.0, STATUS_HEIGHT)),
        );
        bar.setBoxType(NSBoxType::Custom);
        bar.setTitle(&NSString::from_str(""));
        bar.setTransparent(false);
        bar.setBorderWidth(0.0);
        bar.setFillColor(&NSColor::windowBackgroundColor());
        bar.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );

        self.ivars().status_bar.set(bar.clone()).ok();
        bar
    }

    pub(crate) fn make_status_separator(&self, mtm: MainThreadMarker) -> Retained<NSBox> {
        let separator = NSBox::initWithFrame(
            NSBox::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, STATUS_HEIGHT - STATUS_SEPARATOR_HEIGHT),
                NSSize::new(1120.0, STATUS_SEPARATOR_HEIGHT),
            ),
        );
        separator.setBoxType(NSBoxType::Custom);
        separator.setTitle(&NSString::from_str(""));
        separator.setTransparent(false);
        separator.setBorderWidth(0.0);
        separator.setFillColor(&NSColor::separatorColor().colorWithAlphaComponent(0.14));
        separator.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        self.ivars().status_separator.set(separator.clone()).ok();
        separator
    }

    pub(crate) fn make_status(&self, mtm: MainThreadMarker) -> Retained<NSTextField> {
        let status = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        status.setFrame(NSRect::new(
            NSPoint::new(STATUS_SIDE_PADDING, status_label_y()),
            NSSize::new(1096.0, STATUS_LABEL_HEIGHT),
        ));
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        status.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        status.setAlignment(NSTextAlignment::Left);
        status.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        self.ivars().status.set(status.clone()).ok();
        status
    }

    pub(crate) fn make_selection_status(&self, mtm: MainThreadMarker) -> Retained<NSPopUpButton> {
        let status = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            NSRect::new(
                NSPoint::new(12.0, status_control_y()),
                NSSize::new(160.0, STATUS_CONTROL_HEIGHT),
            ),
            false,
        );
        status.setBezelStyle(NSBezelStyle::Toolbar);
        status.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        status.setHidden(true);
        unsafe {
            status.setTarget(Some(any_ref(self)));
            status.setAction(Some(sel!(selectionMetricChanged:)));
        }
        status.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        self.ivars().selection_status.set(status.clone()).ok();
        status
    }

    pub(crate) fn make_table_area(&self, mtm: MainThreadMarker) -> Retained<NSScrollView> {
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
        self.ivars().scroll.set(scroll.clone()).ok();
        scroll
    }

    pub(crate) fn make_saving_overlay(&self, mtm: MainThreadMarker) -> Retained<NSBox> {
        let overlay = NSBox::initWithFrame(
            NSBox::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, STATUS_HEIGHT), NSSize::new(1120.0, 640.0)),
        );
        overlay.setBoxType(NSBoxType::Custom);
        overlay.setTitle(&NSString::from_str(""));
        overlay.setTransparent(false);
        overlay.setBorderWidth(0.0);
        overlay.setFillColor(&NSColor::windowBackgroundColor().colorWithAlphaComponent(0.82));
        overlay.setHidden(true);
        overlay.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );

        let progress = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(32.0, 32.0)),
        );
        progress.setStyle(NSProgressIndicatorStyle::Spinning);
        progress.setIndeterminate(true);
        progress.setDisplayedWhenStopped(false);
        progress.sizeToFit();

        let label = NSTextField::labelWithString(&NSString::from_str("Saving..."), mtm);
        label.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        label.setAlignment(NSTextAlignment::Center);

        overlay.addSubview(&progress);
        overlay.addSubview(&label);
        self.ivars().saving_progress.set(progress).ok();
        self.ivars().saving_label.set(label).ok();
        self.ivars().saving_overlay.set(overlay.clone()).ok();
        overlay
    }

    pub(crate) fn rebuild_columns(&self) {
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

    pub(crate) fn refresh_table(&self) {
        self.recompute_find_matches(false);
        if let Some(table) = self.ivars().table.get() {
            table.reloadData();
        }
        self.update_status();
        self.restore_selection();
    }

    pub(crate) fn update_status(&self) {
        if let Some(status) = self.ivars().status.get() {
            status.setStringValue(&NSString::from_str(
                &self.ivars().state.borrow().status_text(),
            ));
        }
        self.update_selection_stats_button();
        self.layout_status_labels();
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

    pub(crate) fn update_selection_stats_button(&self) {
        let Some(selection_status) = self.ivars().selection_status.get() else {
            return;
        };
        let stats = self.ivars().state.borrow().selection_stats();
        selection_status.removeAllItems();
        if stats.is_empty() {
            selection_status.setHidden(true);
            return;
        }

        for stat in &stats {
            selection_status.addItemWithTitle(&NSString::from_str(&stat.display_text()));
        }

        let preferred = *self.ivars().selected_selection_metric.borrow();
        let selected_index = stats
            .iter()
            .position(|stat| stat.metric == preferred)
            .unwrap_or(0);
        self.ivars()
            .selected_selection_metric
            .replace(stats[selected_index].metric);
        selection_status.selectItemAtIndex(selected_index as NSInteger);
    }

    pub(crate) fn layout_main_views(&self) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let Some(content) = window.contentView() else {
            return;
        };
        let frame = content.frame();
        let width = frame.size.width.max(0.0);
        let height = frame.size.height.max(0.0);
        let toolbar_height = self.layout_toolbar(width);

        if let Some(toolbar) = self.ivars().toolbar.get() {
            toolbar.setFrame(NSRect::new(
                NSPoint::new(0.0, (height - toolbar_height).max(0.0)),
                NSSize::new(width, toolbar_height),
            ));
        }

        if let Some(scroll) = self.ivars().scroll.get() {
            scroll.setFrame(NSRect::new(
                NSPoint::new(0.0, STATUS_HEIGHT),
                NSSize::new(width, (height - STATUS_HEIGHT - toolbar_height).max(0.0)),
            ));
        }

        if let Some(overlay) = self.ivars().saving_overlay.get() {
            let overlay_height = (height - STATUS_HEIGHT - toolbar_height).max(0.0);
            overlay.setFrame(NSRect::new(
                NSPoint::new(0.0, STATUS_HEIGHT),
                NSSize::new(width, overlay_height),
            ));
            self.layout_saving_overlay(width, overlay_height);
        }

        if let Some(status_bar) = self.ivars().status_bar.get() {
            status_bar.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width, STATUS_HEIGHT),
            ));
        }

        if let Some(separator) = self.ivars().status_separator.get() {
            separator.setFrame(NSRect::new(
                NSPoint::new(0.0, STATUS_HEIGHT - STATUS_SEPARATOR_HEIGHT),
                NSSize::new(width, STATUS_SEPARATOR_HEIGHT),
            ));
        }

        self.layout_status_labels();
    }

    pub(crate) fn layout_saving_overlay(&self, width: f64, height: f64) {
        let center_x = width / 2.0;
        let center_y = height / 2.0;
        if let Some(progress) = self.ivars().saving_progress.get() {
            let size = progress.frame().size;
            progress.setFrame(NSRect::new(
                NSPoint::new(center_x - size.width / 2.0, center_y + 8.0),
                size,
            ));
        }
        if let Some(label) = self.ivars().saving_label.get() {
            let label_width = 160.0_f64.min(width.max(0.0));
            label.setFrame(NSRect::new(
                NSPoint::new(center_x - label_width / 2.0, center_y - 26.0),
                NSSize::new(label_width, 20.0),
            ));
        }
    }

    pub(crate) fn layout_toolbar(&self, width: f64) -> f64 {
        let Some(toolbar) = self.ivars().toolbar.get() else {
            return TOOLBAR_HEIGHT;
        };
        let rows = toolbar_row_count(toolbar, width);
        let height = toolbar_height_for_rows(rows);
        layout_toolbar_buttons(toolbar, width, height);
        height
    }

    pub(crate) fn layout_status_labels(&self) {
        let Some(status_bar) = self.ivars().status_bar.get() else {
            return;
        };
        let width = status_bar.frame().size.width.max(0.0);
        let available = (width - STATUS_SIDE_PADDING * 2.0).max(0.0);
        let stats_text = self
            .ivars()
            .selection_status
            .get()
            .and_then(|status| status.titleOfSelectedItem())
            .map(|title| title.to_string())
            .unwrap_or_default();
        let stats_width = if stats_text.is_empty() {
            0.0
        } else if available < 116.0 {
            available
        } else {
            estimated_status_text_width(&stats_text).clamp(116.0, available)
        };

        if let Some(selection_status) = self.ivars().selection_status.get() {
            selection_status.setHidden(stats_text.is_empty());
            selection_status.setFrame(NSRect::new(
                NSPoint::new(
                    STATUS_SIDE_PADDING + available - stats_width,
                    status_control_y(),
                ),
                NSSize::new(stats_width, STATUS_CONTROL_HEIGHT),
            ));
        }

        if let Some(status) = self.ivars().status.get() {
            let left_width = if stats_text.is_empty() {
                available
            } else {
                (available - stats_width - STATUS_LABEL_GAP).max(0.0)
            };
            status.setHidden(left_width < 32.0);
            status.setFrame(NSRect::new(
                NSPoint::new(STATUS_SIDE_PADDING, status_label_y()),
                NSSize::new(left_width, STATUS_LABEL_HEIGHT),
            ));
        }
    }

    pub(crate) fn restore_selection(&self) {
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

    pub(crate) fn update_saving_ui(&self) {
        let saving = self.is_saving();
        if let Some(overlay) = self.ivars().saving_overlay.get() {
            overlay.setHidden(!saving);
        }
        if let Some(progress) = self.ivars().saving_progress.get() {
            unsafe {
                if saving {
                    progress.startAnimation(None);
                } else {
                    progress.stopAnimation(None);
                }
            }
        }
        if let Some(toolbar) = self.ivars().toolbar.get() {
            let subviews = toolbar.subviews();
            for idx in 0..subviews.count() {
                if let Some(control) = subviews.objectAtIndex(idx).downcast_ref::<NSControl>() {
                    control.setEnabled(!saving);
                }
            }
        }
        if let Some(table) = self.ivars().table.get() {
            table.setAllowsColumnReordering(!saving);
        }
    }
}

pub(crate) fn apply_table_cell_style(
    cell: &NSTextFieldCell,
    visible_column: Option<VisibleColumn>,
    selected: bool,
    active: bool,
    find_match: bool,
    find_active: bool,
) {
    cell.setFont(Some(&table_data_font()));

    if find_active {
        cell.setDrawsBackground(true);
        cell.setBackgroundColor(Some(&find_active_background()));
        cell.setTextColor(Some(&NSColor::labelColor()));
    } else if selected {
        cell.setDrawsBackground(true);
        cell.setBackgroundColor(Some(&selection_background(active)));
        cell.setTextColor(Some(&NSColor::labelColor()));
    } else if find_match {
        cell.setDrawsBackground(true);
        cell.setBackgroundColor(Some(&find_match_background()));
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

pub(crate) fn table_data_font() -> Retained<NSFont> {
    NSFont::userFixedPitchFontOfSize(13.0).unwrap_or_else(|| NSFont::systemFontOfSize(13.0))
}

pub(crate) fn selection_background(active: bool) -> Retained<NSColor> {
    user_accent_color().colorWithAlphaComponent(if active { 0.34 } else { 0.22 })
}

pub(crate) fn find_match_background() -> Retained<NSColor> {
    NSColor::systemYellowColor().colorWithAlphaComponent(0.28)
}

pub(crate) fn find_active_background() -> Retained<NSColor> {
    NSColor::systemYellowColor().colorWithAlphaComponent(0.62)
}

pub(crate) fn user_accent_color() -> Retained<NSColor> {
    let defaults = NSUserDefaults::standardUserDefaults();
    let accent_key = NSString::from_str(APPLE_ACCENT_COLOR_KEY);
    match defaults.objectForKey(&accent_key) {
        None => NSColor::systemBlueColor(),
        Some(_) if defaults.integerForKey(&accent_key) == -1 => NSColor::systemBlueColor(),
        Some(_) => NSColor::controlAccentColor(),
    }
}

pub(crate) fn disable_window_restoration() {
    NSUserDefaults::standardUserDefaults()
        .setBool_forKey(false, &NSString::from_str("NSQuitAlwaysKeepsWindows"));
}

pub(crate) fn visible_column_from_table_column(
    table_column: &NSTableColumn,
) -> Option<VisibleColumn> {
    let identifier = table_column.identifier().to_string();
    if identifier == ROW_NUMBER_COLUMN {
        return Some(VisibleColumn::RowNumber);
    }
    identifier
        .strip_prefix("c:")
        .and_then(|value| value.parse::<usize>().ok())
        .map(VisibleColumn::Data)
}

pub(crate) fn toolbar_button(
    icon: ToolbarIcon,
    title: &str,
    target: &AnyObject,
    action: objc2::runtime::Sel,
    mtm: MainThreadMarker,
    x: f64,
    y: f64,
    width: f64,
) -> Retained<NSButton> {
    let button = button(title, target, action, mtm, x, y, width);
    button.setFrame(NSRect::new(
        NSPoint::new(x, y),
        NSSize::new(width, TOOLBAR_BUTTON_HEIGHT),
    ));
    button.setBezelStyle(NSBezelStyle::Toolbar);
    button.setFont(Some(&NSFont::systemFontOfSize(12.0)));
    button.setImagePosition(NSCellImagePosition::ImageLeading);
    button.setImageScaling(NSImageScaling::ScaleProportionallyDown);
    button.setImageHugsTitle(true);
    button.setToolTip(Some(&NSString::from_str(icon.accessibility_label())));

    let description = NSString::from_str(icon.accessibility_label());
    for symbol in icon.symbols() {
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            Some(&description),
        ) {
            image.setTemplate(true);
            button.setImage(Some(&image));
            break;
        }
    }

    button
}

pub(crate) fn toolbar_row_count(toolbar: &NSView, width: f64) -> usize {
    let subviews = toolbar.subviews();
    if subviews.count() == 0 {
        return 1;
    }

    let mut rows = 1;
    let mut x = TOOLBAR_HORIZONTAL_PADDING;
    let max_x = (width - TOOLBAR_HORIZONTAL_PADDING).max(TOOLBAR_HORIZONTAL_PADDING);
    for idx in 0..subviews.count() {
        let view = subviews.objectAtIndex(idx);
        let view_width = view.frame().size.width;
        if idx > 0 && x + view_width > max_x {
            rows += 1;
            x = TOOLBAR_HORIZONTAL_PADDING;
        }
        x += view_width + TOOLBAR_BUTTON_GAP;
    }
    rows
}

pub(crate) fn toolbar_height_for_rows(rows: usize) -> f64 {
    TOOLBAR_VERTICAL_PADDING * 2.0
        + rows as f64 * TOOLBAR_BUTTON_HEIGHT
        + rows.saturating_sub(1) as f64 * TOOLBAR_ROW_GAP
}

pub(crate) fn layout_toolbar_buttons(toolbar: &NSView, width: f64, height: f64) {
    let subviews = toolbar.subviews();
    let mut x = TOOLBAR_HORIZONTAL_PADDING;
    let mut y = height - TOOLBAR_VERTICAL_PADDING - TOOLBAR_BUTTON_HEIGHT;
    let max_x = (width - TOOLBAR_HORIZONTAL_PADDING).max(TOOLBAR_HORIZONTAL_PADDING);
    for idx in 0..subviews.count() {
        let view = subviews.objectAtIndex(idx);
        let view_width = view.frame().size.width;
        if idx > 0 && x + view_width > max_x {
            x = TOOLBAR_HORIZONTAL_PADDING;
            y -= TOOLBAR_BUTTON_HEIGHT + TOOLBAR_ROW_GAP;
        }
        view.setFrame(NSRect::new(
            NSPoint::new(x, y.max(TOOLBAR_VERTICAL_PADDING)),
            NSSize::new(view_width, TOOLBAR_BUTTON_HEIGHT),
        ));
        x += view_width + TOOLBAR_BUTTON_GAP;
    }
}

pub(crate) fn estimated_status_text_width(text: &str) -> f64 {
    (text.chars().count() as f64 * 6.2 + 16.0).ceil()
}

pub(crate) fn status_label_y() -> f64 {
    (STATUS_HEIGHT - STATUS_LABEL_HEIGHT) / 2.0
}

pub(crate) fn status_control_y() -> f64 {
    (STATUS_HEIGHT - STATUS_CONTROL_HEIGHT) / 2.0
}
