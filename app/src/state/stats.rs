//! Selection statistics: the metrics shown in the status bar's selection
//! popup (counts, distinct, and numeric aggregates).

use super::EditableState;
use crate::selection::Selection;

// Above this many cells, skip full stats computation to keep the main thread responsive.
pub(super) const STATS_CELL_LIMIT: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMetric {
    #[default]
    Values,
    Blanks,
    Distinct,
    Total,
    Minimum,
    Average,
    Median,
    Maximum,
    Summary,
}

impl SelectionMetric {
    pub fn label(self) -> &'static str {
        match self {
            Self::Values => "Values",
            Self::Blanks => "Blanks",
            Self::Distinct => "Distinct",
            Self::Total => "Total",
            Self::Minimum => "Minimum",
            Self::Average => "Average",
            Self::Median => "Median",
            Self::Maximum => "Maximum",
            Self::Summary => "Summary",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionStat {
    pub metric: SelectionMetric,
    pub value: String,
}

impl SelectionStat {
    pub fn display_text(&self) -> String {
        format!("{}: {}", self.metric.label(), self.value)
    }
}

impl EditableState {
    pub fn selection_stats(&self) -> Vec<SelectionStat> {
        let Some(doc) = self.document.as_ref() else {
            return Vec::new();
        };
        let mut values = Vec::new();
        let row_count = doc.row_count();
        let column_count = doc.column_count();
        if row_count == 0 || column_count == 0 {
            return Vec::new();
        }

        // Compute cell count cheaply (no file reads) before deciding whether to iterate.
        let cell_count: usize = match &self.selection {
            Selection::Cells { cells, .. } => cells.len(),
            Selection::Cell(rect) => {
                let (top, left, bottom, right) = rect.bounds();
                (bottom.min(row_count - 1).saturating_sub(top) + 1)
                    .saturating_mul(right.min(column_count - 1).saturating_sub(left) + 1)
            }
            Selection::Row { anchor, focus } => {
                let top = (*anchor).min(*focus);
                let bottom = (*anchor).max(*focus).min(row_count - 1);
                (bottom.saturating_sub(top) + 1).saturating_mul(column_count)
            }
            Selection::Column { anchor, focus } => {
                let left = (*anchor).min(*focus);
                let right = (*anchor).max(*focus).min(column_count - 1);
                row_count.saturating_mul(right.saturating_sub(left) + 1)
            }
            Selection::All => row_count.saturating_mul(column_count),
        };
        if cell_count > STATS_CELL_LIMIT {
            return vec![SelectionStat {
                metric: SelectionMetric::Total,
                value: cell_count.to_string(),
            }];
        }

        match &self.selection {
            Selection::Cells { cells, .. } => {
                for cell in cells {
                    if cell.row < row_count && cell.column < column_count {
                        values.push(doc.cell(cell.row, cell.column).unwrap_or_default());
                    }
                }
            }
            Selection::Cell(rect) => {
                let (top, left, bottom, right) = rect.bounds();
                for row in top..=bottom.min(row_count - 1) {
                    for column in left..=right.min(column_count - 1) {
                        values.push(doc.cell(row, column).unwrap_or_default());
                    }
                }
            }
            Selection::Row { anchor, focus } => {
                let top = (*anchor).min(*focus);
                let bottom = (*anchor).max(*focus).min(row_count - 1);
                for row in top..=bottom {
                    for column in 0..column_count {
                        values.push(doc.cell(row, column).unwrap_or_default());
                    }
                }
            }
            Selection::Column { anchor, focus } => {
                let left = (*anchor).min(*focus);
                let right = (*anchor).max(*focus).min(column_count - 1);
                for row in 0..row_count {
                    for column in left..=right {
                        values.push(doc.cell(row, column).unwrap_or_default());
                    }
                }
            }
            Selection::All => {
                for row in 0..row_count {
                    for column in 0..column_count {
                        values.push(doc.cell(row, column).unwrap_or_default());
                    }
                }
            }
        }

        compute_stats(values)
    }
}

fn compute_stats(values: Vec<String>) -> Vec<SelectionStat> {
    let total = values.len();
    if total <= 1 {
        return Vec::new();
    }

    let mut filled = 0;
    let mut distinct = std::collections::HashSet::new();
    let mut numbers = Vec::new();
    let mut all_filled_values_are_numbers = true;

    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }

        filled += 1;
        distinct.insert(trimmed.to_string());
        match trimmed.parse::<f64>() {
            Ok(number) if number.is_finite() => numbers.push(number),
            _ => all_filled_values_are_numbers = false,
        }
    }

    let empty = total - filled;
    let mut stats = vec![
        SelectionStat {
            metric: SelectionMetric::Values,
            value: filled.to_string(),
        },
        SelectionStat {
            metric: SelectionMetric::Blanks,
            value: empty.to_string(),
        },
        SelectionStat {
            metric: SelectionMetric::Distinct,
            value: distinct.len().to_string(),
        },
        SelectionStat {
            metric: SelectionMetric::Total,
            value: total.to_string(),
        },
    ];

    if filled > 0 && all_filled_values_are_numbers {
        numbers.sort_by(|a, b| a.total_cmp(b));
        let sum: f64 = numbers.iter().sum();
        let average = sum / numbers.len() as f64;
        let median = if numbers.len() % 2 == 0 {
            let upper = numbers.len() / 2;
            (numbers[upper - 1] + numbers[upper]) / 2.0
        } else {
            numbers[numbers.len() / 2]
        };
        stats.extend([
            SelectionStat {
                metric: SelectionMetric::Minimum,
                value: format_stat_number(*numbers.first().unwrap_or(&0.0)),
            },
            SelectionStat {
                metric: SelectionMetric::Average,
                value: format_stat_number(average),
            },
            SelectionStat {
                metric: SelectionMetric::Median,
                value: format_stat_number(median),
            },
            SelectionStat {
                metric: SelectionMetric::Maximum,
                value: format_stat_number(*numbers.last().unwrap_or(&0.0)),
            },
            SelectionStat {
                metric: SelectionMetric::Summary,
                value: format_stat_number(sum),
            },
        ]);
    }

    stats
}

fn format_stat_number(value: f64) -> String {
    let value = if value.abs() < 0.000_000_001 {
        0.0
    } else {
        value
    };
    if value.fract().abs() < 0.000_000_001 {
        return format!("{value:.0}");
    }

    let mut text = format!("{value:.4}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}
