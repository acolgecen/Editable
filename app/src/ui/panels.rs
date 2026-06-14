use super::*;
use crate::selection::Cell;
use editable_csv_core::{FilterOperator, FilterRule, SortDirection, SortKey};
use objc2::ffi::NSInteger;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSButtonType,
    NSColor, NSControl, NSControlStateValueOff, NSControlStateValueOn, NSFont, NSPopUpButton, NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    NSPoint,
    NSRect, NSSize, NSString,
};
use std::ptr;

// The auxiliary panels: Find, Formatting, and Sort & Filter. Each owns its
// control widgets, builds its window, and collects edits back into the state.

pub(crate) struct SortRuleControls {
    column: Retained<NSPopUpButton>,
    direction: Retained<NSPopUpButton>,
}

pub(crate) struct FilterRuleControls {
    column: Retained<NSPopUpButton>,
    operator: Retained<NSPopUpButton>,
    value: Retained<NSTextField>,
}

pub(crate) struct SortFilterPanel {
    window: Retained<NSWindow>,
    sort_rows: Vec<SortRuleControls>,
    filter_rows: Vec<FilterRuleControls>,
    pub(crate) error_label: Retained<NSTextField>,
}

pub(crate) struct FormattingPanel {
    header: Retained<NSButton>,
    skip_rows: Retained<NSTextField>,
    delimiter: Retained<NSPopUpButton>,
    custom_delimiter: Retained<NSTextField>,
    pub(crate) error_label: Retained<NSTextField>,
}

pub(crate) struct FindPanel {
    window: Retained<NSWindow>,
    field: Retained<NSTextField>,
    status: Retained<NSTextField>,
    previous: Retained<NSButton>,
    next: Retained<NSButton>,
}

impl Delegate {
    pub(crate) fn present_find_panel(&self) {
        if self.ivars().find_panel.borrow().is_none() {
            let panel = self.build_find_panel();
            self.ivars().find_panel.replace(Some(panel));
        }

        let Some(panel) = self.ivars().find_panel.borrow().as_ref().map(|panel| {
            (
                panel.window.clone(),
                panel.field.clone(),
                panel.previous.clone(),
                panel.next.clone(),
            )
        }) else {
            return;
        };
        let (window, field, previous, next) = panel;
        self.position_find_window(&window);
        previous.setEnabled(!self.ivars().find_matches.borrow().is_empty());
        next.setEnabled(!self.ivars().find_matches.borrow().is_empty());
        window.makeKeyAndOrderFront(None);
        unsafe {
            let _: bool = msg_send![&*window, makeFirstResponder: &*field];
            field.selectText(None);
        }
        self.recompute_find_matches(true);
    }

