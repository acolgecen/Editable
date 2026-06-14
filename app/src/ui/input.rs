use super::*;
use crate::selection::Cell;
use crate::state::MAX_COPY_CELLS;
use objc2::ffi::{NSInteger, NSUInteger};
use objc2::runtime::Sel;
use objc2::{sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertStyle, NSApplication, NSEvent, NSEventModifierFlags, NSPasteboard, NSPasteboardTypeString,
    NSPasteboardTypeTabularText, NSTextView,
};
use objc2_foundation::{
    MainThreadMarker, NSRange, NSString,
};

// Pointer and keyboard handling: translating table clicks/drags and key
// presses into selection changes, cell edits, and clipboard copies.

#[derive(Debug, Clone, Copy)]
pub(crate) enum DragSelection {
    Cells { anchor: Cell },
    Range { anchor: Cell, selected: bool },
    ToggleCells { selected: bool, last: Cell },
    Rows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableHit {
    RowNumber(usize),
    Data { cell: Cell, table_column: NSInteger },
}


impl Delegate {
    pub(crate) fn navigate_selection(&self, rows: isize, columns: isize, extending: bool) {
        self.ivars()
            .state
            .borrow_mut()
            .move_selection(rows, columns, extending);
        self.refresh_table();
    }

    pub(crate) fn copy_selection_to_pasteboard(&self) -> bool {
        let cell_count = self.ivars().state.borrow().selection_cell_count();
        if let Some(count) = cell_count {
            if count > MAX_COPY_CELLS {
                let mtm = self.mtm();
                let alert = NSAlert::new(mtm);
                alert.setAlertStyle(NSAlertStyle::Warning);
                alert.setMessageText(&NSString::from_str("Selection Too Large to Copy"));
                alert.setInformativeText(&NSString::from_str(&format!(
                    "Your selection contains {} cells. Copying more than {} cells at once \
                     would use too much memory. Select a smaller range and try again.",
                    format_count(count),
                    format_count(MAX_COPY_CELLS),
                )));
                alert.addButtonWithTitle(&NSString::from_str("OK"));
                alert.runModal();
                return false;
            }
        }
        let Some(text) = self.ivars().state.borrow().selected_text() else {
            return false;
        };
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        let text = NSString::from_str(&text);
        unsafe {
            pasteboard.setString_forType(&text, NSPasteboardTypeString);
            pasteboard.setString_forType(&text, NSPasteboardTypeTabularText);
        }
        true
    }

    pub(crate) fn edit_top_left_selected_cell(&self, initial_text: Option<&str>) -> bool {
        let Some(table) = self.ivars().table.get() else {
            return false;
        };
        let active = self.ivars().state.borrow().selection.top_left_cell();
        let table_column = active.column + 1;
        if table.numberOfRows() <= active.row as NSInteger
            || table.numberOfColumns() <= table_column as NSInteger
        {
            return false;
        }

        self.ivars()
            .state
            .borrow_mut()
            .select_cell(active.row, active.column);
        table.reloadData();
        table.editColumn_row_withEvent_select(
            table_column as NSInteger,
            active.row as NSInteger,
            None,
            true,
        );

        if let Some(initial_text) = initial_text {
            if let Some(editor) = table.currentEditor() {
                let value = NSString::from_str(initial_text);
                editor.setString(&value);
                editor.setSelectedRange(NSRange::new(value.length(), 0));
            }
        }
        true
    }

    pub(crate) fn commit_text_view_edit(&self, _text_view: &NSTextView) {
        if let Some(window) = self.ivars().window.get() {
            unsafe { window.endEditingFor(None) };
        }
    }

