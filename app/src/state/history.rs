//! Undo/redo history for [`EditableState`].
//!
//! Every mutation runs through [`EditableState::record_undoable`], which
//! snapshots the document before applying the change and pushes the previous
//! state onto a bounded undo stack.

use super::EditableState;
use crate::selection::Selection;
use editable_csv_core::{CsvDocumentSnapshot, Result};

const MAX_UNDO_HISTORY: usize = 10;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct HistoryEntry {
    document: CsvDocumentSnapshot,
    selection: Selection,
    filter_text: String,
}

// `can_undo`/`can_redo` round out the history API but are currently only
// exercised by tests.
#[allow(dead_code)]
impl EditableState {
    pub fn undo(&mut self) -> bool {
        let Some(entry) = self.undo_stack.pop() else {
            return false;
        };
        if let Some(current) = self.history_snapshot() {
            self.redo_stack.push(current);
        }
        self.restore_history_entry(entry);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(entry) = self.redo_stack.pop() else {
            return false;
        };
        if let Some(current) = self.history_snapshot() {
            self.push_undo(current);
        }
        self.restore_history_entry(entry);
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Run `change`, recording the prior state so it can be undone. On error the
    /// document is rolled back to the snapshot taken before `change` ran.
    pub(super) fn record_undoable(
        &mut self,
        change: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<()> {
        let Some(before) = self.history_snapshot() else {
            return change(self);
        };

        if let Err(err) = change(self) {
            self.restore_history_entry(before);
            return Err(err);
        }

        if self.history_snapshot().is_some_and(|after| after != before) {
            self.push_undo(before);
            self.redo_stack.clear();
        }
        Ok(())
    }

    pub(super) fn clear_history(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    fn history_snapshot(&self) -> Option<HistoryEntry> {
        self.document.as_ref().map(|document| HistoryEntry {
            document: document.snapshot(),
            selection: self.selection.clone(),
            filter_text: self.filter_text.clone(),
        })
    }

    fn restore_history_entry(&mut self, entry: HistoryEntry) {
        if let Some(document) = &mut self.document {
            document.restore_snapshot(entry.document);
            self.selection = entry.selection;
            self.filter_text = entry.filter_text;
            self.last_error = None;
        }
    }

    fn push_undo(&mut self, entry: HistoryEntry) {
        if self.undo_stack.len() == MAX_UNDO_HISTORY {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(entry);
    }
}
