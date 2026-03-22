// src/terminal/grid.rs
use crate::terminal::cell::Cell;

#[derive(Clone, Debug)]
pub struct Row {
    cells: Vec<Cell>,
}

impl Row {
    pub fn new(cols: usize) -> Self {
        Self { cells: vec![Cell::blank(); cols] }
    }

    pub fn cells(&self) -> &[Cell] { &self.cells }
    pub fn cells_mut(&mut self) -> &mut [Cell] { &mut self.cells }
    pub fn resize(&mut self, cols: usize) {
        if cols > self.cells.len() {
            self.cells.extend(std::iter::repeat(Cell::blank()).take(cols - self.cells.len()));
        } else {
            self.cells.truncate(cols);
        }
    }
}

#[derive(Clone, Debug)]
pub struct Grid {
    rows: Vec<Row>,
    cols: usize,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            rows: (0..rows).map(|_| Row::new(cols)).collect(),
            cols,
        }
    }

    pub fn row_count(&self) -> usize { self.rows.len() }
    pub fn col_count(&self) -> usize { self.cols }

    pub fn row(&self, idx: usize) -> &Row { &self.rows[idx] }
    pub fn row_mut(&mut self, idx: usize) -> &mut Row { &mut self.rows[idx] }

    /// Get a cell without bounds checking. Panics if out of bounds.
    pub fn cell(&self, row: usize, col: usize) -> &Cell {
        &self.rows[row].cells[col]
    }

    /// Get a mutable cell without bounds checking. Panics if out of bounds.
    pub fn cell_mut(&mut self, row: usize, col: usize) -> &mut Cell {
        &mut self.rows[row].cells[col]
    }

    /// Get a cell with bounds checking. Returns None if out of bounds.
    pub fn get_cell(&self, row: usize, col: usize) -> Option<&Cell> {
        self.rows.get(row).and_then(|r| r.cells.get(col))
    }

    /// Get a mutable cell with bounds checking. Returns None if out of bounds.
    pub fn get_cell_mut(&mut self, row: usize, col: usize) -> Option<&mut Cell> {
        self.rows.get_mut(row).and_then(|r| r.cells.get_mut(col))
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        if rows > self.rows.len() {
            for row in &mut self.rows {
                row.resize(cols);
            }
            self.rows.extend(std::iter::repeat_with(|| Row::new(cols)).take(rows - self.rows.len()));
        } else {
            self.rows.truncate(rows);
            for row in &mut self.rows {
                row.resize(cols);
            }
        }
    }

    pub fn clear(&mut self) {
        for row in &mut self.rows {
            for cell in row.cells_mut() {
                *cell = Cell::blank();
            }
        }
    }
}