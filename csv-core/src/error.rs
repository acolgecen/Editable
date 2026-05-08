use std::{fmt, io};

pub type Result<T> = std::result::Result<T, CsvError>;

#[derive(Debug)]
pub enum CsvError {
    Io(io::Error),
    InvalidRow { row: usize },
    InvalidColumn { column: usize },
    InvalidReorder { from: usize, to: usize },
    UnsupportedEncoding(&'static str),
    Parse(String),
}

impl fmt::Display for CsvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CsvError::Io(err) => write!(f, "{err}"),
            CsvError::InvalidRow { row } => write!(f, "invalid row index {row}"),
            CsvError::InvalidColumn { column } => write!(f, "invalid column index {column}"),
            CsvError::InvalidReorder { from, to } => {
                write!(f, "cannot reorder item from {from} to {to}")
            }
            CsvError::UnsupportedEncoding(name) => write!(f, "unsupported encoding: {name}"),
            CsvError::Parse(message) => write!(f, "CSV parse error: {message}"),
        }
    }
}

impl std::error::Error for CsvError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CsvError::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for CsvError {
    fn from(value: io::Error) -> Self {
        CsvError::Io(value)
    }
}
