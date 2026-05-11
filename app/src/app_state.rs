use crate::selection::{Cell, Selection};
use editable_csv_core::{
    ColumnFilter, CsvDocument, FilterRule, OpenOptions, Result, SortDirection, SortKey,
};
use std::path::{Path, PathBuf};

pub struct EditableState {
    pub document: Option<CsvDocument>,
    pub selection: Selection,
    pub first_row_is_header: bool,
    pub skip_rows: usize,
    pub filter_text: String,
    pub last_error: Option<String>,
}

impl Default for EditableState {
    fn default() -> Self {
        Self {
            document: None,
            selection: Selection::default(),
            first_row_is_header: true,
            skip_rows: 0,
            filter_text: String::new(),
            last_error: None,
        }
    }
}

#[allow(dead_code)]
impl EditableState {
    pub fn open_path(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let document = CsvDocument::open(
            path,
            OpenOptions {
                first_row_is_header: self.first_row_is_header,
                skip_rows: self.skip_rows,
            },
        )?;
        self.document = Some(document);
        self.selection = Selection::default();
        self.last_error = None;
        Ok(())
    }

    pub fn reopen_with_options(&mut self) -> Result<()> {
        let Some(doc) = &self.document else {
            return self.open_sample();
        };
        if let Some(path) = doc.path().map(Path::to_path_buf) {
            self.open_path(path)
        } else {
            let bytes = include_bytes!("../../assets/samples/basic.csv").to_vec();
            self.document = Some(CsvDocument::from_bytes(
                bytes,
                OpenOptions {
                    first_row_is_header: self.first_row_is_header,
                    skip_rows: self.skip_rows,
                },
            )?);
            self.selection = Selection::default();
            self.last_error = None;
            Ok(())
        }
    }

    pub fn open_sample(&mut self) -> Result<()> {
        let sample = include_bytes!("../../assets/samples/basic.csv").to_vec();
        self.document = Some(CsvDocument::from_bytes(sample, OpenOptions::default())?);
        self.selection = Selection::default();
        Ok(())
    }

    pub fn title(&self) -> String {
        self.document
            .as_ref()
            .and_then(|doc| doc.path())
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(|name| format!("{name} - Editable"))
            .unwrap_or_else(|| "Editable".to_string())
    }

    pub fn preview_grid_text(&self, max_rows: usize, max_columns: usize) -> String {
        let Some(doc) = &self.document else {
            return "Open a CSV file to start editing.".to_string();
        };
        let mut out = String::new();
        if let Some(headers) = doc.headers() {
            out.push_str(
                &headers
                    .iter()
                    .take(max_columns)
                    .map(|value| fit(value, 18))
                    .collect::<Vec<_>>()
                    .join(" | "),
            );
            out.push('\n');
            out.push_str(&"-".repeat(max_columns.min(headers.len()) * 21));
            out.push('\n');
        }
        for row in 0..doc.row_count().min(max_rows) {
            let cells = (0..doc.column_count().min(max_columns))
                .map(|column| {
                    let value = doc.cell(row, column).unwrap_or_default();
                    fit(&value.replace('\n', " "), 18)
                })
                .collect::<Vec<_>>();
            out.push_str(&cells.join(" | "));
            out.push('\n');
        }
        out.push_str(&format!(
            "\n{} rows x {} columns{}",
            doc.row_count(),
            doc.column_count(),
            if doc.is_dirty() { " - edited" } else { "" }
        ));
        out
    }

    pub fn status_text(&self) -> String {
        let Some(doc) = &self.document else {
            return "Open a CSV file to start editing.".to_string();
        };
        let stats = doc.edit_stats();
        let edited = if doc.is_dirty() {
            format!(
                " - edited: {} cells, +{} rows, -{} rows, +{} columns, -{} columns",
                stats.edited_cells,
                stats.inserted_rows,
                stats.deleted_rows,
                stats.inserted_columns,
                stats.deleted_columns
            )
        } else {
            String::new()
        };
        format!(
            "{} rows x {} columns{}",
            doc.row_count(),
            doc.column_count(),
            edited
        )
    }

    pub fn column_title(&self, column: usize) -> String {
        let Some(doc) = &self.document else {
            return spreadsheet_column_name(column);
        };
        if let Some(title) = doc.header(column).filter(|title| !title.is_empty()) {
            return title.to_string();
        }
        spreadsheet_column_name(column)
    }

    pub fn set_cell(&mut self, value: impl Into<String>) -> Result<()> {
        let active = self.selection.active_cell();
        if let Some(doc) = &mut self.document {
            doc.set_cell(active.row, active.column, value)?;
        }
        Ok(())
    }

    pub fn insert_row(&mut self) -> Result<()> {
        let row = self.selection.active_cell().row;
        if let Some(doc) = &mut self.document {
            doc.insert_row(row)?;
        }
        Ok(())
    }

    pub fn insert_column(&mut self) -> Result<()> {
        let column = self.selection.active_cell().column;
        if let Some(doc) = &mut self.document {
            doc.insert_column(column)?;
        }
        Ok(())
    }

