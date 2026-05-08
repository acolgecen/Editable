#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub row: usize,
    pub column: usize,
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
    Cell(RectSelection),
    Row { anchor: usize, focus: usize },
    Column { anchor: usize, focus: usize },
    All,
}

impl Default for Selection {
    fn default() -> Self {
        Selection::Cell(RectSelection::new(Cell { row: 0, column: 0 }))
    }
}

#[allow(dead_code)]
impl Selection {
    pub fn active_cell(&self) -> Cell {
        match self {
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
        *self = Selection::Cell(RectSelection::new(Cell { row, column }));
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
        match self {
            Selection::Cell(rect) => rect.extend_to(Cell { row, column }),
            _ => *self = Selection::Cell(RectSelection::new(Cell { row, column })),
        }
    }
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
}