    pub(crate) fn table_mouse_down(&self, table: &EditableTableView, event: &NSEvent) {
        if self.is_saving() {
            return;
        }
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

    pub(crate) fn table_mouse_dragged(&self, table: &EditableTableView, event: &NSEvent) {
        if self.is_saving() {
            return;
        }
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

    pub(crate) fn table_mouse_up(&self) {
        self.ivars().drag_selection.replace(None);
    }

    pub(crate) fn table_key_down(&self, event: &NSEvent) -> bool {
        if self.is_saving() {
            return true;
        }
        let modifiers = event.modifierFlags();
        let characters = event
            .charactersIgnoringModifiers()
            .map(|value| value.to_string())
            .unwrap_or_default();

        if modifiers.contains(NSEventModifierFlags::Command) && characters.eq_ignore_ascii_case("c")
        {
            self.copy_selection_to_pasteboard();
            return true;
        }

        if modifiers.contains(NSEventModifierFlags::Command) && characters.eq_ignore_ascii_case("f")
        {
            self.present_find_panel();
            return true;
        }

        if modifiers.contains(NSEventModifierFlags::Command) && characters.eq_ignore_ascii_case("z")
        {
            if modifiers.contains(NSEventModifierFlags::Shift) {
                if self.ivars().state.borrow_mut().redo() {
                    self.rebuild_columns();
                    self.refresh_table();
                }
            } else if self.ivars().state.borrow_mut().undo() {
                self.rebuild_columns();
                self.refresh_table();
            }
            return true;
        }

        if matches!(event.keyCode(), KEY_BACKSPACE | KEY_FORWARD_DELETE)
            && !modifiers.intersects(
                NSEventModifierFlags::Command
                    | NSEventModifierFlags::Control
                    | NSEventModifierFlags::Option,
            )
        {
            let result = self.ivars().state.borrow_mut().clear_selection_content();
            self.handle_result(result);
            self.refresh_table();
            return true;
        }

        if let Some((rows, columns)) = navigation_delta_for_key_code(event.keyCode()) {
            self.navigate_selection(
                rows,
                columns,
                modifiers.contains(NSEventModifierFlags::Shift),
            );
            return true;
        }

        if matches!(event.keyCode(), KEY_RETURN | KEY_KEYPAD_ENTER) {
            return self.edit_top_left_selected_cell(None);
        }

        if modifiers.intersects(
            NSEventModifierFlags::Command
                | NSEventModifierFlags::Control
                | NSEventModifierFlags::Option,
        ) {
            return false;
        }

        let characters = event
            .characters()
            .map(|value| value.to_string())
            .unwrap_or_default();
        if is_cell_edit_start_text(&characters) {
            return self.edit_top_left_selected_cell(Some(&characters));
        }

        false
    }

}

pub(crate) fn navigation_delta_for_key_code(key_code: u16) -> Option<(isize, isize)> {
    match key_code {
        KEY_UP_ARROW => Some((-1, 0)),
        KEY_DOWN_ARROW => Some((1, 0)),
        KEY_LEFT_ARROW => Some((0, -1)),
        KEY_RIGHT_ARROW => Some((0, 1)),
        _ => None,
    }
}

pub(crate) fn navigation_delta_for_selector(selector: Sel) -> Option<(isize, isize, bool)> {
    match selector {
        selector if selector == sel!(moveUp:) => Some((-1, 0, false)),
        selector if selector == sel!(moveDown:) => Some((1, 0, false)),
        selector if selector == sel!(moveLeft:) => Some((0, -1, false)),
        selector if selector == sel!(moveRight:) => Some((0, 1, false)),
        selector if selector == sel!(moveUpAndModifySelection:) => Some((-1, 0, true)),
        selector if selector == sel!(moveDownAndModifySelection:) => Some((1, 0, true)),
        selector if selector == sel!(moveLeftAndModifySelection:) => Some((0, -1, true)),
        selector if selector == sel!(moveRightAndModifySelection:) => Some((0, 1, true)),
        _ => None,
    }
}

pub(crate) fn current_event_has_shift(mtm: MainThreadMarker) -> bool {
    NSApplication::sharedApplication(mtm)
        .currentEvent()
        .is_some_and(|event| event.modifierFlags().contains(NSEventModifierFlags::Shift))
}

pub(crate) fn is_cell_edit_start_text(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(chars.next(), Some(ch) if ch.is_alphanumeric()) && chars.next().is_none()
}

pub(crate) fn table_hit(table: &EditableTableView, event: &NSEvent) -> Option<TableHit> {
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

pub(crate) fn format_count(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

