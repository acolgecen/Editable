//! High-performance CSV document core for Editable.
//!
//! The crate intentionally knows nothing about AppKit. It owns CSV dialect
//! detection, row indexing, editing overlays, filtering, sorting, and saving.

mod dialect;
mod document;
mod error;
mod parser;

pub use dialect::{detect_dialect, CsvDialect, Encoding, LineEnding};
pub use document::{
    CellCoord, ColumnFilter, CsvDocument, CsvDocumentSnapshot, EditStats, FilterOperator,
    FilterRule, NumberFormat, OpenOptions, SortDirection, SortKey,
};
pub use error::{CsvError, Result};
pub use parser::{FieldIndex, RecordIndex};
