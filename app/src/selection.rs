use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub row: usize,
    pub column: usize,
}

impl Ord for Cell {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.row, self.column).cmp(&(other.row, other.column))
    }
}

impl PartialOrd for Cell {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectSelection {
    pub anchor: Cell,
    pub focus: Cell,
}

#[allow(dead_code)]
impl RectSelection {
    pub fn new(cell: Cell) -> Self {
        Self {
            anchor: cell,
            focus: cell,
        }
    }

    pub fn extend_to(&mut self, cell: Cell) {
        self.focus = cell;
    }

    pub fn bounds(self) -> (usize, usize, usize, usize) {
        let top = self.anchor.row.min(self.focus.row);
        let bottom = self.anchor.row.max(self.focus.row);
        let left = self.anchor.column.min(self.focus.column);
        let right = self.anchor.column.max(self.focus.column);
        (top, left, bottom, right)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Selection {
    Cells {
        anchor: Cell,
        active: Cell,
        cells: BTreeSet<Cell>,
    },
    Cell(RectSelection),
    Row {
        anchor: usize,
        focus: usize,
    },
    Column {
        anchor: usize,
        focus: usize,
    },
    All,
}

impl Default for Selection {
    fn default() -> Self {
        Selection::single_cell(Cell { row: 0, column: 0 })
    }
}

#[allow(dead_code)]
impl Selection {
    pub fn single_cell(cell: Cell) -> Self {
        Self::Cells {
            anchor: cell,
            active: cell,
            cells: BTreeSet::from([cell]),
        }
    }

    pub fn active_cell(&self) -> Cell {
        match self {
            Selection::Cells { active, .. } => *active,
            Selection::Cell(rect) => rect.focus,
            Selection::Row { focus, .. } => Cell {
                row: *focus,
                column: 0,
            },
            Selection::Column { focus, .. } => Cell {
                row: 0,
                column: *focus,
            },
            Selection::All => Cell { row: 0, column: 0 },
        }
    }

    pub fn anchor_cell(&self) -> Cell {
        match self {
            Selection::Cells { anchor, .. } => *anchor,
            Selection::Cell(rect) => rect.anchor,
            _ => self.active_cell(),
        }
    }

    pub fn contains_cell(&self, row: usize, column: usize) -> bool {
        match self {
            Selection::Cells { cells, .. } => cells.contains(&Cell { row, column }),
            Selection::Cell(rect) => {
                let (top, left, bottom, right) = rect.bounds();
                (top..=bottom).contains(&row) && (left..=right).contains(&column)
            }
            Selection::Row { anchor, focus } => {
                ((*anchor).min(*focus)..=(*anchor).max(*focus)).contains(&row)
            }
            Selection::Column { anchor, focus } => {
                ((*anchor).min(*focus)..=(*anchor).max(*focus)).contains(&column)
            }
            Selection::All => true,
        }
    }

    pub fn contains_row(&self, row: usize) -> bool {
        match self {
            Selection::Row { anchor, focus } => {
                ((*anchor).min(*focus)..=(*anchor).max(*focus)).contains(&row)
            }
            Selection::All => true,
            _ => false,
        }
    }

    pub fn move_by(&mut self, rows: isize, columns: isize, row_limit: usize, column_limit: usize) {
        let active = self.active_cell();
        let row = active
            .row
            .saturating_add_signed(rows)
            .min(row_limit.saturating_sub(1));
        let column = active
            .column
            .saturating_add_signed(columns)
            .min(column_limit.saturating_sub(1));
        *self = Selection::single_cell(Cell { row, column });
    }

    pub fn extend_by(
        &mut self,
        rows: isize,
        columns: isize,
        row_limit: usize,
        column_limit: usize,
    ) {
        let active = self.active_cell();
        let row = active
            .row
            .saturating_add_signed(rows)
            .min(row_limit.saturating_sub(1));
        let column = active
            .column
            .saturating_add_signed(columns)
            .min(column_limit.saturating_sub(1));
        self.select_rect_to(Cell { row, column });
    }

    pub fn toggle_cell(&mut self, cell: Cell) -> bool {
        match self {
            Selection::Cells {
                anchor,
                active,
                cells,
            } => {
                let selected = if cells.remove(&cell) {
                    false
                } else {
                    cells.insert(cell);
                    true
                };
                *anchor = cell;
                *active = cell;
                selected
            }
            _ => {
                *self = Selection::single_cell(cell);
                true
            }
        }
    }

    pub fn set_cell_selected(&mut self, cell: Cell, selected: bool) {
        match self {
            Selection::Cells {
                anchor: _,
                active,
                cells,
            } => {
                if selected {
                    cells.insert(cell);
                } else {
                    cells.remove(&cell);
                }
                *active = cell;
            }
            _ => {
                if selected {
                    *self = Selection::single_cell(cell);
                }
            }
        }
    }

    pub fn select_rect_to(&mut self, focus: Cell) {
        let anchor = self.anchor_cell();
        let cells = cells_in_rect(anchor, focus);
        *self = Selection::Cells {
            anchor,
            active: focus,
            cells,
        };
    }

    pub fn set_rect_selected(&mut self, anchor: Cell, focus: Cell, selected: bool) {
        let mut cells = match self {
            Selection::Cells { cells, .. } => cells.clone(),
            _ => BTreeSet::new(),
        };
        for cell in cells_in_rect(anchor, focus) {
            if selected {
                cells.insert(cell);
            } else {
                cells.remove(&cell);
            }
        }
        *self = Selection::Cells {
            anchor,
            active: focus,
            cells,
        };
    }

    pub fn cells(&self) -> Vec<Cell> {
        match self {
            Selection::Cells { cells, .. } => cells.iter().copied().collect(),
            Selection::Cell(rect) => {
                let (top, left, bottom, right) = rect.bounds();
                (top..=bottom)
                    .flat_map(|row| (left..=right).map(move |column| Cell { row, column }))
                    .collect()
            }
            _ => Vec::new(),
        }
    }
}

fn cells_in_rect(anchor: Cell, focus: Cell) -> BTreeSet<Cell> {
    let top = anchor.row.min(focus.row);
    let bottom = anchor.row.max(focus.row);
    let left = anchor.column.min(focus.column);
    let right = anchor.column.max(focus.column);
    (top..=bottom)
        .flat_map(|row| (left..=right).map(move |column| Cell { row, column }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_rectangular_bounds() {
        let mut selection = RectSelection::new(Cell { row: 5, column: 4 });
        selection.extend_to(Cell { row: 1, column: 7 });
        assert_eq!(selection.bounds(), (1, 4, 5, 7));
    }

    #[test]
    fn toggles_individual_cells() {
        let mut selection = Selection::single_cell(Cell { row: 1, column: 1 });
        assert!(!selection.toggle_cell(Cell { row: 1, column: 1 }));
        assert!(!selection.contains_cell(1, 1));
        assert!(selection.toggle_cell(Cell { row: 2, column: 3 }));
        assert!(selection.contains_cell(2, 3));
    }
}
