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
        // Normalize start and end based on actual position (handles backward selection)
        let (start, end) = if self.start.row < self.end.row
            || (self.start.row == self.end.row && self.start.col <= self.end.col)
        {
            (&self.start, &self.end)
        } else {
            (&self.end, &self.start)
        };

        if pos.row < start.row || pos.row > end.row {
            return false;
        }

        if start.row == end.row {
            return pos.col >= start.col && pos.col <= end.col;
        }

        // First row: from start.col to end of line
        if pos.row == start.row {
            return pos.col >= start.col;
        }

        // Last row: from beginning to end.col
        if pos.row == end.row {
            return pos.col <= end.col;
        }

        // Middle rows: entire row is selected
        true
    }
}