    pub(crate) fn build_find_panel(&self) -> FindPanel {
        let mtm = self.mtm();
        let target = unsafe { any_ref(self) };
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(FIND_WINDOW_WIDTH, FIND_WINDOW_HEIGHT),
                ),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("Find"));
        window.setRestorable(false);
        window.setDelegate(Some(ProtocolObject::from_ref(self)));

        let content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(FIND_WINDOW_WIDTH, FIND_WINDOW_HEIGHT),
            ),
        );

        let field = NSTextField::textFieldWithString(
            &NSString::from_str(&self.ivars().find_query.borrow()),
            mtm,
        );
        field.setFrame(NSRect::new(
            NSPoint::new(18.0, 38.0),
            NSSize::new(378.0, 28.0),
        ));
        field.setPlaceholderString(Some(&NSString::from_str("Find")));
        field.setFont(Some(&NSFont::systemFontOfSize(14.0)));
        unsafe {
            field.setDelegate(Some(ProtocolObject::from_ref(self)));
            field.setTarget(Some(target));
            field.setAction(Some(sel!(findNext:)));
        }

        let status = NSTextField::labelWithString(&NSString::from_str("No query"), mtm);
        status.setFrame(NSRect::new(
            NSPoint::new(20.0, 14.0),
            NSSize::new(376.0, 20.0),
        ));
        status.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));

        let previous = button("<", target, sel!(findPrevious:), mtm, 412.0, 41.0, 46.0);
        previous.setToolTip(Some(&NSString::from_str("Previous match")));
        let next = button(">", target, sel!(findNext:), mtm, 468.0, 41.0, 46.0);
        next.setToolTip(Some(&NSString::from_str("Next match")));
        previous.setEnabled(false);
        next.setEnabled(false);

        content.addSubview(&field);
        content.addSubview(&status);
        content.addSubview(&previous);
        content.addSubview(&next);
        window.setContentView(Some(&content));

        FindPanel {
            window,
            field,
            status,
            previous,
            next,
        }
    }

    pub(crate) fn position_find_window(&self, find_window: &NSWindow) {
        let Some(window) = self.ivars().window.get() else {
            find_window.center();
            return;
        };
        let parent = window.frame();
        let origin = NSPoint::new(
            parent.origin.x + (parent.size.width - FIND_WINDOW_WIDTH) / 2.0,
            parent.origin.y + (parent.size.height - FIND_WINDOW_HEIGHT) / 2.0,
        );
        find_window.setFrame_display(
            NSRect::new(origin, NSSize::new(FIND_WINDOW_WIDTH, FIND_WINDOW_HEIGHT)),
            true,
        );
    }

    pub(crate) fn recenter_find_panel(&self) {
        if let Some(window) = self
            .ivars()
            .find_panel
            .borrow()
            .as_ref()
            .map(|panel| panel.window.clone())
        {
            self.position_find_window(&window);
        }
    }

    pub(crate) fn present_formatting_panel(&self) {
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

    pub(crate) fn build_formatting_panel(&self, window: Retained<NSWindow>, error: &str) -> FormattingPanel {
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

        content.addSubview(&secondary_label("Skip rows", mtm, 24.0, 214.0, 110.0));
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
        content.addSubview(&secondary_label("CSV separator", mtm, 24.0, 136.0, 110.0));
        let delimiter = popup(
            &delimiter_titles(),
            delimiter_index(state.delimiter),
            mtm,
            150.0,
            132.0,
            132.0,
        );
        unsafe {
            delimiter.setTarget(Some(target));
            delimiter.setAction(Some(sel!(formattingSeparatorChanged:)));
        }
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
        custom_delimiter.setHidden(delimiter_index(state.delimiter) != 5);
        content.addSubview(&custom_delimiter);

        let error_label = secondary_label(error, mtm, 24.0, 58.0, 280.0);
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
            error_label,
        }
    }

    pub(crate) fn collect_formatting_draft(&self) -> Option<FormattingDraft> {
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
        Some(FormattingDraft {
            first_row_is_header: panel.header.state() == NSControlStateValueOn,
            skip_rows,
            delimiter,
        })
    }

    pub(crate) fn update_custom_delimiter_visibility(&self) {
        if let Some(panel) = self.ivars().formatting_panel.borrow().as_ref() {
            panel
                .custom_delimiter
                .setHidden(panel.delimiter.indexOfSelectedItem() != 5);
        }
    }

    pub(crate) fn present_sort_filter_panel(&self) {
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

    pub(crate) fn rebuild_sort_filter_panel(
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

    pub(crate) fn build_sort_filter_panel(
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
            content.addSubview(&secondary_label("No sort rules", mtm, 24.0, y + 4.0, 220.0));
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
            content.addSubview(&secondary_label("No filter rules", mtm, 24.0, y + 4.0, 220.0));
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

        let error_label = secondary_label(error, mtm, 24.0, 58.0, 450.0);
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
            content.addSubview(&secondary_label(
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

    pub(crate) fn collect_sort_filter_draft(&self) -> (Vec<SortKey>, Vec<FilterRule>) {
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

    pub(crate) fn column_titles(&self) -> Vec<String> {
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

    pub(crate) fn update_find_query_from_field(&self, field: &NSTextField) {
        self.ivars()
            .find_query
            .replace(field.stringValue().to_string());
        self.recompute_find_matches(true);
    }

    pub(crate) fn recompute_find_matches(&self, activate: bool) {
        if self.ivars().find_panel.borrow().is_none() {
            return;
        }

        let query = self.ivars().find_query.borrow().clone();
        let matches = self.ivars().state.borrow().find_matches(&query);
        let previous_active = self
            .ivars()
            .find_index
            .borrow()
            .and_then(|index| self.ivars().find_matches.borrow().get(index).copied());
        let next_index = previous_active
            .and_then(|cell| matches.iter().position(|match_cell| *match_cell == cell))
            .or_else(|| (!matches.is_empty()).then_some(0));

        self.ivars().find_matches.replace(matches);
        self.ivars().find_index.replace(next_index);
        self.update_find_buttons();

        if activate {
            if next_index.is_some() {
                self.activate_current_find_match();
            } else if let Some(table) = self.ivars().table.get() {
                table.reloadData();
            }
        }
    }

    pub(crate) fn update_find_buttons(&self) {
        let enabled = !self.ivars().find_matches.borrow().is_empty();
        if let Some(panel) = self.ivars().find_panel.borrow().as_ref() {
            panel.previous.setEnabled(enabled);
            panel.next.setEnabled(enabled);
            panel
                .status
                .setStringValue(&NSString::from_str(&self.find_status_text()));
        }
    }

    pub(crate) fn clear_find_matches(&self) {
        self.ivars().find_matches.borrow_mut().clear();
        self.ivars().find_index.replace(None);
        if let Some(table) = self.ivars().table.get() {
            table.reloadData();
        }
    }

    pub(crate) fn find_status_text(&self) -> String {
        let query_is_empty = self.ivars().find_query.borrow().trim().is_empty();
        if query_is_empty {
            return "No query".to_string();
        }

        let count = self.ivars().find_matches.borrow().len();
        if count == 0 {
            return "No matches".to_string();
        }

        let current = self.ivars().find_index.borrow().unwrap_or(0) + 1;
        format!("{current} of {count} matches")
    }

    pub(crate) fn step_find_match(&self, delta: isize) {
        if self.ivars().find_panel.borrow().is_none() {
            self.present_find_panel();
            return;
        }

        if let Some(field) = self
            .ivars()
            .find_panel
            .borrow()
            .as_ref()
            .map(|panel| panel.field.clone())
        {
            self.ivars()
                .find_query
                .replace(field.stringValue().to_string());
            self.recompute_find_matches(false);
        }

        let len = self.ivars().find_matches.borrow().len();
        if len == 0 {
            self.ivars().find_index.replace(None);
            self.update_find_buttons();
            if let Some(table) = self.ivars().table.get() {
                table.reloadData();
            }
            return;
        }

        let current = self.ivars().find_index.borrow().unwrap_or_else(|| {
            if delta < 0 {
                0
            } else {
                len.saturating_sub(1)
            }
        });
        let next = if delta < 0 {
            (current + len - 1) % len
        } else {
            (current + 1) % len
        };
        self.ivars().find_index.replace(Some(next));
        self.activate_current_find_match();
    }

    pub(crate) fn activate_current_find_match(&self) {
        let Some(cell) = self
            .ivars()
            .find_index
            .borrow()
            .and_then(|index| self.ivars().find_matches.borrow().get(index).copied())
        else {
            return;
        };
        self.ivars()
            .state
            .borrow_mut()
            .select_cell(cell.row, cell.column);
        self.refresh_table();
    }

    pub(crate) fn find_cell_state(&self, cell: Cell) -> (bool, bool) {
        let matches = self.ivars().find_matches.borrow();
        let Some(index) = matches.iter().position(|match_cell| *match_cell == cell) else {
            return (false, false);
        };
        let active = self
            .ivars()
            .find_index
            .borrow()
            .is_some_and(|active| active == index);
        (true, active)
    }

    pub(crate) fn find_field_matches(&self, field: &NSTextField) -> bool {
        self.ivars()
            .find_panel
            .borrow()
            .as_ref()
            .is_some_and(|panel| {
                let panel_field = &*panel.field as *const NSTextField as *const ();
                let field = field as *const NSTextField as *const ();
                ptr::eq(panel_field, field)
            })
    }

    pub(crate) fn find_field_matches_control(&self, control: &NSControl) -> bool {
        self.ivars()
            .find_panel
            .borrow()
            .as_ref()
            .is_some_and(|panel| {
                let panel_field = &*panel.field as *const NSTextField as *const ();
                let control = control as *const NSControl as *const ();
                ptr::eq(panel_field, control)
            })
    }

    pub(crate) fn find_window_matches(&self, window: &NSWindow) -> bool {
        self.ivars()
            .find_panel
            .borrow()
            .as_ref()
            .is_some_and(|panel| {
                let panel_window = &*panel.window as *const NSWindow;
                ptr::eq(panel_window, window)
            })
    }

}

pub(crate) fn delimiter_titles() -> Vec<String> {
    ["Comma", "Semicolon", "Tab", "Pipe", "Colon", "Custom"]
        .iter()
        .map(|title| title.to_string())
        .collect()
}

pub(crate) fn delimiter_index(delimiter: u8) -> usize {
    match delimiter {
        b',' => 0,
        b';' => 1,
        b'\t' => 2,
        b'|' => 3,
        b':' => 4,
        _ => 5,
    }
}

pub(crate) fn custom_delimiter_text(delimiter: u8) -> String {
    if delimiter_index(delimiter) == 5 {
        (delimiter as char).to_string()
    } else {
        String::new()
    }
}

pub(crate) fn delimiter_at(index: NSInteger, custom: &str) -> std::result::Result<u8, String> {
    match index {
        0 => Ok(b','),
        1 => Ok(b';'),
        2 => Ok(b'\t'),
        3 => Ok(b'|'),
        4 => Ok(b':'),
        _ => custom_delimiter(custom),
    }
}

pub(crate) fn custom_delimiter(value: &str) -> std::result::Result<u8, String> {
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

pub(crate) fn single_character(value: &str, label: &str) -> std::result::Result<char, String> {
    let mut chars = value.chars();
    let Some(ch) = chars.next() else {
        return Err(format!("{label} must be one character."));
    };
    if chars.next().is_some() {
        return Err(format!("{label} must be one character."));
    }
    Ok(ch)
}

pub(crate) fn filter_operator_titles() -> Vec<String> {
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

pub(crate) fn filter_operator_index(operator: FilterOperator) -> usize {
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

pub(crate) fn filter_operator_at(index: NSInteger) -> FilterOperator {
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

