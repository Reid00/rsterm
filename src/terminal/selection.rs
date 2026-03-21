// src/terminal/selection.rs

/// Position in terminal grid (row, col)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    pub row: usize,
    pub col: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SelectionMode {
    Stream,
    Line,
    Word,
}

#[derive(Clone, Debug)]
pub struct Selection {
    pub start: Position,
    pub end: Position,
    pub mode: SelectionMode,
}

impl Selection {
    pub fn new(start: Position, mode: SelectionMode) -> Self {
        Self { start, end: start, mode }
    }

    pub fn update(&mut self, end: Position) {
        self.end = end;
    }

    pub fn contains(&self, pos: &Position) -> bool {
        let (min_row, max_row) = if self.start.row <= self.end.row {
            (self.start.row, self.end.row)
        } else {
            (self.end.row, self.start.row)
        };

        if pos.row < min_row || pos.row > max_row {
            return false;
        }

        if self.start.row == self.end.row {
            let (min_col, max_col) = if self.start.col <= self.end.col {
                (self.start.col, self.end.col)
            } else {
                (self.end.col, self.start.col)
            };
            return pos.col >= min_col && pos.col <= max_col;
        }

        if pos.row == min_row && pos.row != max_row {
            return pos.col >= self.start.col;
        }
        if pos.row == max_row && pos.row != min_row {
            return pos.col <= self.end.col;
        }
        true
    }
}