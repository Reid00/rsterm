// src/terminal/cursor.rs
#[derive(Clone, Copy, Debug, Default)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub saved_row: usize,
    pub saved_col: usize,
}

impl Cursor {
    pub fn new() -> Self { Self::default() }

    pub fn move_to(&mut self, row: usize, col: usize, max_row: usize, max_col: usize) {
        self.row = row.min(max_row.saturating_sub(1));
        self.col = col.min(max_col.saturating_sub(1));
    }

    pub fn save(&mut self) { self.saved_row = self.row; self.saved_col = self.col; }
    pub fn restore(&mut self) { self.row = self.saved_row; self.col = self.saved_col; }
}