    pub fn delete_selection(&mut self) -> Result<()> {
        if let Some(doc) = &mut self.document {
            match self.selection.clone() {
                Selection::Row { anchor, focus } => {
                    for row in (anchor.min(focus)..=anchor.max(focus)).rev() {
                        doc.delete_row(row)?;
                    }
                }
                Selection::Column { anchor, focus } => {
                    for column in (anchor.min(focus)..=anchor.max(focus)).rev() {
                        doc.delete_column(column)?;
                    }
                }
                Selection::Cells { .. } | Selection::Cell(_) => {
                    for cell in self.selection.cells() {
                        doc.set_cell(cell.row, cell.column, "")?;
                    }
                }
                Selection::All => {
                    for row in (0..doc.row_count()).rev() {
                        doc.delete_row(row)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn sort_active_column(&mut self, direction: SortDirection) -> Result<()> {
        let column = self.selection.active_cell().column;
        if let Some(doc) = &mut self.document {
            doc.sort_by(vec![SortKey { column, direction }])?;
        }
        Ok(())
    }

    pub fn sort_keys(&self) -> Vec<SortKey> {
        self.document
            .as_ref()
            .map(CsvDocument::sort_keys)
            .unwrap_or_default()
    }

    pub fn filter_rules(&self) -> Vec<FilterRule> {
        self.document
            .as_ref()
            .map(CsvDocument::filter_rules)
            .unwrap_or_default()
    }

    pub fn apply_sort_filter_rules(
        &mut self,
        sort_keys: Vec<SortKey>,
        filter_rules: Vec<FilterRule>,
    ) -> Result<()> {
        if let Some(doc) = &mut self.document {
            doc.set_filter_rules(filter_rules)?;
            doc.sort_by(sort_keys)?;
        }
        self.filter_text.clear();
        Ok(())
    }

    pub fn filter_active_column_contains(&mut self, needle: String) -> Result<()> {
        let column = self.selection.active_cell().column;
        self.filter_text = needle.clone();
        if let Some(doc) = &mut self.document {
            let filter = if needle.is_empty() {
                None
            } else {
                Some(ColumnFilter::Contains(needle))
            };
            doc.set_filter(column, filter)?;
        }
        Ok(())
    }

    pub fn move_active_row(&mut self, delta: isize) -> Result<()> {
        let active = self.selection.active_cell();
        if let Some(doc) = &mut self.document {
            let to = active
                .row
                .saturating_add_signed(delta)
                .min(doc.row_count().saturating_sub(1));
            if to != active.row {
                doc.reorder_row(active.row, to)?;
                self.select_cell(to, active.column);
            }
        }
        Ok(())
    }

    pub fn move_active_column(&mut self, delta: isize) -> Result<()> {
        let active = self.selection.active_cell();
        if let Some(doc) = &mut self.document {
            let to = active
                .column
                .saturating_add_signed(delta)
                .min(doc.column_count().saturating_sub(1));
            if to != active.column {
                doc.reorder_column(active.column, to)?;
                self.select_cell(active.row, to);
            }
        }
        Ok(())
    }

    pub fn move_selection(&mut self, rows: isize, columns: isize, extending: bool) {
        let Some(doc) = &self.document else {
            return;
        };
        if extending {
            self.selection
                .extend_by(rows, columns, doc.row_count(), doc.column_count());
        } else {
            self.selection
                .move_by(rows, columns, doc.row_count(), doc.column_count());
        }
    }

    pub fn select_cell(&mut self, row: usize, column: usize) {
        self.selection = Selection::single_cell(Cell { row, column });
    }

    pub fn toggle_cell_selection(&mut self, row: usize, column: usize) -> bool {
        self.selection.toggle_cell(Cell { row, column })
    }

    pub fn set_cell_selection(&mut self, row: usize, column: usize, selected: bool) {
        self.selection
            .set_cell_selected(Cell { row, column }, selected);
    }

    pub fn select_cell_range_to(&mut self, row: usize, column: usize) {
        self.selection.select_rect_to(Cell { row, column });
    }

    pub fn select_cell_range_from(&mut self, anchor: Cell, row: usize, column: usize) {
        self.selection = Selection::single_cell(anchor);
        self.selection.select_rect_to(Cell { row, column });
    }

    pub fn set_cell_range_selection(&mut self, row: usize, column: usize, selected: bool) {
        let anchor = self.selection.anchor_cell();
        self.selection
            .set_rect_selected(anchor, Cell { row, column }, selected);
    }

    pub fn set_cell_range_selection_from(
        &mut self,
        anchor: Cell,
        row: usize,
        column: usize,
        selected: bool,
    ) {
        self.selection
            .set_rect_selected(anchor, Cell { row, column }, selected);
    }

    pub fn select_row(&mut self, row: usize) {
        self.selection = Selection::Row {
            anchor: row,
            focus: row,
        };
    }

    pub fn select_row_range_to(&mut self, row: usize) {
        match &mut self.selection {
            Selection::Row { focus, .. } => *focus = row,
            _ => self.select_row(row),
        }
    }

    pub fn select_column(&mut self, column: usize) {
        self.selection = Selection::Column {
            anchor: column,
            focus: column,
        };
    }

    pub fn save(&mut self, target: Option<PathBuf>) -> Result<()> {
        let Some(doc) = &self.document else {
            return Ok(());
        };
        let path = target
            .or_else(|| doc.path().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("Editable.csv"));
        doc.save_to(path)?;
        Ok(())
    }
}

fn fit(value: &str, width: usize) -> String {
    let mut chars = value.chars();
    let mut out = String::new();
    for _ in 0..width {
        match chars.next() {
            Some(ch) => out.push(ch),
            None => break,
        }
    }
    if chars.next().is_some() && width > 1 {
        out.pop();
        out.push_str("...");
    }
    format!("{out:width$}")
}

fn spreadsheet_column_name(mut column: usize) -> String {
    let mut name = String::new();
    loop {
        let rem = column % 26;
        name.insert(0, (b'A' + rem as u8) as char);
        if column < 26 {
            break;
        }
        column = column / 26 - 1;
    }
    name
}
