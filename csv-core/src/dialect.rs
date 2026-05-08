#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsvDialect {
    pub delimiter: u8,
    pub quote: u8,
    pub escape: Option<u8>,
    pub line_ending: LineEnding,
    pub encoding: Encoding,
}

impl Default for CsvDialect {
    fn default() -> Self {
        Self {
            delimiter: b',',
            quote: b'"',
            escape: None,
            line_ending: LineEnding::Lf,
            encoding: Encoding::Utf8,
        }
    }
}

pub fn detect_dialect(bytes: &[u8]) -> CsvDialect {
    let encoding = detect_encoding(bytes);
    let sample = if matches!(encoding, Encoding::Utf8Bom) && bytes.len() >= 3 {
        &bytes[3..]
    } else {
        bytes
    };
    let sample = &sample[..sample.len().min(1024 * 1024)];

    CsvDialect {
        delimiter: detect_delimiter(sample),
        quote: b'"',
        escape: None,
        line_ending: detect_line_ending(sample),
        encoding,
    }
}

fn detect_encoding(bytes: &[u8]) -> Encoding {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Encoding::Utf8Bom
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        Encoding::Utf16Le
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        Encoding::Utf16Be
    } else {
        Encoding::Utf8
    }
}

fn detect_line_ending(bytes: &[u8]) -> LineEnding {
    let mut crlf = 0usize;
    let mut lf = 0usize;
    let mut cr = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if i + 1 < bytes.len() && bytes[i + 1] == b'\n' => {
                crlf += 1;
                i += 2;
            }
            b'\r' => {
                cr += 1;
                i += 1;
            }
            b'\n' => {
                lf += 1;
                i += 1;
            }
            _ => i += 1,
        }
    }

    if crlf >= lf && crlf >= cr && crlf > 0 {
        LineEnding::CrLf
    } else if cr > lf {
        LineEnding::Cr
    } else {
        LineEnding::Lf
    }
}

fn detect_delimiter(bytes: &[u8]) -> u8 {
    const CANDIDATES: [u8; 5] = [b',', b'\t', b';', b'|', b':'];
    let mut scores = [(0usize, 0usize); CANDIDATES.len()];
    let mut in_quotes = false;
    let mut current_counts = [0usize; CANDIDATES.len()];
    let mut line_has_data = false;
    let mut lines = 0usize;
    let mut i = 0usize;

    while i < bytes.len() && lines < 64 {
        let byte = bytes[i];
        if byte == b'"' {
            if in_quotes && i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                i += 2;
                continue;
            }
            in_quotes = !in_quotes;
        } else if !in_quotes {
            if byte == b'\n' || byte == b'\r' {
                if line_has_data {
                    for (idx, count) in current_counts.iter().copied().enumerate() {
                        scores[idx].0 += count;
                        if count > 0 {
                            scores[idx].1 += 1;
                        }
                    }
                    lines += 1;
                }
                current_counts = [0; CANDIDATES.len()];
                line_has_data = false;
                if byte == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    i += 1;
                }
            } else {
                line_has_data = true;
                if let Some(idx) = CANDIDATES.iter().position(|candidate| *candidate == byte) {
                    current_counts[idx] += 1;
                }
            }
        } else {
            line_has_data = true;
        }
        i += 1;
    }

    CANDIDATES
        .iter()
        .copied()
        .enumerate()
        .max_by_key(|(idx, _)| {
            let (total, populated_lines) = scores[*idx];
            populated_lines * 1_000 + total
        })
        .map(|(_, delimiter)| delimiter)
        .unwrap_or(b',')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_semicolon_and_crlf() {
        let dialect = detect_dialect(b"a;b;c\r\n1;2;3\r\n");
        assert_eq!(dialect.delimiter, b';');
        assert_eq!(dialect.line_ending, LineEnding::CrLf);
    }

    #[test]
    fn ignores_delimiters_inside_quotes() {
        let dialect = detect_dialect(
            br#""a,b";c;d
"1,2";3;4
"#,
        );
        assert_eq!(dialect.delimiter, b';');
    }
}
