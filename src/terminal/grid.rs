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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::cell::CellFlags;

    #[test]
    fn test_grid_new() {
        let grid = Grid::new(80, 24);
        assert_eq!(grid.col_count(), 80);
        assert_eq!(grid.row_count(), 24);
    }

    #[test]
    fn test_cell_get_set() {
        let mut grid = Grid::new(10, 5);

        // Test cell() - panics on out of bounds
        assert_eq!(grid.cell(0, 0).char, ' ');

        // Test cell_mut() - modify a cell
        {
            let cell = grid.cell_mut(2, 3);
            cell.char = 'X';
            cell.fg = crate::terminal::cell::PackedColor::indexed(1);
        }

        // Verify modification
        let cell = grid.cell(2, 3);
        assert_eq!(cell.char, 'X');
        assert!(cell.fg.is_indexed());
        assert_eq!(cell.fg.indexed_index(), 1);
    }

    #[test]
    fn test_cell_with_bounds_check() {
        let mut grid = Grid::new(10, 5);

        // Test get_cell() - returns None on out of bounds
        assert!(grid.get_cell(0, 0).is_some());
        assert!(grid.get_cell(100, 0).is_none());
        assert!(grid.get_cell(0, 100).is_none());

        // Test get_cell_mut() - returns None on out of bounds
        assert!(grid.get_cell_mut(0, 0).is_some());
        assert!(grid.get_cell_mut(100, 0).is_none());
        assert!(grid.get_cell_mut(0, 100).is_none());

        // Modify through get_cell_mut()
        if let Some(cell) = grid.get_cell_mut(1, 1) {
            cell.char = 'A';
            cell.flags.set(CellFlags::BOLD, true);
        }

        let cell = grid.cell(1, 1);
        assert_eq!(cell.char, 'A');
        assert!(cell.flags.contains(CellFlags::BOLD));
    }

    #[test]
    fn test_resize_larger() {
        let mut grid = Grid::new(10, 5);

        // Fill grid with content
        for row in 0..5 {
            for col in 0..10 {
                let cell = grid.cell_mut(row, col);
                cell.char = char::from(b'A' + (row * 10 + col) as u8);
            }
        }

        // Resize larger
        grid.resize(20, 10);

        assert_eq!(grid.col_count(), 20);
        assert_eq!(grid.row_count(), 10);

        // Original content should be preserved
        for row in 0..5 {
            for col in 0..10 {
                let expected = char::from(b'A' + (row * 10 + col) as u8);
                assert_eq!(grid.cell(row, col).char, expected);
            }
        }

        // New cells should be blank
        for row in 0..5 {
            for col in 10..20 {
                assert_eq!(grid.cell(row, col).char, ' ');
            }
        }
        for row in 5..10 {
            for col in 0..20 {
                assert_eq!(grid.cell(row, col).char, ' ');
            }
        }
    }

    #[test]
    fn test_resize_smaller() {
        let mut grid = Grid::new(20, 10);

        // Fill grid with content
        for row in 0..10 {
            for col in 0..20 {
                let cell = grid.cell_mut(row, col);
                cell.char = char::from(b'A' + (row * 20 + col) as u8 % 26);
            }
        }

        // Resize smaller
        grid.resize(10, 5);

        assert_eq!(grid.col_count(), 10);
        assert_eq!(grid.row_count(), 5);

        // Remaining content should be preserved
        for row in 0..5 {
            for col in 0..10 {
                let expected = char::from(b'A' + (row * 20 + col) as u8 % 26);
                assert_eq!(grid.cell(row, col).char, expected);
            }
        }
    }

    #[test]
    fn test_resize_same_size() {
        let mut grid = Grid::new(10, 5);

        // Fill grid with content
        for row in 0..5 {
            for col in 0..10 {
                let cell = grid.cell_mut(row, col);
                cell.char = char::from(b'0' + col as u8 % 10);
            }
        }

        // Resize to same size
        grid.resize(10, 5);

        // Content should be unchanged
        for row in 0..5 {
            for col in 0..10 {
                let expected = char::from(b'0' + col as u8 % 10);
                assert_eq!(grid.cell(row, col).char, expected);
            }
        }
    }

    #[test]
    fn test_clear() {
        let mut grid = Grid::new(10, 5);

        // Fill grid with content
        for row in 0..5 {
            for col in 0..10 {
                let cell = grid.cell_mut(row, col);
                cell.char = 'X';
                cell.fg = crate::terminal::cell::PackedColor::indexed(1);
                cell.flags.set(CellFlags::BOLD, true);
            }
        }

        // Clear grid
        grid.clear();

        // All cells should be blank
        for row in 0..5 {
            for col in 0..10 {
                let cell = grid.cell(row, col);
                assert_eq!(cell.char, ' ');
                assert!(cell.fg.is_default());
                assert!(!cell.flags.contains(CellFlags::BOLD));
            }
        }
    }

    #[test]
    fn test_row_access() {
        let mut grid = Grid::new(10, 5);

        // Modify a row
        {
            let row = grid.row_mut(2);
            for cell in row.cells_mut() {
                cell.char = 'R';
            }
        }

        // Verify through row()
        let row = grid.row(2);
        for cell in row.cells() {
            assert_eq!(cell.char, 'R');
        }

        // Other rows should still be blank
        let row = grid.row(1);
        for cell in row.cells() {
            assert_eq!(cell.char, ' ');
        }
    }

    #[test]
    fn test_empty_grid() {
        let grid = Grid::new(0, 0);
        assert_eq!(grid.col_count(), 0);
        assert_eq!(grid.row_count(), 0);
    }

    #[test]
    fn test_grid_clone() {
        let mut grid1 = Grid::new(10, 5);
        grid1.cell_mut(0, 0).char = 'A';

        let grid2 = grid1.clone();
        assert_eq!(grid2.cell(0, 0).char, 'A');

        // Modifying grid1 should not affect grid2
        grid1.cell_mut(0, 0).char = 'B';
        assert_eq!(grid2.cell(0, 0).char, 'A');
    }
}