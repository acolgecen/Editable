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
    pub delimiter: Option<u8>,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            first_row_is_header: true,
            skip_rows: 0,
            delimiter: None,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOperator {
    Contains,
    DoesNotContain,
    Equals,
    DoesNotEqual,
    StartsWith,
    EndsWith,
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
    IsEmpty,
    IsNotEmpty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterRule {
    pub column: usize,
    pub operator: FilterOperator,
    pub value: String,
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

#[derive(Clone, PartialEq, Eq)]
pub struct CsvDocumentSnapshot {
    view_rows: Vec<usize>,
    column_order: Vec<usize>,
    inserted_rows: Vec<Vec<String>>,
    inserted_columns: Vec<Vec<String>>,
    deleted_rows: HashSet<usize>,
    deleted_columns: HashSet<usize>,
    edits: HashMap<CellCoord, String>,
    filters: Vec<FilterRule>,
    sort_keys: Vec<SortKey>,
    dirty: bool,
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
    filters: Vec<FilterRule>,
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
        let mut dialect = detect_dialect(source.bytes());
        if let Some(delimiter) = options.delimiter {
            dialect.delimiter = delimiter;
        }
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
            filters: Vec::new(),
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

    pub fn set_dirty(&mut self, dirty: bool) {
        self.dirty = dirty;
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

    pub fn snapshot(&self) -> CsvDocumentSnapshot {
        CsvDocumentSnapshot {
            view_rows: self.view_rows.clone(),
            column_order: self.column_order.clone(),
            inserted_rows: self.inserted_rows.clone(),
            inserted_columns: self.inserted_columns.clone(),
            deleted_rows: self.deleted_rows.clone(),
            deleted_columns: self.deleted_columns.clone(),
            edits: self.edits.clone(),
            filters: self.filters.clone(),
            sort_keys: self.sort_keys.clone(),
            dirty: self.dirty,
        }
    }

    pub fn restore_snapshot(&mut self, snapshot: CsvDocumentSnapshot) {
        self.view_rows = snapshot.view_rows;
        self.column_order = snapshot.column_order;
        self.inserted_rows = snapshot.inserted_rows;
        self.inserted_columns = snapshot.inserted_columns;
        self.deleted_rows = snapshot.deleted_rows;
        self.deleted_columns = snapshot.deleted_columns;
        self.edits = snapshot.edits;
        self.filters = snapshot.filters;
        self.sort_keys = snapshot.sort_keys;
        self.dirty = snapshot.dirty;
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
        self.filters.retain(|rule| rule.column != real_column);
        if let Some(filter) = filter {
            self.filters.push(legacy_filter_rule(real_column, filter));
        }
        self.refresh_view();
        Ok(())
    }

    pub fn set_filter_rules(&mut self, rules: Vec<FilterRule>) -> Result<()> {
        let mut real_rules = Vec::with_capacity(rules.len());
        for rule in rules {
            let Some(real_column) = self.real_column(rule.column) else {
                return Err(CsvError::InvalidColumn {
                    column: rule.column,
                });
            };
            real_rules.push(FilterRule {
                column: real_column,
                ..rule
            });
        }
        self.filters = real_rules;
        self.refresh_view();
        Ok(())
    }

    pub fn filter_rules(&self) -> Vec<FilterRule> {
        self.filters
            .iter()
            .filter_map(|rule| {
                self.visible_column(rule.column).map(|column| FilterRule {
                    column,
                    operator: rule.operator,
                    value: rule.value.clone(),
                })
            })
            .collect()
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

    pub fn sort_keys(&self) -> Vec<SortKey> {
        self.sort_keys
            .iter()
            .filter_map(|key| {
                self.visible_column(key.column).map(|column| SortKey {
                    column,
                    direction: key.direction,
                })
            })
            .collect()
    }

    pub fn save_to(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let bytes = self.to_csv_bytes();
        std::fs::write(path, &bytes)?;
        self.rebase_saved_bytes(path.to_path_buf(), bytes)?;
        self.dirty = false;
        Ok(())
    }

    fn rebase_saved_bytes(&mut self, path: PathBuf, bytes: Vec<u8>) -> Result<()> {
        let filters = self.filter_rules();
        let sort_keys = self.sort_keys();
        let mut saved = Self::from_source(
            Some(path),
            Source::Bytes(bytes),
            OpenOptions {
                first_row_is_header: self.headers.is_some(),
                skip_rows: self.skipped_rows.len(),
                delimiter: Some(self.dialect.delimiter),
            },
        )?;

        saved.filters = filters
            .into_iter()
            .filter_map(|rule| {
                saved.real_column(rule.column).map(|column| FilterRule {
                    column,
                    operator: rule.operator,
                    value: rule.value,
                })
            })
            .collect();
        saved.sort_keys = sort_keys
            .into_iter()
            .filter_map(|key| {
                saved.real_column(key.column).map(|column| SortKey {
                    column,
                    direction: key.direction,
                })
            })
            .collect();
        saved.refresh_view();

        *self = saved;
        Ok(())
    }

    pub fn to_csv_bytes(&self) -> Vec<u8> {
        self.to_csv_bytes_with_dialect(self.dialect, true)
    }

    pub fn to_csv_bytes_with_delimiter(&self, delimiter: u8) -> Vec<u8> {
        let mut dialect = self.dialect;
        dialect.delimiter = delimiter;
        self.to_csv_bytes_with_dialect(dialect, true)
    }

    pub fn to_csv_bytes_with_delimiter_untrimmed(&self, delimiter: u8) -> Vec<u8> {
        let mut dialect = self.dialect;
        dialect.delimiter = delimiter;
        self.to_csv_bytes_with_dialect(dialect, false)
    }

    fn to_csv_bytes_with_dialect(&self, dialect: CsvDialect, trim_empty_edges: bool) -> Vec<u8> {
        let data_rows = (0..self.row_count())
            .map(|row| {
                (0..self.column_count())
                    .map(|column| self.cell(row, column).unwrap_or_default())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let retained_data_rows = if trim_empty_edges {
            retained_row_count(&data_rows)
        } else {
            data_rows.len()
        };
        let retained_columns = if trim_empty_edges {
            retained_column_count(self, &data_rows[..retained_data_rows])
        } else {
            self.column_count()
        };

        let mut records = Vec::new();
        records.extend(self.skipped_rows.iter().cloned());
        if let Some(headers) = &self.headers {
            if retained_columns > 0 || !trim_empty_edges {
                let visible_headers = (0..retained_columns)
                    .map(|column| {
                        self.real_column(column)
                            .and_then(|real| headers.get(real))
                            .cloned()
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>();
                records.push(visible_headers);
            }
        }
        records.extend(
            data_rows
                .iter()
                .take(retained_data_rows)
                .map(|row| trimmed_record(row, retained_columns)),
        );

        let mut out = String::new();
        for (idx, record) in records.iter().enumerate() {
            write_record(&mut out, record, dialect);
            if idx + 1 < records.len() {
                out.push_str(dialect.line_ending.as_str());
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
        self.filters.iter().all(|rule| {
            let value = self.cell_from_data_row(data_row, rule.column);
            rule_matches_value(rule, &value)
        })
    }

    fn compare_rows(&self, left: usize, right: usize, keys: &[SortKey]) -> Ordering {
        for key in keys {
            let left_value = self.cell_from_data_row(left, key.column);
            let right_value = self.cell_from_data_row(right, key.column);
            let left_null = left_value.is_empty();
            let right_null = right_value.is_empty();
            let ordering = match (left_null, right_null) {
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (true, true) => Ordering::Equal,
                (false, false) => naturalish_cmp(&left_value, &right_value),
            };
            if ordering != Ordering::Equal && (left_null || right_null) {
                return ordering;
            }
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

    fn visible_column(&self, real_column: usize) -> Option<usize> {
        self.column_order
            .iter()
            .copied()
            .filter(|column| !self.deleted_columns.contains(column))
            .position(|column| column == real_column)
    }
}

fn legacy_filter_rule(column: usize, filter: ColumnFilter) -> FilterRule {
    match filter {
        ColumnFilter::Contains(value) => FilterRule {
            column,
            operator: FilterOperator::Contains,
            value,
        },
        ColumnFilter::Equals(value) => FilterRule {
            column,
            operator: FilterOperator::Equals,
            value,
        },
        ColumnFilter::Empty => FilterRule {
            column,
            operator: FilterOperator::IsEmpty,
            value: String::new(),
        },
        ColumnFilter::NotEmpty => FilterRule {
            column,
            operator: FilterOperator::IsNotEmpty,
            value: String::new(),
        },
    }
}

fn rule_matches_value(rule: &FilterRule, value: &str) -> bool {
    match rule.operator {
        FilterOperator::IsEmpty => value.is_empty(),
        FilterOperator::IsNotEmpty => !value.is_empty(),
        FilterOperator::Contains => {
            text_match(rule, value, |value, pattern| value.contains(pattern))
        }
        FilterOperator::DoesNotContain => {
            !text_match(rule, value, |value, pattern| value.contains(pattern))
        }
        FilterOperator::Equals => text_match(rule, value, |value, pattern| value == pattern),
        FilterOperator::DoesNotEqual => !text_match(rule, value, |value, pattern| value == pattern),
        FilterOperator::StartsWith => {
            text_match(rule, value, |value, pattern| value.starts_with(pattern))
        }
        FilterOperator::EndsWith => {
            text_match(rule, value, |value, pattern| value.ends_with(pattern))
        }
        FilterOperator::GreaterThan => {
            compare_filter_value(value, &rule.value) == Some(Ordering::Greater)
        }
        FilterOperator::GreaterThanOrEqual => matches!(
            compare_filter_value(value, &rule.value),
            Some(Ordering::Greater | Ordering::Equal)
        ),
        FilterOperator::LessThan => {
            compare_filter_value(value, &rule.value) == Some(Ordering::Less)
        }
        FilterOperator::LessThanOrEqual => matches!(
            compare_filter_value(value, &rule.value),
            Some(Ordering::Less | Ordering::Equal)
        ),
    }
}

fn text_match(rule: &FilterRule, value: &str, plain: impl Fn(&str, &str) -> bool) -> bool {
    plain(value, &rule.value)
}

fn compare_filter_value(value: &str, expected: &str) -> Option<Ordering> {
    match (parse_number(value), parse_number(expected)) {
        (Some(value), Some(expected)) => value.partial_cmp(&expected),
        (None, None) => Some(value.to_lowercase().cmp(&expected.to_lowercase())),
        _ => None,
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

fn retained_row_count(rows: &[Vec<String>]) -> usize {
    rows.iter()
        .rposition(|row| row.iter().any(|value| !value.is_empty()))
        .map(|index| index + 1)
        .unwrap_or(0)
}

fn retained_column_count(document: &CsvDocument, rows: &[Vec<String>]) -> usize {
    let header_count = document.headers.as_ref().map_or(0, |headers| {
        (0..document.column_count())
            .rposition(|column| {
                document
                    .real_column(column)
                    .and_then(|real| headers.get(real))
                    .is_some_and(|value| !value.is_empty())
            })
            .map(|column| column + 1)
            .unwrap_or(0)
    });
    let row_count = rows
        .iter()
        .filter_map(|row| {
            row.iter()
                .rposition(|value| !value.is_empty())
                .map(|column| column + 1)
        })
        .max()
        .unwrap_or(0);
    header_count.max(row_count)
}

fn trimmed_record(row: &[String], retained_columns: usize) -> Vec<String> {
    row.iter()
        .take(retained_columns)
        .cloned()
        .collect::<Vec<_>>()
}

fn naturalish_cmp(left: &str, right: &str) -> Ordering {
    match (parse_number(left), parse_number(right)) {
        (Some(left), Some(right)) => left.partial_cmp(&right).unwrap_or(Ordering::Equal),
        _ => left.to_lowercase().cmp(&right.to_lowercase()),
    }
}

fn parse_number(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
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
                ..OpenOptions::default()
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
    fn save_clears_dirty_state() {
        let mut doc =
            CsvDocument::from_bytes(b"name,note\nAda,ok\n".to_vec(), OpenOptions::default())
                .unwrap();
        doc.set_cell(0, 1, "saved").unwrap();
        assert!(doc.is_dirty());

        let path = std::env::temp_dir().join(format!(
            "editable-save-clears-dirty-{}.csv",
            std::process::id()
        ));
        doc.save_to(&path).unwrap();

        assert!(!doc.is_dirty());
        assert_eq!(doc.path(), Some(path.as_path()));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "name,note\nAda,saved"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn save_rebases_in_memory_document_to_written_bytes() {
        let path =
            std::env::temp_dir().join(format!("editable-save-rebase-{}.csv", std::process::id()));
        std::fs::write(&path, "first,second\ncell1,cell2").unwrap();

        let mut doc = CsvDocument::open(&path, OpenOptions::default()).unwrap();
        doc.set_cell(0, 1, "updated").unwrap();
        doc.save_to(&path).unwrap();

        assert!(!doc.is_dirty());
        assert_eq!(doc.edit_stats().edited_cells, 0);
        assert_eq!(doc.cell(0, 0).unwrap(), "cell1");
        assert_eq!(doc.cell(0, 1).unwrap(), "updated");
        assert_eq!(
            String::from_utf8(doc.to_csv_bytes()).unwrap(),
            std::fs::read_to_string(&path).unwrap()
        );

        let _ = std::fs::remove_file(path);
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
    fn empty_values_sort_last_in_both_directions() {
        let mut doc = CsvDocument::from_bytes(
            b"name,score\nBlank,\nLow,1\nHigh,9\n".to_vec(),
            OpenOptions::default(),
        )
        .unwrap();
        doc.sort_by(vec![SortKey {
            column: 1,
            direction: SortDirection::Ascending,
        }])
        .unwrap();
        assert_eq!(doc.cell(0, 0).unwrap(), "Low");
        assert_eq!(doc.cell(2, 0).unwrap(), "Blank");

        doc.sort_by(vec![SortKey {
            column: 1,
            direction: SortDirection::Descending,
        }])
        .unwrap();
        assert_eq!(doc.cell(0, 0).unwrap(), "High");
        assert_eq!(doc.cell(2, 0).unwrap(), "Blank");
    }

    #[test]
    fn multiple_filter_rules_can_use_text_and_comparisons() {
        let mut doc = CsvDocument::from_bytes(
            b"name,age\nAda,36\nGrace,85\nAlan,41\n".to_vec(),
            OpenOptions::default(),
        )
        .unwrap();
        doc.set_filter_rules(vec![
            FilterRule {
                column: 0,
                operator: FilterOperator::StartsWith,
                value: "A".to_string(),
            },
            FilterRule {
                column: 1,
                operator: FilterOperator::GreaterThan,
                value: "40".to_string(),
            },
        ])
        .unwrap();
        assert_eq!(doc.row_count(), 1);
        assert_eq!(doc.cell(0, 0).unwrap(), "Alan");
    }

    #[test]
    fn custom_delimiter_and_dot_decimal_numbers_drive_comparisons() {
        let mut doc = CsvDocument::from_bytes(
            b"name;amount\nSmall;1.25\nLarge;10.5\n".to_vec(),
            OpenOptions {
                delimiter: Some(b';'),
                ..OpenOptions::default()
            },
        )
        .unwrap();

        assert_eq!(doc.cell(0, 1).unwrap(), "1.25");
        doc.set_filter_rules(vec![FilterRule {
            column: 1,
            operator: FilterOperator::GreaterThan,
            value: "2.0".to_string(),
        }])
        .unwrap();
        assert_eq!(doc.cell(0, 0).unwrap(), "Large");

        let text = String::from_utf8(doc.to_csv_bytes()).unwrap();
        assert_eq!(text, "name;amount\nLarge;10.5");
    }

    #[test]
    fn comma_decimal_values_remain_text_not_locale_numbers() {
        let mut doc = CsvDocument::from_bytes(
            b"name;amount\nSmall;1,25\nLarge;10,5\n".to_vec(),
            OpenOptions {
                delimiter: Some(b';'),
                ..OpenOptions::default()
            },
        )
        .unwrap();

        doc.set_filter_rules(vec![FilterRule {
            column: 1,
            operator: FilterOperator::GreaterThan,
            value: "2.0".to_string(),
        }])
        .unwrap();

        assert_eq!(doc.row_count(), 0);
    }

    #[test]
    fn default_open_options_auto_detect_delimiter_before_saving() {
        let mut doc = CsvDocument::from_bytes(
            b"name;amount\nAda;1,23\nGrace;4,56\n".to_vec(),
            OpenOptions::default(),
        )
        .unwrap();

        assert_eq!(doc.dialect().delimiter, b';');
        assert_eq!(doc.column_count(), 2);
        assert_eq!(doc.cell(0, 0).unwrap(), "Ada");
        assert_eq!(doc.cell(0, 1).unwrap(), "1,23");

        doc.set_cell(0, 0, "Augusta").unwrap();
        let text = String::from_utf8(doc.to_csv_bytes()).unwrap();
        assert_eq!(text, "name;amount\nAugusta;1,23\nGrace;4,56");
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
                ..OpenOptions::default()
            },
        )
        .unwrap();
        doc.reorder_column(1, 0).unwrap();
        doc.delete_row(0).unwrap();
        let text = String::from_utf8(doc.to_csv_bytes()).unwrap();
        assert_eq!(text, "generated by tool\nyear,name\n1985,Excel");
    }

    #[test]
    fn saving_trims_trailing_empty_rows_and_columns() {
        let doc = CsvDocument::from_bytes(
            b"name,year,,\nAda,1843,,\n,,,\n".to_vec(),
            OpenOptions::default(),
        )
        .unwrap();

        let text = String::from_utf8(doc.to_csv_bytes()).unwrap();
        assert_eq!(text, "name,year\nAda,1843");
    }

    #[test]
    fn saving_preserves_empty_rows_and_columns_inside_data() {
        let doc = CsvDocument::from_bytes(
            b"name,,year,\nAda,,1843,\n,,,\nGrace,,1952,\n,,,\n".to_vec(),
            OpenOptions::default(),
        )
        .unwrap();

        let text = String::from_utf8(doc.to_csv_bytes()).unwrap();
        assert_eq!(text, "name,,year\nAda,,1843\n,,\nGrace,,1952");
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
