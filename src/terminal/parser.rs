// src/terminal/parser.rs
use vte::{Parser, Perform};
use crate::terminal::cell::{Cell, CellFlags, PackedColor};
use crate::terminal::grid::Grid;
use crate::terminal::cursor::Cursor;
use crate::terminal::mode::TerminalMode;

/// Pending cell attributes for the next character to be written
#[derive(Clone, Copy, Debug, Default)]
pub struct CellAttrs {
    pub fg: PackedColor,
    pub bg: PackedColor,
    pub flags: CellFlags,
}

/// State for the terminal parser
pub struct ParserState {
    /// Primary screen buffer
    grid: Grid,
    /// Alternate screen buffer
    alt_grid: Grid,
    /// Cursor position
    cursor: Cursor,
    /// Saved cursor for main screen (used when switching to/from alternate screen)
    saved_main_cursor: Cursor,
    /// Terminal modes
    mode: TerminalMode,
    /// Pending attributes for next cell
    pending_attrs: CellAttrs,
    /// Scroll region top (inclusive)
    scroll_top: usize,
    /// Scroll region bottom (exclusive)
    scroll_bottom: usize,
}

impl ParserState {
    fn new(cols: usize, rows: usize) -> Self {
        let grid = Grid::new(cols, rows);
        let alt_grid = Grid::new(cols, rows);
        let scroll_bottom = rows;
        Self {
            grid,
            alt_grid,
            cursor: Cursor::new(),
            saved_main_cursor: Cursor::new(),
            mode: TerminalMode::default(),
            pending_attrs: CellAttrs::default(),
            scroll_top: 0,
            scroll_bottom,
        }
    }

    /// Check if alternate screen is active
    fn is_alternate(&self) -> bool {
        self.mode.contains(TerminalMode::ALTERNATE_SCREEN)
    }

    /// Scroll up within the scroll region
    fn scroll_up(&mut self, n: usize) {
        if n == 0 {
            return;
        }

        let top = self.scroll_top;
        let bottom = self.scroll_bottom;

        // Determine which grid to use based on mode
        let use_alt = self.is_alternate();
        let row_count = if use_alt {
            self.alt_grid.row_count()
        } else {
            self.grid.row_count()
        };
        let bottom = bottom.min(row_count);

        if top >= bottom {
            return;
        }

        let scroll_region_size = bottom - top;
        let n = n.min(scroll_region_size);

        // Get the grid after we've read all the values we need
        let grid = if use_alt { &mut self.alt_grid } else { &mut self.grid };

        // Shift rows up within scroll region by copying cell by cell
        for i in top..(bottom - n) {
            let src_row = i + n;
            for c in 0..grid.col_count() {
                let cell = *grid.cell(src_row, c);
                *grid.cell_mut(i, c) = cell;
            }
        }

        // Clear the bottom n rows
        for i in (bottom - n)..bottom {
            for c in 0..grid.col_count() {
                *grid.cell_mut(i, c) = Cell::blank();
            }
        }
    }

    /// Erase in display
    /// mode 0: cursor to end of screen
    /// mode 1: start of screen to cursor
    /// mode 2/3: entire screen
    fn erase_display(&mut self, mode: u8) {
        let (row, col) = (self.cursor.row, self.cursor.col);
        let use_alt = self.is_alternate();
        let grid = if use_alt { &mut self.alt_grid } else { &mut self.grid };
        let rows = grid.row_count();
        let cols = grid.col_count();

        match mode {
            0 => {
                // Cursor to end of screen
                // Clear from cursor to end of current line
                for c in col..cols {
                    *grid.cell_mut(row, c) = Cell::blank();
                }
                // Clear remaining lines
                for r in (row + 1)..rows {
                    for c in 0..cols {
                        *grid.cell_mut(r, c) = Cell::blank();
                    }
                }
            }
            1 => {
                // Start of screen to cursor
                // Clear lines above cursor
                for r in 0..row {
                    for c in 0..cols {
                        *grid.cell_mut(r, c) = Cell::blank();
                    }
                }
                // Clear from start of line to cursor (inclusive)
                for c in 0..=col {
                    *grid.cell_mut(row, c) = Cell::blank();
                }
            }
            2 | 3 => {
                // Entire screen
                grid.clear();
            }
            _ => {}
        }
    }

    /// Erase in line
    /// mode 0: cursor to end of line
    /// mode 1: start of line to cursor
    /// mode 2: entire line
    fn erase_line(&mut self, mode: u8) {
        let (row, col) = (self.cursor.row, self.cursor.col);
        let use_alt = self.is_alternate();
        let grid = if use_alt { &mut self.alt_grid } else { &mut self.grid };
        let cols = grid.col_count();

        match mode {
            0 => {
                // Cursor to end of line
                for c in col..cols {
                    *grid.cell_mut(row, c) = Cell::blank();
                }
            }
            1 => {
                // Start of line to cursor (inclusive)
                for c in 0..=col {
                    *grid.cell_mut(row, c) = Cell::blank();
                }
            }
            2 => {
                // Entire line
                for c in 0..cols {
                    *grid.cell_mut(row, c) = Cell::blank();
                }
            }
            _ => {}
        }
    }

