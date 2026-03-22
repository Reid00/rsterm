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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selection_new() {
        let pos = Position { row: 5, col: 10 };
        let selection = Selection::new(pos, SelectionMode::Stream);

        assert_eq!(selection.start, pos);
        assert_eq!(selection.end, pos);
        assert_eq!(selection.mode, SelectionMode::Stream);
    }

    #[test]
    fn test_selection_update() {
        let start = Position { row: 2, col: 5 };
        let mut selection = Selection::new(start, SelectionMode::Stream);

        let end = Position { row: 5, col: 10 };
        selection.update(end);

        assert_eq!(selection.start, start);
        assert_eq!(selection.end, end);
    }

    #[test]
    fn test_contains_single_position() {
        let pos = Position { row: 5, col: 10 };
        let selection = Selection::new(pos, SelectionMode::Stream);

        // Should contain its own position
        assert!(selection.contains(&pos));

        // Should not contain other positions
        assert!(!selection.contains(&Position { row: 5, col: 9 }));
        assert!(!selection.contains(&Position { row: 5, col: 11 }));
        assert!(!selection.contains(&Position { row: 4, col: 10 }));
        assert!(!selection.contains(&Position { row: 6, col: 10 }));
    }

    #[test]
    fn test_contains_forward_selection() {
        let mut selection = Selection::new(Position { row: 2, col: 5 }, SelectionMode::Stream);
        selection.update(Position { row: 5, col: 10 });

        // Start and end positions
        assert!(selection.contains(&Position { row: 2, col: 5 }));
        assert!(selection.contains(&Position { row: 5, col: 10 }));

        // Positions in between on start row
        assert!(selection.contains(&Position { row: 2, col: 6 }));
        assert!(selection.contains(&Position { row: 2, col: 100 }));

        // Positions in between on end row
        assert!(selection.contains(&Position { row: 5, col: 0 }));
        assert!(selection.contains(&Position { row: 5, col: 9 }));

        // Positions on middle rows (entire row selected)
        assert!(selection.contains(&Position { row: 3, col: 0 }));
        assert!(selection.contains(&Position { row: 3, col: 50 }));
        assert!(selection.contains(&Position { row: 4, col: 0 }));
        assert!(selection.contains(&Position { row: 4, col: 50 }));

        // Positions outside selection
        assert!(!selection.contains(&Position { row: 1, col: 100 }));
        assert!(!selection.contains(&Position { row: 2, col: 4 }));
        assert!(!selection.contains(&Position { row: 5, col: 11 }));
        assert!(!selection.contains(&Position { row: 6, col: 0 }));
    }

    #[test]
    fn test_contains_backward_selection() {
        let mut selection = Selection::new(Position { row: 5, col: 10 }, SelectionMode::Stream);
        selection.update(Position { row: 2, col: 5 });

        // Start and end positions (swapped in selection, but contains should still work)
        assert!(selection.contains(&Position { row: 2, col: 5 }));
        assert!(selection.contains(&Position { row: 5, col: 10 }));

        // Positions in between on start row (now row 2)
        assert!(selection.contains(&Position { row: 2, col: 6 }));
        assert!(selection.contains(&Position { row: 2, col: 100 }));

        // Positions in between on end row (now row 5)
        assert!(selection.contains(&Position { row: 5, col: 0 }));
        assert!(selection.contains(&Position { row: 5, col: 9 }));

        // Positions on middle rows (entire row selected)
        assert!(selection.contains(&Position { row: 3, col: 0 }));
        assert!(selection.contains(&Position { row: 3, col: 50 }));
        assert!(selection.contains(&Position { row: 4, col: 0 }));
        assert!(selection.contains(&Position { row: 4, col: 50 }));

        // Positions outside selection
        assert!(!selection.contains(&Position { row: 1, col: 100 }));
        assert!(!selection.contains(&Position { row: 2, col: 4 }));
        assert!(!selection.contains(&Position { row: 5, col: 11 }));
        assert!(!selection.contains(&Position { row: 6, col: 0 }));
    }

    #[test]
    fn test_contains_same_row_forward() {
        let mut selection = Selection::new(Position { row: 5, col: 3 }, SelectionMode::Stream);
        selection.update(Position { row: 5, col: 10 });

        // Within selection
        assert!(selection.contains(&Position { row: 5, col: 3 }));
        assert!(selection.contains(&Position { row: 5, col: 5 }));
        assert!(selection.contains(&Position { row: 5, col: 10 }));

        // Outside selection
        assert!(!selection.contains(&Position { row: 5, col: 2 }));
        assert!(!selection.contains(&Position { row: 5, col: 11 }));
        assert!(!selection.contains(&Position { row: 4, col: 5 }));
        assert!(!selection.contains(&Position { row: 6, col: 5 }));
    }

    #[test]
    fn test_contains_same_row_backward() {
        let mut selection = Selection::new(Position { row: 5, col: 10 }, SelectionMode::Stream);
        selection.update(Position { row: 5, col: 3 });

        // Within selection (backward selection should still work)
        assert!(selection.contains(&Position { row: 5, col: 3 }));
        assert!(selection.contains(&Position { row: 5, col: 5 }));
        assert!(selection.contains(&Position { row: 5, col: 10 }));

        // Outside selection
        assert!(!selection.contains(&Position { row: 5, col: 2 }));
        assert!(!selection.contains(&Position { row: 5, col: 11 }));
        assert!(!selection.contains(&Position { row: 4, col: 5 }));
        assert!(!selection.contains(&Position { row: 6, col: 5 }));
    }

    #[test]
    fn test_contains_edge_case_start_equals_end() {
        let mut selection = Selection::new(Position { row: 5, col: 10 }, SelectionMode::Stream);
        selection.update(Position { row: 5, col: 10 });

        // Should only contain that one position
        assert!(selection.contains(&Position { row: 5, col: 10 }));
        assert!(!selection.contains(&Position { row: 5, col: 9 }));
        assert!(!selection.contains(&Position { row: 5, col: 11 }));
        assert!(!selection.contains(&Position { row: 4, col: 10 }));
        assert!(!selection.contains(&Position { row: 6, col: 10 }));
    }

    #[test]
    fn test_contains_adjacent_rows() {
        let mut selection = Selection::new(Position { row: 2, col: 5 }, SelectionMode::Stream);
        selection.update(Position { row: 3, col: 10 });

        // First row: from col 5 to end
        assert!(selection.contains(&Position { row: 2, col: 5 }));
        assert!(selection.contains(&Position { row: 2, col: 79 }));

        // Second row: from start to col 10
        assert!(selection.contains(&Position { row: 3, col: 0 }));
        assert!(selection.contains(&Position { row: 3, col: 10 }));

        // Outside
        assert!(!selection.contains(&Position { row: 2, col: 4 }));
        assert!(!selection.contains(&Position { row: 3, col: 11 }));
        assert!(!selection.contains(&Position { row: 1, col: 50 }));
        assert!(!selection.contains(&Position { row: 4, col: 5 }));
    }

    #[test]
    fn test_selection_modes() {
        let pos = Position { row: 0, col: 0 };

        let stream_selection = Selection::new(pos, SelectionMode::Stream);
        assert_eq!(stream_selection.mode, SelectionMode::Stream);

        let line_selection = Selection::new(pos, SelectionMode::Line);
        assert_eq!(line_selection.mode, SelectionMode::Line);

        let word_selection = Selection::new(pos, SelectionMode::Word);
        assert_eq!(word_selection.mode, SelectionMode::Word);
    }

    #[test]
    fn test_selection_clone() {
        let mut selection = Selection::new(Position { row: 2, col: 5 }, SelectionMode::Stream);
        selection.update(Position { row: 5, col: 10 });

        let cloned = selection.clone();
        assert_eq!(cloned.start, selection.start);
        assert_eq!(cloned.end, selection.end);
        assert_eq!(cloned.mode, selection.mode);
    }

    #[test]
    fn test_position_default() {
        let pos = Position::default();
        assert_eq!(pos.row, 0);
        assert_eq!(pos.col, 0);
    }

    #[test]
    fn test_position_equality() {
        let pos1 = Position { row: 5, col: 10 };
        let pos2 = Position { row: 5, col: 10 };
        let pos3 = Position { row: 5, col: 11 };

        assert_eq!(pos1, pos2);
        assert_ne!(pos1, pos3);
    }
}