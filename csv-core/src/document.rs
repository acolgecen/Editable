use crate::dialect::detect_dialect;
use crate::parser::{decode_field, encode_field, parse_records};
use crate::{CsvDialect, CsvError, RecordIndex, Result};
use memmap2::Mmap;
use rayon::prelude::*;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellCoord {
    pub row: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenOptions {
    pub first_row_is_header: bool,
    pub skip_rows: usize,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            first_row_is_header: true,
            skip_rows: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortKey {
    pub column: usize,
    pub direction: SortDirection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnFilter {
    Contains(String),
    Equals(String),
    Empty,
    NotEmpty,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EditStats {
    pub edited_cells: usize,
    pub inserted_rows: usize,
    pub deleted_rows: usize,
    pub inserted_columns: usize,
    pub deleted_columns: usize,
}

enum Source {
    Mmap(Mmap),
    Bytes(Vec<u8>),
}

impl Source {
    fn bytes(&self) -> &[u8] {
        match self {
            Source::Mmap(map) => map,
            Source::Bytes(bytes) => bytes,
        }
    }
}

pub struct CsvDocument {
    path: Option<PathBuf>,
    source: Source,
    dialect: CsvDialect,
    records: Vec<RecordIndex>,
    skipped_rows: Vec<Vec<String>>,
    data_rows: Vec<usize>,
    view_rows: Vec<usize>,
    headers: Option<Vec<String>>,
    column_order: Vec<usize>,
    inserted_rows: Vec<Vec<String>>,
    inserted_columns: Vec<Vec<String>>,
    deleted_rows: HashSet<usize>,
    deleted_columns: HashSet<usize>,
    edits: HashMap<CellCoord, String>,
    filters: HashMap<usize, ColumnFilter>,
    sort_keys: Vec<SortKey>,
    dirty: bool,
}

impl CsvDocument {
    pub fn open(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let path = path.as_ref();
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        Self::from_source(Some(path.to_path_buf()), Source::Mmap(mmap), options)
    }

    pub fn from_bytes(bytes: Vec<u8>, options: OpenOptions) -> Result<Self> {
        Self::from_source(None, Source::Bytes(bytes), options)
    }

    fn from_source(path: Option<PathBuf>, source: Source, options: OpenOptions) -> Result<Self> {
        let dialect = detect_dialect(source.bytes());
        let records = parse_records(source.bytes(), dialect)?;
        let first_data_record = options.skip_rows + usize::from(options.first_row_is_header);
        let skipped_rows = records
            .iter()
            .take(options.skip_rows)
            .map(|record| read_record(source.bytes(), record, dialect))
            .collect::<Vec<_>>();
        let data_rows = (first_data_record..records.len()).collect::<Vec<_>>();
        let view_rows = (0..data_rows.len()).collect::<Vec<_>>();
        let headers = if options.first_row_is_header && options.skip_rows < records.len() {
            Some(read_record(
                source.bytes(),
                &records[options.skip_rows],
                dialect,
            ))
        } else {
            None
        };
        let column_count = records
            .iter()
            .skip(options.skip_rows)
            .map(|record| record.fields.len())
            .max()
            .unwrap_or(0);

        Ok(Self {
            path,
            source,
            dialect,
            records,
            skipped_rows,
            data_rows,
            view_rows,
            headers,
            column_order: (0..column_count).collect(),
            inserted_rows: Vec::new(),
            inserted_columns: Vec::new(),
            deleted_rows: HashSet::new(),
            deleted_columns: HashSet::new(),
            edits: HashMap::new(),
            filters: HashMap::new(),
            sort_keys: Vec::new(),
            dirty: false,
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn dialect(&self) -> CsvDialect {
        self.dialect
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn headers(&self) -> Option<&[String]> {
        self.headers.as_deref()
    }

    pub fn header(&self, column: usize) -> Option<&str> {
        let real_column = self.real_column(column)?;
        self.headers
            .as_ref()
            .and_then(|headers| headers.get(real_column))
            .map(String::as_str)
    }

    pub fn row_count(&self) -> usize {
        self.visible_source_row_count() + self.inserted_rows.len()
    }

    pub fn column_count(&self) -> usize {
        self.column_order
            .iter()
            .filter(|column| !self.deleted_columns.contains(column))
            .count()
            + self.inserted_columns.len()
    }

    pub fn edit_stats(&self) -> EditStats {
        EditStats {
            edited_cells: self.edits.len(),
            inserted_rows: self.inserted_rows.len(),
            deleted_rows: self.deleted_rows.len(),
            inserted_columns: self.inserted_columns.len(),
            deleted_columns: self.deleted_columns.len(),
        }
    }

    pub fn cell(&self, row: usize, column: usize) -> Option<String> {
        let real_column = self.real_column(column)?;
        let storage_row = self.storage_row(row)?;
        if let Some(value) = self.edits.get(&CellCoord {
            row: storage_row,
            column: real_column,
        }) {
            return Some(value.clone());
        }

        let visible_source_rows = self.visible_source_row_count();
        if row >= visible_source_rows {
            let inserted = row.checked_sub(visible_source_rows)?;
            return self
                .inserted_rows
                .get(inserted)
                .and_then(|values| values.get(real_column).cloned())
                .or_else(|| Some(String::new()));
        }

        let data_row = self.visible_data_row_at(row)?;
        if self.deleted_rows.contains(&data_row) {
            return None;
        }
        let record_idx = *self.data_rows.get(data_row)?;
        let record = self.records.get(record_idx)?;
        record
            .fields
            .get(real_column)
            .map(|field| decode_field(self.source.bytes(), *field, self.dialect))
            .or_else(|| Some(String::new()))
    }

    pub fn set_cell(&mut self, row: usize, column: usize, value: impl Into<String>) -> Result<()> {
        let real_column = self
            .real_column(column)
            .ok_or(CsvError::InvalidColumn { column })?;
        let storage_row = self.storage_row(row).ok_or(CsvError::InvalidRow { row })?;
        self.edits.insert(
            CellCoord {
                row: storage_row,
                column: real_column,
            },
            value.into(),
        );
        self.dirty = true;
        Ok(())
    }

    pub fn insert_row(&mut self, at: usize) -> Result<()> {
        if at > self.row_count() {
            return Err(CsvError::InvalidRow { row: at });
        }
        let row = vec![String::new(); self.column_count()];
        let insert_at = at
            .saturating_sub(self.visible_source_row_count())
            .min(self.inserted_rows.len());
        self.inserted_rows.insert(insert_at, row);
        self.dirty = true;
        Ok(())
    }

    pub fn delete_row(&mut self, row: usize) -> Result<()> {
        if row >= self.row_count() {
            return Err(CsvError::InvalidRow { row });
        }
        let visible_source_rows = self.visible_source_row_count();
        if row >= visible_source_rows {
            self.inserted_rows.remove(row - visible_source_rows);
        } else if let Some(data_row) = self.visible_data_row_at(row) {
            self.deleted_rows.insert(data_row);
        }
        self.dirty = true;
        Ok(())
    }

    pub fn insert_column(&mut self, at: usize) -> Result<()> {
        if at > self.column_count() {
            return Err(CsvError::InvalidColumn { column: at });
        }
        let new_column = self.column_order.len() + self.inserted_columns.len();
        self.column_order
            .insert(at.min(self.column_order.len()), new_column);
        self.inserted_columns
            .push(vec![String::new(); self.row_count()]);
        self.dirty = true;
        Ok(())
    }

    pub fn delete_column(&mut self, column: usize) -> Result<()> {
        let real_column = self
            .real_column(column)
            .ok_or(CsvError::InvalidColumn { column })?;
        self.deleted_columns.insert(real_column);
        self.dirty = true;
        Ok(())
    }

    pub fn reorder_row(&mut self, from: usize, to: usize) -> Result<()> {
        let Some(from_data_row) = self.visible_data_row_at(from) else {
            return Err(CsvError::InvalidReorder { from, to });
        };
        let Some(to_data_row) = self.visible_data_row_at(to) else {
            return Err(CsvError::InvalidReorder { from, to });
        };
        let from_idx = self
            .view_rows
            .iter()
            .position(|row| *row == from_data_row)
            .ok_or(CsvError::InvalidReorder { from, to })?;
        let to_idx = self
            .view_rows
            .iter()
            .position(|row| *row == to_data_row)
            .ok_or(CsvError::InvalidReorder { from, to })?;
        let row = self.view_rows.remove(from_idx);
        self.view_rows.insert(to_idx, row);
        self.dirty = true;
        Ok(())
    }

    pub fn reorder_column(&mut self, from: usize, to: usize) -> Result<()> {
        if from >= self.column_order.len() || to >= self.column_order.len() {
            return Err(CsvError::InvalidReorder { from, to });
        }
        let column = self.column_order.remove(from);
        self.column_order.insert(to, column);
        self.dirty = true;
        Ok(())
    }

    pub fn set_filter(&mut self, column: usize, filter: Option<ColumnFilter>) -> Result<()> {
        let real_column = self
            .real_column(column)
            .ok_or(CsvError::InvalidColumn { column })?;
        if let Some(filter) = filter {
            self.filters.insert(real_column, filter);
        } else {
            self.filters.remove(&real_column);
        }
        self.refresh_view();
        Ok(())
    }

    pub fn sort_by(&mut self, keys: Vec<SortKey>) -> Result<()> {
        let mut real_keys = Vec::with_capacity(keys.len());
        for key in keys {
            let Some(real_column) = self.real_column(key.column) else {
                return Err(CsvError::InvalidColumn { column: key.column });
            };
            real_keys.push(SortKey {
                column: real_column,
                direction: key.direction,
            });
        }
        self.sort_keys = real_keys;
        self.refresh_view();
        Ok(())
    }

    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<()> {
        std::fs::write(path, self.to_csv_bytes())?;
        Ok(())
    }

    pub fn to_csv_bytes(&self) -> Vec<u8> {
        let mut out = String::new();
        for row in &self.skipped_rows {
            write_record(&mut out, row, self.dialect);
            out.push_str(self.dialect.line_ending.as_str());
        }
        if let Some(headers) = &self.headers {
            let visible_headers = (0..self.column_count())
                .map(|column| {
                    self.real_column(column)
                        .and_then(|real| headers.get(real))
                        .cloned()
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>();
            write_record(&mut out, &visible_headers, self.dialect);
            out.push_str(self.dialect.line_ending.as_str());
        }

        for row in 0..self.row_count() {
            let values = (0..self.column_count())
                .map(|column| self.cell(row, column).unwrap_or_default())
                .collect::<Vec<_>>();
            write_record(&mut out, &values, self.dialect);
            if row + 1 < self.row_count() {
                out.push_str(self.dialect.line_ending.as_str());
            }
        }
        out.into_bytes()
    }

    fn refresh_view(&mut self) {
        let mut rows = (0..self.data_rows.len())
            .filter(|row| !self.deleted_rows.contains(row))
            .filter(|row| self.row_matches_filters(*row))
            .collect::<Vec<_>>();

        if !self.sort_keys.is_empty() {
            let keys = self.sort_keys.clone();
            rows.par_sort_by(|left, right| self.compare_rows(*left, *right, &keys));
        }
        self.view_rows = rows;
    }

    fn visible_source_row_count(&self) -> usize {
        self.view_rows
            .iter()
            .filter(|row| !self.deleted_rows.contains(row))
            .count()
    }

    fn visible_data_row_at(&self, visible_row: usize) -> Option<usize> {
        self.view_rows
            .iter()
            .copied()
            .filter(|row| !self.deleted_rows.contains(row))
            .nth(visible_row)
    }

    fn storage_row(&self, visible_row: usize) -> Option<usize> {
        if visible_row < self.visible_source_row_count() {
            return self.visible_data_row_at(visible_row);
        }
        let inserted = visible_row.checked_sub(self.visible_source_row_count())?;
        (inserted < self.inserted_rows.len()).then_some(self.data_rows.len() + inserted)
    }

    fn row_matches_filters(&self, data_row: usize) -> bool {
        self.filters.iter().all(|(column, filter)| {
            let value = self.cell_from_data_row(data_row, *column);
            match filter {
                ColumnFilter::Contains(needle) => value.contains(needle),
                ColumnFilter::Equals(expected) => value == *expected,
                ColumnFilter::Empty => value.is_empty(),
                ColumnFilter::NotEmpty => !value.is_empty(),
            }
        })
    }

    fn compare_rows(&self, left: usize, right: usize, keys: &[SortKey]) -> Ordering {
        for key in keys {
            let left_value = self.cell_from_data_row(left, key.column);
            let right_value = self.cell_from_data_row(right, key.column);
            let ordering = naturalish_cmp(&left_value, &right_value);
            let ordering = match key.direction {
                SortDirection::Ascending => ordering,
                SortDirection::Descending => ordering.reverse(),
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        left.cmp(&right)
    }

    fn cell_from_data_row(&self, data_row: usize, column: usize) -> String {
        let Some(record_idx) = self.data_rows.get(data_row).copied() else {
            return String::new();
        };
        let Some(record) = self.records.get(record_idx) else {
            return String::new();
        };
        record
            .fields
            .get(column)
            .map(|field| decode_field(self.source.bytes(), *field, self.dialect))
            .unwrap_or_default()
    }

    fn real_column(&self, visible_column: usize) -> Option<usize> {
        self.column_order
            .iter()
            .copied()
            .filter(|column| !self.deleted_columns.contains(column))
            .nth(visible_column)
    }
}

fn read_record(bytes: &[u8], record: &RecordIndex, dialect: CsvDialect) -> Vec<String> {
    record
        .fields
        .iter()
        .map(|field| decode_field(bytes, *field, dialect))
        .collect()
}

fn write_record(out: &mut String, values: &[String], dialect: CsvDialect) {
    for (idx, value) in values.iter().enumerate() {
        if idx > 0 {
            out.push(dialect.delimiter as char);
        }
        out.push_str(&encode_field(value, dialect));
    }
}

fn naturalish_cmp(left: &str, right: &str) -> Ordering {
    match (left.parse::<f64>(), right.parse::<f64>()) {
        (Ok(left), Ok(right)) => left.partial_cmp(&right).unwrap_or(Ordering::Equal),
        _ => left.to_lowercase().cmp(&right.to_lowercase()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_with_header_and_skip_rows() {
        let doc = CsvDocument::from_bytes(
            b"metadata\nname,age\nAda,36\nGrace,85\n".to_vec(),
            OpenOptions {
                first_row_is_header: true,
                skip_rows: 1,
            },
        )
        .unwrap();
        let headers = doc.headers().unwrap();
        assert_eq!(headers[0], "name");
        assert_eq!(headers[1], "age");
        assert_eq!(doc.row_count(), 2);
        assert_eq!(doc.cell(0, 0).unwrap(), "Ada");
    }

    #[test]
    fn edits_and_saves_with_quotes() {
        let mut doc =
            CsvDocument::from_bytes(b"name,note\nAda,ok\n".to_vec(), OpenOptions::default())
                .unwrap();
        doc.set_cell(0, 1, "hello, \"world\"").unwrap();
        let text = String::from_utf8(doc.to_csv_bytes()).unwrap();
        assert_eq!(text, "name,note\nAda,\"hello, \"\"world\"\"\"");
    }

    #[test]
    fn filters_and_sorts() {
        let mut doc = CsvDocument::from_bytes(
            b"name,age\nB,2\nA,10\nC,1\n".to_vec(),
            OpenOptions::default(),
        )
        .unwrap();
        doc.set_filter(1, Some(ColumnFilter::NotEmpty)).unwrap();
        doc.sort_by(vec![SortKey {
            column: 1,
            direction: SortDirection::Ascending,
        }])
        .unwrap();
        assert_eq!(doc.cell(0, 0).unwrap(), "C");
        assert_eq!(doc.cell(2, 0).unwrap(), "A");
    }

    #[test]
    fn row_and_column_operations() {
        let mut doc =
            CsvDocument::from_bytes(b"a,b\n1,2\n3,4\n".to_vec(), OpenOptions::default()).unwrap();
        doc.reorder_row(1, 0).unwrap();
        assert_eq!(doc.cell(0, 0).unwrap(), "3");
        doc.reorder_column(1, 0).unwrap();
        assert_eq!(doc.cell(0, 0).unwrap(), "4");
        doc.delete_row(0).unwrap();
        assert_eq!(doc.row_count(), 1);
    }

    #[test]
    fn saves_visible_column_order_and_skipped_rows() {
        let mut doc = CsvDocument::from_bytes(
            b"generated by tool\nname,year\nVisiCalc,1979\nExcel,1985\n".to_vec(),
            OpenOptions {
                first_row_is_header: true,
                skip_rows: 1,
            },
        )
        .unwrap();
        doc.reorder_column(1, 0).unwrap();
        doc.delete_row(0).unwrap();
        let text = String::from_utf8(doc.to_csv_bytes()).unwrap();
        assert_eq!(text, "generated by tool\nyear,name\n1985,Excel");
    }

    #[test]
    fn edits_follow_rows_after_sorting() {
        let mut doc =
            CsvDocument::from_bytes(b"name,age\nB,2\nA,10\n".to_vec(), OpenOptions::default())
                .unwrap();
        doc.set_cell(0, 0, "Bee").unwrap();
        doc.sort_by(vec![SortKey {
            column: 1,
            direction: SortDirection::Descending,
        }])
        .unwrap();
        assert_eq!(doc.cell(1, 0).unwrap(), "Bee");
    }
}
