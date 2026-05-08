use crate::{CsvDialect, Encoding, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldIndex {
    pub start: usize,
    pub end: usize,
    pub quoted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordIndex {
    pub start: usize,
    pub end: usize,
    pub fields: Vec<FieldIndex>,
}

pub fn parse_records(bytes: &[u8], dialect: CsvDialect) -> Result<Vec<RecordIndex>> {
    let bytes = match dialect.encoding {
        Encoding::Utf8 | Encoding::Utf8Bom => bytes,
        Encoding::Utf16Le => return Err(crate::CsvError::UnsupportedEncoding("UTF-16 LE")),
        Encoding::Utf16Be => return Err(crate::CsvError::UnsupportedEncoding("UTF-16 BE")),
    };

    let mut records = Vec::new();
    let mut record_start = if matches!(dialect.encoding, Encoding::Utf8Bom) {
        3.min(bytes.len())
    } else {
        0
    };
    let mut field_start = record_start;
    let mut fields = Vec::new();
    let mut in_quotes = false;
    let mut field_quoted = false;
    let mut at_field_start = true;
    let mut i = record_start;

    while i < bytes.len() {
        let b = bytes[i];
        if at_field_start && b == dialect.quote {
            field_quoted = true;
            in_quotes = true;
            at_field_start = false;
            i += 1;
            continue;
        }

        if in_quotes {
            if b == dialect.quote {
                if i + 1 < bytes.len() && bytes[i + 1] == dialect.quote {
                    i += 2;
                    continue;
                }
                in_quotes = false;
            }
            i += 1;
            continue;
        }

        if b == dialect.delimiter {
            fields.push(FieldIndex {
                start: field_start,
                end: i,
                quoted: field_quoted,
            });
            field_start = i + 1;
            field_quoted = false;
            at_field_start = true;
            i += 1;
            continue;
        }

        if b == b'\n' || b == b'\r' {
            fields.push(FieldIndex {
                start: field_start,
                end: i,
                quoted: field_quoted,
            });
            let record_end = i;
            if b == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                i += 1;
            }
            if record_end > record_start || !fields.is_empty() {
                records.push(RecordIndex {
                    start: record_start,
                    end: record_end,
                    fields,
                });
            }
            i += 1;
            record_start = i;
            field_start = i;
            fields = Vec::new();
            field_quoted = false;
            at_field_start = true;
            continue;
        }

        at_field_start = false;
        i += 1;
    }

    if field_start < bytes.len() || !fields.is_empty() || record_start < bytes.len() {
        fields.push(FieldIndex {
            start: field_start,
            end: bytes.len(),
            quoted: field_quoted,
        });
        records.push(RecordIndex {
            start: record_start,
            end: bytes.len(),
            fields,
        });
    }

    Ok(records)
}

pub fn decode_field(bytes: &[u8], field: FieldIndex, dialect: CsvDialect) -> String {
    let mut slice = &bytes[field.start..field.end];
    if field.quoted
        && slice.len() >= 2
        && slice[0] == dialect.quote
        && slice[slice.len() - 1] == dialect.quote
    {
        slice = &slice[1..slice.len() - 1];
    }

    let mut out = Vec::with_capacity(slice.len());
    let mut i = 0usize;
    while i < slice.len() {
        if slice[i] == dialect.quote && i + 1 < slice.len() && slice[i + 1] == dialect.quote {
            out.push(dialect.quote);
            i += 2;
        } else {
            out.push(slice[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn encode_field(value: &str, dialect: CsvDialect) -> String {
    let must_quote = value.as_bytes().iter().any(|byte| {
        *byte == dialect.delimiter || *byte == b'\n' || *byte == b'\r' || *byte == dialect.quote
    });
    if !must_quote {
        return value.to_string();
    }

    let mut encoded = String::with_capacity(value.len() + 2);
    encoded.push(dialect.quote as char);
    for ch in value.chars() {
        if ch as u32 == dialect.quote as u32 {
            encoded.push(dialect.quote as char);
            encoded.push(dialect.quote as char);
        } else {
            encoded.push(ch);
        }
    }
    encoded.push(dialect.quote as char);
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiline_quoted_field() {
        let dialect = CsvDialect::default();
        let records = parse_records(b"a,b\n\"one\ntwo\",3\n", dialect).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].fields.len(), 2);
        assert_eq!(
            decode_field(b"a,b\n\"one\ntwo\",3\n", records[1].fields[0], dialect),
            "one\ntwo"
        );
    }

    #[test]
    fn encodes_quotes_by_doubling() {
        assert_eq!(
            encode_field("a \"quote\"", CsvDialect::default()),
            "\"a \"\"quote\"\"\""
        );
    }
}
