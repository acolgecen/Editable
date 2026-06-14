use super::*;
use crate::selection::Cell;
use editable_csv_core::{FilterOperator, FilterRule, SortDirection, SortKey};
use objc2::ffi::{NSInteger, NSUInteger};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSApplicationTerminateReply, NSButton, NSControl, NSControlStateValueOn,
    NSControlTextEditingDelegate, NSModalResponseCancel,
    NSModalResponseOK, NSPopUpButton, NSTableColumn, NSTableView,
    NSTableViewDataSource, NSTableViewDelegate, NSTextField, NSTextFieldCell,
    NSTextFieldDelegate, NSTextView, NSWindow, NSWindowDelegate,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint,
    NSRect, NSSize, NSString,
};
use std::path::PathBuf;
use std::ptr;

// The application/window delegate: the single objc class that backs one
// window. `define_class!` registers every AppKit protocol method and menu/
// toolbar action; the heavy lifting lives in sibling `ui` modules.

define_class!(
    // SAFETY:
    // - NSObject has no special subclassing requirements for this delegate.
    // - Delegate does not implement Drop.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = AppDelegateIvars]
    pub(crate) struct Delegate;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: The text-editing delegate is used to turn field-editor arrow
    // commands back into table navigation while a cell is being edited.
    unsafe impl NSControlTextEditingDelegate for Delegate {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, notification: &NSNotification) {
            let Some(field) = notification
                .object()
                .and_then(|object| object.downcast::<NSTextField>().ok())
            else {
                return;
            };
            if self.find_field_matches(&field) {
                self.update_find_query_from_field(&field);
            }
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        #[allow(non_snake_case)]
        unsafe fn control_textView_doCommandBySelector(
            &self,
            control: &NSControl,
            text_view: &NSTextView,
            command_selector: Sel,
        ) -> Bool {
            if self.find_field_matches_control(control) {
                return false.into();
            }
            let Some((rows, columns, selector_extending)) =
                navigation_delta_for_selector(command_selector)
            else {
                return false.into();
            };
            let extending = selector_extending || current_event_has_shift(self.mtm());
            self.commit_text_view_edit(text_view);
            self.navigate_selection(rows, columns, extending);
            true.into()
        }
    }

    unsafe impl NSTextFieldDelegate for Delegate {}

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
            disable_window_restoration();
            self.install_main_menu(&app);

            if let Some(path) = launch_path_arg() {
                {
                    let mut state = self.ivars().state.borrow_mut();
                    if state.document.is_some() {
                        state.last_error = None;
                    } else if let Err(err) = state.open_path(path) {
                        state.last_error = Some(err.to_string());
                    }
                }
                self.show_window(mtm);
            } else if launch_is_default_launch(notification) {
                self.present_welcome_window();
            }
        }

        #[unsafe(method(applicationShouldOpenUntitledFile:))]
        fn should_open_untitled_file(&self, _app: &NSApplication) -> bool {
            false
        }

        #[unsafe(method(applicationShouldTerminate:))]
        fn application_should_terminate(
            &self,
            _sender: &NSApplication,
        ) -> NSApplicationTerminateReply {
            if window_delegate_is_visible(self)
                && self
                    .ivars()
                    .state
                    .borrow()
                    .document
                    .as_ref()
                    .is_some_and(editable_csv_core::CsvDocument::is_dirty)
            {
                match self.confirm_close_with_unsaved_changes() {
                    CloseChoice::Save => {
                        if !self.save_document_with_prompt() {
                            return NSApplicationTerminateReply::TerminateCancel;
                        }
                    }
                    CloseChoice::Discard => {}
                    CloseChoice::Cancel => {
                        return NSApplicationTerminateReply::TerminateCancel;
                    }
                }
            }

            let delegates = self.ivars().window_delegates.borrow().clone();
            for delegate in &delegates {
                if !window_delegate_is_visible(delegate) {
                    continue;
                }
                if delegate
                    .ivars()
                    .state
                    .borrow()
                    .document
                    .as_ref()
                    .is_some_and(editable_csv_core::CsvDocument::is_dirty)
                {
                    match delegate.confirm_close_with_unsaved_changes() {
                        CloseChoice::Save => {
                            if !delegate.save_document_with_prompt() {
                                return NSApplicationTerminateReply::TerminateCancel;
                            }
                        }
                        CloseChoice::Discard => {}
                        CloseChoice::Cancel => {
                            return NSApplicationTerminateReply::TerminateCancel;
                        }
                    }
                }
            }

            NSApplicationTerminateReply::TerminateNow
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window_closed(&self, _app: &NSApplication) -> bool {
            false
        }

        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn should_handle_reopen(&self, _app: &NSApplication, has_visible_windows: bool) -> Bool {
            if !has_visible_windows {
                self.present_welcome_window();
                return false.into();
            }
            true.into()
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
            if self.is_saving() {
                return;
            }
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
        ) -> Bool {
            if self.is_saving() {
                return false.into();
            }
            matches!(
                table_column.and_then(visible_column_from_table_column),
                Some(VisibleColumn::Data(_))
            )
            .into()
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

            let (find_match, find_active) = match visible_column {
                Some(VisibleColumn::Data(column)) => self.find_cell_state(Cell { row, column }),
                _ => (false, false),
            };

            apply_table_cell_style(
                text_cell,
                visible_column,
                selected,
                active,
                find_match,
                find_active,
            );
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
        fn window_will_close(&self, notification: &NSNotification) {
            if notification
                .object()
                .and_then(|object| object.downcast::<NSWindow>().ok())
                .is_some_and(|window| self.find_window_matches(&window))
            {
                self.clear_find_matches();
                self.ivars().find_panel.replace(None);
                return;
            }
            self.ivars().sort_filter_panel.replace(None);
            self.ivars().formatting_panel.replace(None);
            self.clear_find_matches();
            self.ivars().find_panel.replace(None);
        }

        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _notification: &NSNotification) {
            self.layout_main_views();
            self.recenter_find_panel();
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

        #[unsafe(method(menuCopySelection:))]
        fn menu_copy_selection(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| {
                delegate.copy_selection(sel!(copySelection:), sender)
            });
        }

        #[unsafe(method(menuFind:))]
        fn menu_find(&self, sender: &AnyObject) {
            self.with_active_window_delegate(|delegate| delegate.open_find(sel!(openFind:), sender));
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
            if self.is_saving() {
                return;
            }
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

        #[unsafe(method(formattingSeparatorChanged:))]
        fn formatting_separator_changed(&self, _sender: &AnyObject) {
            self.update_custom_delimiter_visibility();
        }

        #[unsafe(method(selectionMetricChanged:))]
        fn selection_metric_changed(&self, sender: &AnyObject) {
            let Some(popup) = sender.downcast_ref::<NSPopUpButton>() else {
                return;
            };
            let index = popup.indexOfSelectedItem();
            if index < 0 {
                return;
            }
            let stats = self.ivars().state.borrow().selection_stats();
            if let Some(stat) = stats.get(index as usize) {
                self.ivars()
                    .selected_selection_metric
                    .replace(stat.metric);
            }
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
                }
            };
            let result = {
                let mut state = self.ivars().state.borrow_mut();
                state.first_row_is_header = options.first_row_is_header;
                state.skip_rows = options.skip_rows;
                state.delimiter = options.delimiter;
                state.reopen_with_options()
            };
            if let Err(err) = result {
                {
                    let mut state = self.ivars().state.borrow_mut();
                    state.first_row_is_header = previous.first_row_is_header;
                    state.skip_rows = previous.skip_rows;
                    state.delimiter = previous.delimiter;
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
            if self.is_saving() {
                return;
            }
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
            if self.is_saving() {
                return;
            }
            if self.ivars().state.borrow_mut().undo() {
                self.rebuild_columns();
                self.refresh_table();
            } else {
                self.update_status();
            }
        }

        #[unsafe(method(redoChange:))]
        fn redo_change(&self, _sender: &AnyObject) {
            if self.is_saving() {
                return;
            }
            if self.ivars().state.borrow_mut().redo() {
                self.rebuild_columns();
                self.refresh_table();
            } else {
                self.update_status();
            }
        }

        #[unsafe(method(copySelection:))]
        fn copy_selection(&self, _sender: &AnyObject) {
            self.copy_selection_to_pasteboard();
        }

        #[unsafe(method(openFind:))]
        fn open_find(&self, _sender: &AnyObject) {
            self.present_find_panel();
        }

        #[unsafe(method(findPrevious:))]
        fn find_previous(&self, _sender: &AnyObject) {
            self.step_find_match(-1);
        }

        #[unsafe(method(findNext:))]
        fn find_next(&self, _sender: &AnyObject) {
            self.step_find_match(1);
        }

        #[unsafe(method(sortDescending:))]
        fn sort_descending(&self, _sender: &AnyObject) {
            if self.is_saving() {
                return;
            }
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
            if self.is_saving() {
                return;
            }
            let result = self.ivars().state.borrow_mut().insert_row();
            self.handle_result(result);
            self.refresh_table();
        }

        #[unsafe(method(addColumn:))]
        fn add_column(&self, _sender: &AnyObject) {
            if self.is_saving() {
                return;
            }
            let result = self.ivars().state.borrow_mut().insert_column();
            self.handle_result(result);
            self.rebuild_columns();
            self.refresh_table();
        }

        #[unsafe(method(deleteSelection:))]
        fn delete_selection(&self, _sender: &AnyObject) {
            if self.is_saving() {
                return;
            }
            let result = self.ivars().state.borrow_mut().delete_selection();
            self.handle_result(result);
            self.rebuild_columns();
            self.refresh_table();
        }

        #[unsafe(method(rowUp:))]
        fn row_up(&self, _sender: &AnyObject) {
            if self.is_saving() {
                return;
            }
            let result = self.ivars().state.borrow_mut().move_active_row(-1);
            self.handle_result(result);
            self.refresh_table();
            self.restore_selection();
        }

        #[unsafe(method(rowDown:))]
        fn row_down(&self, _sender: &AnyObject) {
            if self.is_saving() {
                return;
            }
            let result = self.ivars().state.borrow_mut().move_active_row(1);
            self.handle_result(result);
            self.refresh_table();
            self.restore_selection();
        }

        #[unsafe(method(columnLeft:))]
        fn column_left(&self, _sender: &AnyObject) {
            if self.is_saving() {
                return;
            }
            let result = self.ivars().state.borrow_mut().move_active_column(-1);
            self.handle_result(result);
            self.rebuild_columns();
            self.refresh_table();
            self.restore_selection();
        }

        #[unsafe(method(columnRight:))]
        fn column_right(&self, _sender: &AnyObject) {
            if self.is_saving() {
                return;
            }
            let result = self.ivars().state.borrow_mut().move_active_column(1);
            self.handle_result(result);
            self.rebuild_columns();
            self.refresh_table();
            self.restore_selection();
        }

        #[unsafe(method(saveDocument:))]
        fn save_document(&self, _sender: &AnyObject) {
            if self.is_saving() {
                return;
            }
            self.save_document_with_prompt();
        }

        #[unsafe(method(editClickedCell:))]
        fn edit_clicked_cell(&self, _sender: &AnyObject) {
            if self.is_saving() {
                return;
            }
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
                    .select_cell(row as usize, column);
                table.editColumn_row_withEvent_select(column as NSInteger, row, None, true);
            }
        }

        #[unsafe(method(editSelectedCell:))]
        fn edit_selected_cell(&self, _sender: &AnyObject) {
            if self.is_saving() {
                return;
            }
            self.edit_top_left_selected_cell(None);
        }
    }
);

impl Delegate {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AppDelegateIvars::default());
        unsafe { msg_send![super(this), init] }
    }

    pub(crate) fn with_active_window_delegate(&self, action: impl FnOnce(&Delegate)) -> bool {
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

    pub(crate) fn is_saving(&self) -> bool {
        *self.ivars().is_saving.borrow()
    }

    pub(crate) fn set_saving(&self, saving: bool) {
        self.ivars().is_saving.replace(saving);
        self.update_saving_ui();
        if let Some(window) = self.ivars().window.get() {
            window.displayIfNeeded();
        }
    }

    pub(crate) fn handle_result(&self, result: editable_csv_core::Result<()>) {
        if let Err(err) = result {
            self.ivars().state.borrow_mut().last_error = Some(err.to_string());
        } else {
            self.ivars().state.borrow_mut().last_error = None;
        }
    }

}
