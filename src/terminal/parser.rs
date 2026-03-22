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