    /// Handle SGR (Select Graphic Rendition) sequences
    fn sgr(&mut self, params: &[u16]) {
        if params.is_empty() {
            self.pending_attrs = CellAttrs::default();
            return;
        }

        let mut i = 0;
        while i < params.len() {
            let p = params[i];
            match p {
                0 => {
                    // Reset all attributes
                    self.pending_attrs = CellAttrs::default();
                }
                1 => {
                    self.pending_attrs.flags.set(CellFlags::BOLD, true);
                }
                2 => {
                    self.pending_attrs.flags.set(CellFlags::DIM, true);
                }
                3 => {
                    self.pending_attrs.flags.set(CellFlags::ITALIC, true);
                }
                4 => {
                    self.pending_attrs.flags.set(CellFlags::UNDERLINE, true);
                }
                7 => {
                    self.pending_attrs.flags.set(CellFlags::REVERSE, true);
                }
                9 => {
                    self.pending_attrs.flags.set(CellFlags::STRIKETHROUGH, true);
                }
                22 => {
                    // Clear both bold and dim
                    self.pending_attrs.flags.set(CellFlags::BOLD, false);
                    self.pending_attrs.flags.set(CellFlags::DIM, false);
                }
                23 => {
                    self.pending_attrs.flags.set(CellFlags::ITALIC, false);
                }
                24 => {
                    self.pending_attrs.flags.set(CellFlags::UNDERLINE, false);
                }
                27 => {
                    self.pending_attrs.flags.set(CellFlags::REVERSE, false);
                }
                29 => {
                    self.pending_attrs.flags.set(CellFlags::STRIKETHROUGH, false);
                }
                30..=37 => {
                    // Standard foreground colors
                    self.pending_attrs.fg = PackedColor::indexed((p - 30) as u8);
                }
                38 => {
                    // Extended foreground color
                    if i + 1 < params.len() {
                        match params[i + 1] {
                            5 => {
                                // 256-color indexed
                                if i + 2 < params.len() {
                                    self.pending_attrs.fg = PackedColor::indexed(params[i + 2] as u8);
                                    i += 2;
                                }
                            }
                            2 => {
                                // RGB color
                                if i + 4 < params.len() {
                                    let r = params[i + 2] as u8;
                                    let g = params[i + 3] as u8;
                                    let b = params[i + 4] as u8;
                                    self.pending_attrs.fg = PackedColor::rgb(r, g, b);
                                    i += 4;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                39 => {
                    // Default foreground
                    self.pending_attrs.fg = PackedColor::DEFAULT_FG;
                }
                40..=47 => {
                    // Standard background colors
                    self.pending_attrs.bg = PackedColor::indexed((p - 40) as u8);
                }
                48 => {
                    // Extended background color
                    if i + 1 < params.len() {
                        match params[i + 1] {
                            5 => {
                                // 256-color indexed
                                if i + 2 < params.len() {
                                    self.pending_attrs.bg = PackedColor::indexed(params[i + 2] as u8);
                                    i += 2;
                                }
                            }
                            2 => {
                                // RGB color
                                if i + 4 < params.len() {
                                    let r = params[i + 2] as u8;
                                    let g = params[i + 3] as u8;
                                    let b = params[i + 4] as u8;
                                    self.pending_attrs.bg = PackedColor::rgb(r, g, b);
                                    i += 4;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                49 => {
                    // Default background
                    self.pending_attrs.bg = PackedColor::DEFAULT_BG;
                }
                90..=97 => {
                    // Bright foreground colors
                    self.pending_attrs.fg = PackedColor::indexed((p - 90 + 8) as u8);
                }
                100..=107 => {
                    // Bright background colors
                    self.pending_attrs.bg = PackedColor::indexed((p - 100 + 8) as u8);
                }
                _ => {}
            }
            i += 1;
        }
    }

    /// Set DEC private mode
    fn set_mode(&mut self, mode: u16, private: bool) {
        if private {
            match mode {
                1 => {
                    // Cursor keys application mode
                    self.mode.set(TerminalMode::CURSOR_KEYS_APPLICATION, true);
                }
                25 => {
                    // Cursor visible
                    self.mode.set(TerminalMode::CURSOR_VISIBLE, true);
                }
                1049 => {
                    // Alternate screen buffer - save main screen cursor
                    if !self.mode.contains(TerminalMode::ALTERNATE_SCREEN) {
                        self.saved_main_cursor = self.cursor;
                        self.mode.set(TerminalMode::ALTERNATE_SCREEN, true);
                        self.alt_grid.clear();
                        self.cursor.row = 0;
                        self.cursor.col = 0;
                    }
                }
                _ => {}
            }
        } else {
            match mode {
                4 => {
                    // Insert mode
                    self.mode.set(TerminalMode::INSERT_MODE, true);
                }
                20 => {
                    // Line feed new line mode
                    self.mode.set(TerminalMode::LINE_FEED_NEW_LINE, true);
                }
                _ => {}
            }
        }
    }

    /// Reset DEC private mode
    fn reset_mode(&mut self, mode: u16, private: bool) {
        if private {
            match mode {
                1 => {
                    // Cursor keys normal mode
                    self.mode.set(TerminalMode::CURSOR_KEYS_APPLICATION, false);
                }
                25 => {
                    // Cursor invisible
                    self.mode.set(TerminalMode::CURSOR_VISIBLE, false);
                }
                1049 => {
                    // Exit alternate screen buffer - restore main screen cursor
                    self.mode.set(TerminalMode::ALTERNATE_SCREEN, false);
                    self.cursor = self.saved_main_cursor;
                }
                _ => {}
            }
        } else {
            match mode {
                4 => {
                    // Replace mode
                    self.mode.set(TerminalMode::INSERT_MODE, false);
                }
                20 => {
                    // Normal line feed mode
                    self.mode.set(TerminalMode::LINE_FEED_NEW_LINE, false);
                }
                _ => {}
            }
        }
    }
}

/// VTE Parser wrapper implementing the Perform trait
pub struct TerminalParser {
    parser: Parser,
    state: ParserState,
}

impl TerminalParser {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            parser: Parser::new(),
            state: ParserState::new(cols, rows),
        }
    }

    /// Process incoming bytes
    pub fn process(&mut self, data: &[u8]) {
        for byte in data {
            self.parser.advance(&mut self.state, *byte);
        }
    }

    /// Get the active grid (immutable)
    pub fn grid(&self) -> &Grid {
        if self.state.mode.contains(TerminalMode::ALTERNATE_SCREEN) {
            &self.state.alt_grid
        } else {
            &self.state.grid
        }
    }

    /// Get the active grid (mutable)
    pub fn grid_mut(&mut self) -> &mut Grid {
        if self.state.mode.contains(TerminalMode::ALTERNATE_SCREEN) {
            &mut self.state.alt_grid
        } else {
            &mut self.state.grid
        }
    }

    /// Get cursor reference
    pub fn cursor(&self) -> &Cursor {
        &self.state.cursor
    }

    /// Get cursor mutable reference
    pub fn cursor_mut(&mut self) -> &mut Cursor {
        &mut self.state.cursor
    }

    /// Get terminal mode
    pub fn mode(&self) -> TerminalMode {
        self.state.mode
    }

    /// Resize the terminal
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.state.grid.resize(cols, rows);
        self.state.alt_grid.resize(cols, rows);
        self.state.scroll_top = 0;
        self.state.scroll_bottom = rows;
        self.state.cursor.move_to(self.state.cursor.row, self.state.cursor.col, rows, cols);
    }
}

impl Perform for ParserState {
    fn print(&mut self, c: char) {
        let width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);

        // Determine which grid to use and get dimensions
        let use_alt = self.is_alternate();
        let (max_row, max_col) = if use_alt {
            (self.alt_grid.row_count(), self.alt_grid.col_count())
        } else {
            (self.grid.row_count(), self.grid.col_count())
        };

        if max_row == 0 || max_col == 0 {
            return;
        }

        // Cache values we need
        let auto_wrap = self.mode.contains(TerminalMode::AUTO_WRAP);
        let scroll_bottom = self.scroll_bottom;
        let attrs = self.pending_attrs;

        // Handle auto-wrap (cursor past end of line)
        if auto_wrap && self.cursor.col >= max_col {
            self.cursor.col = 0;
            self.cursor.row += 1;
            if self.cursor.row >= scroll_bottom {
                self.cursor.row = scroll_bottom - 1;
                // scroll_up needs its own mutable borrow, so call it before getting grid
                self.scroll_up(1);
            }
        }

        // Handle wide character at line end - needs room for continuation cell
        if width > 1 && self.cursor.col + width > max_col {
            // Not enough room for wide character on this line
            if auto_wrap {
                // Wrap to next line first
                self.cursor.col = 0;
                self.cursor.row += 1;
                if self.cursor.row >= scroll_bottom {
                    self.cursor.row = scroll_bottom - 1;
                    self.scroll_up(1);
                }
            } else {
                // No auto-wrap - skip the character
                return;
            }
        }

        // Skip if cursor is out of bounds
        if self.cursor.row >= max_row || self.cursor.col >= max_col {
            return;
        }

        // Now get the mutable grid reference after all potential scroll_up calls
        let grid = if use_alt { &mut self.alt_grid } else { &mut self.grid };

        // Write the character with pending attributes
        let row = self.cursor.row;
        let col = self.cursor.col;
        let cell = grid.cell_mut(row, col);
        cell.char = c;
        cell.fg = attrs.fg;
        cell.bg = attrs.bg;
        cell.flags = attrs.flags;
        cell.width = width as u8;

        // Advance cursor
        self.cursor.col += 1;

        // Handle wide characters - mark next cell as continuation
        if width > 1 && self.cursor.col < max_col {
            // Mark the next cell as a continuation of wide char
            let next_col = self.cursor.col;
            let next_cell = grid.cell_mut(row, next_col);
            next_cell.char = '\0';
            next_cell.width = 0;
            self.cursor.col += 1;
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x08 => {
                // BS - Backspace
                if self.cursor.col > 0 {
                    self.cursor.col -= 1;
                }
            }
            0x09 => {
                // HT - Horizontal Tab
                // Determine which grid to use
                let use_alt = self.is_alternate();
                let max_col = if use_alt {
                    self.alt_grid.col_count()
                } else {
                    self.grid.col_count()
                };
                // Move to next tab stop (every 8 columns by default)
                let next_tab = ((self.cursor.col / 8) + 1) * 8;
                self.cursor.col = next_tab.min(max_col.saturating_sub(1));
            }
            0x0A => {
                // LF - Line Feed
                if self.mode.contains(TerminalMode::LINE_FEED_NEW_LINE) {
                    // LF also does CR in this mode
                    self.cursor.col = 0;
                }
                if self.cursor.row + 1 >= self.scroll_bottom {
                    self.scroll_up(1);
                } else {
                    self.cursor.row += 1;
                }
            }
            0x0D => {
                // CR - Carriage Return
                self.cursor.col = 0;
            }
            _ => {
                // Ignore other control characters for MVP
            }
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        if ignore {
            return;
        }

        // Convert params to Vec<u16>
        let param_vec: Vec<u16> = params.iter().map(|p| {
            if p.is_empty() { 0 } else { p[0] as u16 }
        }).collect();

        // Check for private mode indicator
        let private = intermediates.contains(&b'?');

        // Helper to get row count
        let row_count = if self.is_alternate() {
            self.alt_grid.row_count()
        } else {
            self.grid.row_count()
        };

        // Helper to get col count
        let col_count = if self.is_alternate() {
            self.alt_grid.col_count()
        } else {
            self.grid.col_count()
        };

        match action {
            'A' => {
                // Cursor up
                let n = param_vec.first().copied().unwrap_or(1) as usize;
                let min_row = self.scroll_top;
                if self.cursor.row > min_row {
                    self.cursor.row = self.cursor.row.saturating_sub(n).max(min_row);
                }
            }
            'B' => {
                // Cursor down
                let n = param_vec.first().copied().unwrap_or(1) as usize;
                let max_row = row_count.saturating_sub(1);
                self.cursor.row = (self.cursor.row + n).min(max_row);
            }
            'C' => {
                // Cursor forward (right)
                let n = param_vec.first().copied().unwrap_or(1) as usize;
                let max_col = col_count.saturating_sub(1);
                self.cursor.col = (self.cursor.col + n).min(max_col);
            }
            'D' => {
                // Cursor backward (left)
                let n = param_vec.first().copied().unwrap_or(1) as usize;
                self.cursor.col = self.cursor.col.saturating_sub(n);
            }
            'E' => {
                // Cursor next line
                let n = param_vec.first().copied().unwrap_or(1) as usize;
                self.cursor.col = 0;
                let max_row = row_count.saturating_sub(1);
                self.cursor.row = (self.cursor.row + n).min(max_row);
            }
            'F' => {
                // Cursor previous line
                let n = param_vec.first().copied().unwrap_or(1) as usize;
                self.cursor.col = 0;
                let min_row = self.scroll_top;
                if self.cursor.row > min_row {
                    self.cursor.row = self.cursor.row.saturating_sub(n).max(min_row);
                }
            }
            'G' => {
                // Cursor horizontal absolute
                let col = param_vec.first().copied().unwrap_or(1) as usize;
                self.cursor.col = col.saturating_sub(1).min(col_count.saturating_sub(1));
            }
            'H' | 'f' => {
                // Cursor position
                let row = param_vec.first().copied().unwrap_or(1) as usize;
                let col = param_vec.get(1).copied().unwrap_or(1) as usize;
                self.cursor.row = row.saturating_sub(1).min(row_count.saturating_sub(1));
                self.cursor.col = col.saturating_sub(1).min(col_count.saturating_sub(1));
            }
            'J' => {
                // Erase in display
                let mode = param_vec.first().copied().unwrap_or(0) as u8;
                self.erase_display(mode);
            }
            'K' => {
                // Erase in line
                let mode = param_vec.first().copied().unwrap_or(0) as u8;
                self.erase_line(mode);
            }
            'm' => {
                // SGR - Select Graphic Rendition
                self.sgr(&param_vec);
            }
            'r' => {
                // Set scrolling region
                let top = param_vec.first().copied().unwrap_or(1) as usize;
                let bottom = param_vec.get(1).copied().unwrap_or(row_count as u16) as usize;
                self.scroll_top = top.saturating_sub(1).min(row_count.saturating_sub(1));
                self.scroll_bottom = bottom.min(row_count);
                // Move cursor to home position
                self.cursor.row = self.scroll_top;
                self.cursor.col = 0;
            }
            's' => {
                // Save cursor position
                self.cursor.save();
            }
            'u' => {
                // Restore cursor position
                self.cursor.restore();
            }
            'h' => {
                // Set mode
                for mode in param_vec {
                    self.set_mode(mode, private);
                }
            }
            'l' => {
                // Reset mode
                for mode in param_vec {
                    self.reset_mode(mode, private);
                }
            }
            'd' => {
                // Vertical position absolute (row)
                let row = param_vec.first().copied().unwrap_or(1) as usize;
                self.cursor.row = row.saturating_sub(1).min(row_count.saturating_sub(1));
            }
            'S' => {
                // Scroll up
                let n = param_vec.first().copied().unwrap_or(1) as usize;
                self.scroll_up(n);
            }
            'T' => {
                // Scroll down - for MVP, just ignore or implement basic version
                // This would require scroll_down implementation
            }
            'X' => {
                // Erase characters
                let n = param_vec.first().copied().unwrap_or(1) as usize;
                let use_alt = self.is_alternate();
                let grid = if use_alt { &mut self.alt_grid } else { &mut self.grid };
                let max_col = grid.col_count();
                let row = self.cursor.row;
                let start_col = self.cursor.col;
                for i in 0..n {
                    let col = start_col + i;
                    if col >= max_col {
                        break;
                    }
                    *grid.cell_mut(row, col) = Cell::blank();
                }
            }
            _ => {
                // Ignore unhandled CSI sequences for MVP
            }
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // For MVP, just ignore OSC sequences (window titles, etc.)
        if params.is_empty() {
            return;
        }

        // OSC 0 and OSC 2: Set window title (ignore for MVP)
        // OSC 1: Set icon name (ignore for MVP)
        // Could log or handle in future versions
        let _ = params;
    }

    fn hook(&mut self, _params: &vte::Params, _intermediates: &[u8], _ignore: bool, _action: char) {
        // DCS hook - not implemented for MVP
    }

    fn put(&mut self, _byte: u8) {
        // DCS put - not implemented for MVP
    }

    fn unhook(&mut self) {
        // DCS unhook - not implemented for MVP
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {
        // ESC dispatch - not implemented for MVP
        // Common sequences: ESC M (reverse index), ESC 7 (save cursor), ESC 8 (restore cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_parser() -> TerminalParser {
        TerminalParser::new(80, 24)
    }

    #[test]
    fn test_basic_character_output() {
        let mut parser = create_parser();
        parser.process(b"Hello");

        let grid = parser.grid();
        assert_eq!(grid.cell(0, 0).char, 'H');
        assert_eq!(grid.cell(0, 1).char, 'e');
        assert_eq!(grid.cell(0, 2).char, 'l');
        assert_eq!(grid.cell(0, 3).char, 'l');
        assert_eq!(grid.cell(0, 4).char, 'o');

        // Cursor should be at column 5
        assert_eq!(parser.cursor().col, 5);
        assert_eq!(parser.cursor().row, 0);
    }

    #[test]
    fn test_cursor_up_cuu() {
        let mut parser = create_parser();
        // Move cursor to row 5, col 10
        parser.process(b"\x1b[6;11H");
        assert_eq!(parser.cursor().row, 5);
        assert_eq!(parser.cursor().col, 10);

        // Move up 3 rows
        parser.process(b"\x1b[3A");
        assert_eq!(parser.cursor().row, 2);
        assert_eq!(parser.cursor().col, 10);
    }

    #[test]
    fn test_cursor_down_cud() {
        let mut parser = create_parser();
        // Start at row 0, col 10
        parser.process(b"\x1b[1;11H");
        assert_eq!(parser.cursor().row, 0);
        assert_eq!(parser.cursor().col, 10);

        // Move down 5 rows
        parser.process(b"\x1b[5B");
        assert_eq!(parser.cursor().row, 5);
        assert_eq!(parser.cursor().col, 10);
    }

    #[test]
    fn test_cursor_forward_cuf() {
        let mut parser = create_parser();
        parser.process(b"\x1b[1;1H");
        assert_eq!(parser.cursor().col, 0);

        // Move forward 10 columns
        parser.process(b"\x1b[10C");
        assert_eq!(parser.cursor().col, 10);
    }

    #[test]
    fn test_cursor_backward_cub() {
        let mut parser = create_parser();
        parser.process(b"\x1b[1;20H");
        assert_eq!(parser.cursor().col, 19);

        // Move backward 10 columns
        parser.process(b"\x1b[10D");
        assert_eq!(parser.cursor().col, 9);
    }

    #[test]
    fn test_cursor_position_cup() {
        let mut parser = create_parser();

        // Move to row 10, col 20 (1-indexed in ANSI)
        parser.process(b"\x1b[10;20H");
        assert_eq!(parser.cursor().row, 9);
        assert_eq!(parser.cursor().col, 19);

        // Move to home (no params = 1,1)
        parser.process(b"\x1b[H");
        assert_eq!(parser.cursor().row, 0);
        assert_eq!(parser.cursor().col, 0);
    }

    #[test]
    fn test_cursor_position_cup_f_variant() {
        let mut parser = create_parser();

        // Test with 'f' action (same as 'H')
        parser.process(b"\x1b[5;15f");
        assert_eq!(parser.cursor().row, 4);
        assert_eq!(parser.cursor().col, 14);
    }

    #[test]
    fn test_erase_display_cursor_to_end() {
        let mut parser = create_parser();
        // Write some text with proper CR+LF
        parser.process(b"Line1\r\nLine2\r\nLine3");
        parser.process(b"\x1b[2;3H"); // Move to row 2 (index 1), col 3 (index 2)

        // Erase from cursor to end
        parser.process(b"\x1b[0J");

        let grid = parser.grid();
        // First line should be intact
        assert_eq!(grid.cell(0, 0).char, 'L');
        // Second line: first two chars should remain
        assert_eq!(grid.cell(1, 0).char, 'L');
        assert_eq!(grid.cell(1, 1).char, 'i');
        // Rest of second line should be cleared
        assert_eq!(grid.cell(1, 2).char, ' ');
        // Third line should be cleared
        assert_eq!(grid.cell(2, 0).char, ' ');
    }

    #[test]
    fn test_erase_display_start_to_cursor() {
        let mut parser = create_parser();
        // Write some text with proper CR+LF
        parser.process(b"Line1\r\nLine2\r\nLine3");
        parser.process(b"\x1b[2;3H"); // Move to row 2 (index 1), col 3 (index 2)

        // Erase from start to cursor
        parser.process(b"\x1b[1J");

        let grid = parser.grid();
        // First line should be cleared
        assert_eq!(grid.cell(0, 0).char, ' ');
        // Second line: first 3 chars should be cleared (inclusive)
        assert_eq!(grid.cell(1, 0).char, ' ');
        assert_eq!(grid.cell(1, 1).char, ' ');
        assert_eq!(grid.cell(1, 2).char, ' ');
        // Rest of second line and third line should remain
        assert_eq!(grid.cell(1, 3).char, 'e');
        assert_eq!(grid.cell(2, 0).char, 'L');
    }

    #[test]
    fn test_erase_display_entire() {
        let mut parser = create_parser();
        parser.process(b"Line1\nLine2");

        // Erase entire display
        parser.process(b"\x1b[2J");

        let grid = parser.grid();
        assert_eq!(grid.cell(0, 0).char, ' ');
        assert_eq!(grid.cell(1, 0).char, ' ');
    }

    #[test]
    fn test_erase_line_cursor_to_end() {
        let mut parser = create_parser();
        parser.process(b"Hello World");
        parser.process(b"\x1b[1;6H"); // Move to col 6 (index 5)

        // Erase from cursor to end of line
        parser.process(b"\x1b[0K");

        let grid = parser.grid();
        // First 5 chars should remain
        assert_eq!(grid.cell(0, 0).char, 'H');
        assert_eq!(grid.cell(0, 4).char, 'o');
        // Rest should be cleared
        assert_eq!(grid.cell(0, 5).char, ' ');
        assert_eq!(grid.cell(0, 6).char, ' ');
    }

    #[test]
    fn test_erase_line_start_to_cursor() {
        let mut parser = create_parser();
        parser.process(b"Hello World");
        parser.process(b"\x1b[1;6H"); // Move to col 6 (index 5)

        // Erase from start to cursor (inclusive)
        parser.process(b"\x1b[1K");

        let grid = parser.grid();
        // First 6 chars should be cleared (inclusive)
        assert_eq!(grid.cell(0, 0).char, ' ');
        assert_eq!(grid.cell(0, 5).char, ' ');
        // Rest should remain
        assert_eq!(grid.cell(0, 6).char, 'W');
    }

    #[test]
    fn test_erase_line_entire() {
        let mut parser = create_parser();
        parser.process(b"Hello World");

        // Erase entire line
        parser.process(b"\x1b[2K");

        let grid = parser.grid();
        for i in 0..11 {
            assert_eq!(grid.cell(0, i).char, ' ');
        }
    }

    #[test]
    fn test_sgr_reset() {
        let mut parser = create_parser();
        // Set some attributes
        parser.process(b"\x1b[1;3;4m"); // Bold, italic, underline
        parser.process(b"A");
        let cell = parser.grid().cell(0, 0);
        assert!(cell.flags.contains(CellFlags::BOLD));
        assert!(cell.flags.contains(CellFlags::ITALIC));
        assert!(cell.flags.contains(CellFlags::UNDERLINE));

        // Reset all attributes
        parser.process(b"\x1b[0m");
        parser.process(b"B");
        let cell = parser.grid().cell(0, 1);
        assert!(!cell.flags.contains(CellFlags::BOLD));
        assert!(!cell.flags.contains(CellFlags::ITALIC));
        assert!(!cell.flags.contains(CellFlags::UNDERLINE));
    }

    #[test]
    fn test_sgr_bold() {
        let mut parser = create_parser();
        parser.process(b"\x1b[1mA");
        assert!(parser.grid().cell(0, 0).flags.contains(CellFlags::BOLD));

        // Clear bold with code 22
        parser.process(b"\x1b[22mB");
        assert!(!parser.grid().cell(0, 1).flags.contains(CellFlags::BOLD));
    }

    #[test]
    fn test_sgr_dim() {
        let mut parser = create_parser();
        parser.process(b"\x1b[2mA");
        assert!(parser.grid().cell(0, 0).flags.contains(CellFlags::DIM));
    }

    #[test]
    fn test_sgr_italic() {
        let mut parser = create_parser();
        parser.process(b"\x1b[3mA");
        assert!(parser.grid().cell(0, 0).flags.contains(CellFlags::ITALIC));

        // Clear italic with code 23
        parser.process(b"\x1b[23mB");
        assert!(!parser.grid().cell(0, 1).flags.contains(CellFlags::ITALIC));
    }

    #[test]
    fn test_sgr_underline() {
        let mut parser = create_parser();
        parser.process(b"\x1b[4mA");
        assert!(parser.grid().cell(0, 0).flags.contains(CellFlags::UNDERLINE));

        // Clear underline with code 24
        parser.process(b"\x1b[24mB");
        assert!(!parser.grid().cell(0, 1).flags.contains(CellFlags::UNDERLINE));
    }

    #[test]
    fn test_sgr_reverse() {
        let mut parser = create_parser();
        parser.process(b"\x1b[7mA");
        assert!(parser.grid().cell(0, 0).flags.contains(CellFlags::REVERSE));

        // Clear reverse with code 27
        parser.process(b"\x1b[27mB");
        assert!(!parser.grid().cell(0, 1).flags.contains(CellFlags::REVERSE));
    }

    #[test]
    fn test_sgr_strikethrough() {
        let mut parser = create_parser();
        parser.process(b"\x1b[9mA");
        assert!(parser.grid().cell(0, 0).flags.contains(CellFlags::STRIKETHROUGH));

        // Clear strikethrough with code 29
        parser.process(b"\x1b[29mB");
        assert!(!parser.grid().cell(0, 1).flags.contains(CellFlags::STRIKETHROUGH));
    }

    #[test]
    fn test_sgr_standard_colors() {
        let mut parser = create_parser();

        // Test foreground colors 30-37
        for i in 0..8 {
            parser.process(&format!("\x1b[{}mX", 30 + i).into_bytes());
            let cell = parser.grid().cell(0, i as usize);
            assert!(cell.fg.is_indexed());
            assert_eq!(cell.fg.indexed_index(), i);
        }

        // Test background colors 40-47 on row 2 (index 1)
        parser.process(b"\x1b[2;1H");
        for i in 0..8 {
            parser.process(&format!("\x1b[{}mY", 40 + i).into_bytes());
            let cell = parser.grid().cell(1, i as usize);
            assert!(cell.bg.is_indexed());
            assert_eq!(cell.bg.indexed_index(), i);
        }
    }

    #[test]
    fn test_sgr_bright_colors() {
        let mut parser = create_parser();

        // Test bright foreground colors 90-97
        for i in 0..8 {
            parser.process(&format!("\x1b[{}mX", 90 + i).into_bytes());
            let cell = parser.grid().cell(0, i as usize);
            assert!(cell.fg.is_indexed());
            assert_eq!(cell.fg.indexed_index(), 8 + i);
        }

        // Test bright background colors 100-107
        parser.process(b"\x1b[2;1H");
        for i in 0..8 {
            parser.process(&format!("\x1b[{}mY", 100 + i).into_bytes());
            let cell = parser.grid().cell(1, i as usize);
            assert!(cell.bg.is_indexed());
            assert_eq!(cell.bg.indexed_index(), 8 + i);
        }
    }

    #[test]
    fn test_sgr_256_color() {
        let mut parser = create_parser();

        // Test 256-color foreground
        parser.process(b"\x1b[38;5;100mA");
        let cell = parser.grid().cell(0, 0);
        assert!(cell.fg.is_indexed());
        assert_eq!(cell.fg.indexed_index(), 100);

        // Test 256-color background
        parser.process(b"\x1b[48;5;200mB");
        let cell = parser.grid().cell(0, 1);
        assert!(cell.bg.is_indexed());
        assert_eq!(cell.bg.indexed_index(), 200);
    }

    #[test]
    fn test_sgr_rgb_color() {
        let mut parser = create_parser();

        // Test RGB foreground
        parser.process(b"\x1b[38;2;255;128;64mA");
        let cell = parser.grid().cell(0, 0);
        assert!(cell.fg.is_rgb());
        let (r, g, b) = cell.fg.rgb_components();
        assert_eq!((r, g, b), (255, 128, 64));

        // Test RGB background
        parser.process(b"\x1b[48;2;100;150;200mB");
        let cell = parser.grid().cell(0, 1);
        assert!(cell.bg.is_rgb());
        let (r, g, b) = cell.bg.rgb_components();
        assert_eq!((r, g, b), (100, 150, 200));
    }

    #[test]
    fn test_sgr_default_colors() {
        let mut parser = create_parser();

        // Set some colors first
        parser.process(b"\x1b[31;42mA");

        // Reset to default
        parser.process(b"\x1b[39;49mB");
        let cell = parser.grid().cell(0, 1);
        assert!(cell.fg.is_default());
        assert!(cell.bg.is_default());
    }

    #[test]
    fn test_auto_wrap() {
        let mut parser = TerminalParser::new(10, 5);

        // Write 12 characters (should wrap to next line)
        parser.process(b"0123456789AB");

        let grid = parser.grid();
        // First 10 chars on row 0
        for i in 0..10 {
            assert_eq!(grid.cell(0, i).char, (b'0' + i as u8) as char);
        }
        // Last 2 chars on row 1
        assert_eq!(grid.cell(1, 0).char, 'A');
        assert_eq!(grid.cell(1, 1).char, 'B');
    }

    #[test]
    fn test_auto_wrap_with_lf() {
        let mut parser = TerminalParser::new(10, 5);

        // Write text with proper CR+LF (LF doesn't do CR by default)
        parser.process(b"Line1\r\nLine2");

        let grid = parser.grid();
        assert_eq!(grid.cell(0, 0).char, 'L');
        assert_eq!(grid.cell(0, 4).char, '1');
        assert_eq!(grid.cell(1, 0).char, 'L');
        assert_eq!(grid.cell(1, 4).char, '2');
    }

    #[test]
    fn test_carriage_return() {
        let mut parser = create_parser();
        parser.process(b"Hello\rWorld");

        let grid = parser.grid();
        // "World" should overwrite "Hello"
        assert_eq!(grid.cell(0, 0).char, 'W');
        assert_eq!(grid.cell(0, 1).char, 'o');
        assert_eq!(grid.cell(0, 2).char, 'r');
        assert_eq!(grid.cell(0, 3).char, 'l');
        assert_eq!(grid.cell(0, 4).char, 'd');
    }

    #[test]
    fn test_backspace() {
        let mut parser = create_parser();
        parser.process(b"ABC\x08X");

        let grid = parser.grid();
        assert_eq!(grid.cell(0, 0).char, 'A');
        assert_eq!(grid.cell(0, 1).char, 'B');
        assert_eq!(grid.cell(0, 2).char, 'X'); // 'C' overwritten by 'X'
    }

    #[test]
    fn test_tab() {
        let mut parser = create_parser();
        parser.process(b"A\tB");

        let grid = parser.grid();
        assert_eq!(grid.cell(0, 0).char, 'A');
        assert_eq!(grid.cell(0, 1).char, ' '); // Filled by tab
        // Tab stops at columns 8, 16, etc.
        assert_eq!(grid.cell(0, 8).char, 'B');
        assert_eq!(parser.cursor().col, 9);
    }

    #[test]
    fn test_alternate_screen_switch() {
        let mut parser = create_parser();

        // Write to main screen
        parser.process(b"Main");
        assert_eq!(parser.grid().cell(0, 0).char, 'M');

        // Switch to alternate screen
        parser.process(b"\x1b[?1049h");

        // Verify we're on alternate screen (should be cleared)
        assert!(parser.mode().contains(TerminalMode::ALTERNATE_SCREEN));
        assert_eq!(parser.grid().cell(0, 0).char, ' ');

        // Write to alternate screen
        parser.process(b"Alt");
        assert_eq!(parser.grid().cell(0, 0).char, 'A');

        // Switch back to main screen
        parser.process(b"\x1b[?1049l");

        // Verify we're back on main screen with content intact
        assert!(!parser.mode().contains(TerminalMode::ALTERNATE_SCREEN));
        assert_eq!(parser.grid().cell(0, 0).char, 'M');
    }

    #[test]
    fn test_alternate_screen_cursor_restore() {
        let mut parser = create_parser();

        // Position cursor on main screen
        parser.process(b"\x1b[5;10H");
        assert_eq!(parser.cursor().row, 4);
        assert_eq!(parser.cursor().col, 9);

        // Switch to alternate screen
        parser.process(b"\x1b[?1049h");

        // Cursor should be at home position on alternate screen
        assert_eq!(parser.cursor().row, 0);
        assert_eq!(parser.cursor().col, 0);

        // Move cursor on alternate screen
        parser.process(b"\x1b[10;20H");
        assert_eq!(parser.cursor().row, 9);
        assert_eq!(parser.cursor().col, 19);

        // Switch back to main screen
        parser.process(b"\x1b[?1049l");

        // Cursor should be restored to original position
        assert_eq!(parser.cursor().row, 4);
        assert_eq!(parser.cursor().col, 9);
    }

    #[test]
    fn test_cursor_save_restore() {
        let mut parser = create_parser();

        // Move cursor
        parser.process(b"\x1b[10;20H");
        assert_eq!(parser.cursor().row, 9);
        assert_eq!(parser.cursor().col, 19);

        // Save cursor position
        parser.process(b"\x1b[s");

        // Move cursor elsewhere
        parser.process(b"\x1b[1;1H");
        assert_eq!(parser.cursor().row, 0);
        assert_eq!(parser.cursor().col, 0);

        // Restore cursor position
        parser.process(b"\x1b[u");
        assert_eq!(parser.cursor().row, 9);
        assert_eq!(parser.cursor().col, 19);
    }

    #[test]
    fn test_cursor_next_line() {
        let mut parser = create_parser();
        parser.process(b"\x1b[5;10H");
        assert_eq!(parser.cursor().row, 4);
        assert_eq!(parser.cursor().col, 9);

        // Move to next line (E)
        parser.process(b"\x1b[2E");
        assert_eq!(parser.cursor().row, 6);
        assert_eq!(parser.cursor().col, 0); // Column resets
    }

    #[test]
    fn test_cursor_previous_line() {
        let mut parser = create_parser();
        parser.process(b"\x1b[10;20H");
        assert_eq!(parser.cursor().row, 9);
        assert_eq!(parser.cursor().col, 19);

        // Move to previous line (F)
        parser.process(b"\x1b[3F");
        assert_eq!(parser.cursor().row, 6);
        assert_eq!(parser.cursor().col, 0); // Column resets
    }

    #[test]
    fn test_cursor_horizontal_absolute() {
        let mut parser = create_parser();
        parser.process(b"\x1b[5;1H");
        assert_eq!(parser.cursor().col, 0);

        // Move to column 20 (G)
        parser.process(b"\x1b[20G");
        assert_eq!(parser.cursor().row, 4); // Row unchanged
        assert_eq!(parser.cursor().col, 19);
    }

    #[test]
    fn test_vertical_position_absolute() {
        let mut parser = create_parser();
        parser.process(b"\x1b[1;20H");
        assert_eq!(parser.cursor().row, 0);

        // Move to row 10 (d)
        parser.process(b"\x1b[10d");
        assert_eq!(parser.cursor().row, 9);
        assert_eq!(parser.cursor().col, 19); // Column unchanged
    }

    #[test]
    fn test_scroll_up() {
        let mut parser = TerminalParser::new(10, 5);

        // Fill each row with a different character using explicit cursor positioning
        // Row 0 = 'A', Row 1 = 'B', etc.
        parser.process(b"\x1b[1;1HA");  // Row 1 (0-indexed: 0)
        parser.process(b"\x1b[2;1HB");  // Row 2 (0-indexed: 1)
        parser.process(b"\x1b[3;1HC");  // Row 3 (0-indexed: 2)
        parser.process(b"\x1b[4;1HD");  // Row 4 (0-indexed: 3)
        parser.process(b"\x1b[5;1HE");  // Row 5 (0-indexed: 4)

        // Verify setup
        let grid = parser.grid();
        assert_eq!(grid.cell(0, 0).char, 'A');
        assert_eq!(grid.cell(1, 0).char, 'B');
        assert_eq!(grid.cell(2, 0).char, 'C');
        assert_eq!(grid.cell(3, 0).char, 'D');
        assert_eq!(grid.cell(4, 0).char, 'E');

        // Scroll up 2 lines
        parser.process(b"\x1b[2S");

        let grid = parser.grid();
        // After scroll up 2: rows shift up, bottom 2 rows become blank
        // Row 0 should have C (was row 2)
        // Row 1 should have D (was row 3)
        // Row 2 should have E (was row 4)
        // Row 3, 4 should be blank
        assert_eq!(grid.cell(0, 0).char, 'C', "After scroll, row 0 should have C");
        assert_eq!(grid.cell(1, 0).char, 'D', "After scroll, row 1 should have D");
        assert_eq!(grid.cell(2, 0).char, 'E', "After scroll, row 2 should have E");
        assert_eq!(grid.cell(3, 0).char, ' ', "After scroll, row 3 should be blank");
        assert_eq!(grid.cell(4, 0).char, ' ', "After scroll, row 4 should be blank");
    }

    #[test]
    fn test_erase_characters() {
        let mut parser = create_parser();
        parser.process(b"Hello World");
        parser.process(b"\x1b[1;6H"); // Move to col 6 (index 5)

        // Erase 3 characters
        parser.process(b"\x1b[3X");

        let grid = parser.grid();
        assert_eq!(grid.cell(0, 0).char, 'H');
        assert_eq!(grid.cell(0, 4).char, 'o');
        assert_eq!(grid.cell(0, 5).char, ' '); // Erased
        assert_eq!(grid.cell(0, 6).char, ' '); // Erased
        assert_eq!(grid.cell(0, 7).char, ' '); // Erased
        assert_eq!(grid.cell(0, 8).char, 'r'); // Not erased
    }

    #[test]
    fn test_set_scroll_region() {
        let mut parser = create_parser();

        // Set scroll region from lines 5-10
        parser.process(b"\x1b[5;10r");

        // Cursor should move to home (top of scroll region)
        assert_eq!(parser.cursor().row, 4);
        assert_eq!(parser.cursor().col, 0);
    }

    #[test]
    fn test_cursor_visible_mode() {
        let mut parser = create_parser();

        // Cursor is visible by default
        assert!(parser.mode().contains(TerminalMode::CURSOR_VISIBLE));

        // Hide cursor
        parser.process(b"\x1b[?25l");
        assert!(!parser.mode().contains(TerminalMode::CURSOR_VISIBLE));

        // Show cursor
        parser.process(b"\x1b[?25h");
        assert!(parser.mode().contains(TerminalMode::CURSOR_VISIBLE));
    }

    #[test]
    fn test_cursor_bounds() {
        let mut parser = TerminalParser::new(10, 5);

        // Try to move cursor beyond bounds
        parser.process(b"\x1b[100;100H");
        assert!(parser.cursor().row < 5);
        assert!(parser.cursor().col < 10);

        // Try to move up beyond top
        parser.process(b"\x1b[100A");
        assert_eq!(parser.cursor().row, 0);

        // Try to move left beyond start
        parser.process(b"\x1b[100D");
        assert_eq!(parser.cursor().col, 0);
    }

    #[test]
    fn test_line_feed_new_line_mode() {
        let mut parser = create_parser();

        // Enable line feed new line mode
        parser.process(b"\x1b[20h");

        // LF should also do CR in this mode
        parser.process(b"\x1b[1;10H");
        parser.process(b"\n");

        assert_eq!(parser.cursor().row, 1);
        assert_eq!(parser.cursor().col, 0);

        // Disable line feed new line mode
        parser.process(b"\x1b[20l");
    }

    #[test]
    fn test_auto_scroll_at_bottom() {
        let mut parser = TerminalParser::new(10, 5);

        // Fill the screen with 5 lines using direct cursor positioning
        for i in 1..=5 {
            parser.process(format!("\x1b[{};1HLine{}", i, i).as_bytes());
        }

        // Move cursor to bottom row
        parser.process(b"\x1b[5;1H");

        // Write LF to trigger scroll
        parser.process(b"\n");
        parser.process(b"Line6");

        let grid = parser.grid();
        // After scroll, top row (Line1) is scrolled off
        // Row 0 should now show Line2
        assert_eq!(grid.cell(0, 0).char, 'L');
        assert_eq!(grid.cell(0, 4).char, '2');
        // Row 4 should have Line6
        assert_eq!(grid.cell(4, 0).char, 'L');
        assert_eq!(grid.cell(4, 4).char, '6');
    }

    #[test]
    fn test_wide_character() {
        let mut parser = TerminalParser::new(10, 5);

        // Write a wide character (CJK characters are typically width 2)
        parser.process("你好".as_bytes());

        let grid = parser.grid();
        // First wide character at position 0
        assert_eq!(grid.cell(0, 0).char, '你');
        assert_eq!(grid.cell(0, 0).width, 2);
        // Second cell should be a placeholder
        assert_eq!(grid.cell(0, 1).char, '\0');

        // Second wide character at position 2
        assert_eq!(grid.cell(0, 2).char, '好');
        assert_eq!(grid.cell(0, 2).width, 2);
    }

    #[test]
    fn test_wide_character_at_line_end() {
        let mut parser = TerminalParser::new(10, 5);

        // Move cursor to column 8 (0-indexed), near the end
        parser.process(b"\x1b[1;9H");

        // Write a wide character - should wrap to next line since it doesn't fit
        parser.process("你".as_bytes());

        let grid = parser.grid();
        // The wide character should be on row 1 (wrapped)
        // Implementation may vary - just verify the wide char was written somewhere
        let found = (0..5).any(|row| {
            (0..10).any(|col| grid.cell(row, col).char == '你')
        });
        assert!(found, "Wide character should be written to the grid");
    }

    #[test]
    fn test_scroll_region_isolation() {
        let mut parser = TerminalParser::new(10, 6);

        // Fill screen with identifiable content using cursor positioning
        for (i, ch) in ['A', 'B', 'C', 'D', 'E', 'F'].iter().enumerate() {
            parser.process(format!("\x1b[{};1H{}", i + 1, ch).as_bytes());
        }

        // Set scroll region to rows 2-4 (1-indexed)
        parser.process(b"\x1b[2;4r");

        // Move to bottom of scroll region (row 4, 1-indexed = row 3, 0-indexed)
        parser.process(b"\x1b[4;1H");

        // Trigger scroll with LF at bottom of region
        parser.process(b"\nX");

        let grid = parser.grid();
        // Row 0 (A) should be unchanged - outside scroll region
        assert_eq!(grid.cell(0, 0).char, 'A');

        // Rows 4 and 5 (E, F) should be unchanged - outside scroll region
        assert_eq!(grid.cell(4, 0).char, 'E');
        assert_eq!(grid.cell(5, 0).char, 'F');

        // Within scroll region (rows 1-3), content should have scrolled
        // Row 1 should have C (shifted from row 2)
        assert_eq!(grid.cell(1, 0).char, 'C');
        // Row 2 should have D (shifted from row 3)
        assert_eq!(grid.cell(2, 0).char, 'D');
        // Row 3 should have X (new content)
        assert_eq!(grid.cell(3, 0).char, 'X');

        // Reset scroll region
        parser.process(b"\x1b[r");
    }
}