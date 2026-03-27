// src/terminal_v2/mod.rs
//! Terminal implementation using alacritty_terminal's VTE parser
//!
//! This provides a thin wrapper around alacritty_terminal for use in RSTerm.
//! The VTE parsing is handled by alacritty_terminal, but PTY management
//! is handled by our backend module.

use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};
use egui::Key;
use bitflags::bitflags;

// ============================================================================
// CellFlags - compatible with the old implementation
// ============================================================================

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct CellFlags: u8 {
        const BOLD = 0x01;
        const ITALIC = 0x02;
        const UNDERLINE = 0x04;
        const BLINK = 0x08;
        const REVERSE = 0x10;
        const DIM = 0x20;
        const STRIKETHROUGH = 0x40;
        const WIDE_CHAR = 0x80;          // Double-width character (CJK)
    }
}

impl From<alacritty_terminal::term::cell::Flags> for CellFlags {
    fn from(flags: alacritty_terminal::term::cell::Flags) -> Self {
        let mut result = CellFlags::empty();
        if flags.contains(alacritty_terminal::term::cell::Flags::BOLD) {
            result |= CellFlags::BOLD;
        }
        if flags.contains(alacritty_terminal::term::cell::Flags::ITALIC) {
            result |= CellFlags::ITALIC;
        }
        if flags.contains(alacritty_terminal::term::cell::Flags::UNDERLINE) {
            result |= CellFlags::UNDERLINE;
        }
        if flags.contains(alacritty_terminal::term::cell::Flags::INVERSE) {
            result |= CellFlags::REVERSE;
        }
        if flags.contains(alacritty_terminal::term::cell::Flags::DIM) {
            result |= CellFlags::DIM;
        }
        if flags.contains(alacritty_terminal::term::cell::Flags::STRIKEOUT) {
            result |= CellFlags::STRIKETHROUGH;
        }
        if flags.contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR) {
            result |= CellFlags::WIDE_CHAR;
        }
        result
    }
}

/// Check if a cell is a wide char spacer (second cell of a double-width char)
pub fn is_wide_char_spacer(flags: alacritty_terminal::term::cell::Flags) -> bool {
    flags.contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER)
}

// ============================================================================
// PackedColor - compatible with the old implementation
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PackedColor(u32);

impl PackedColor {
    pub const DEFAULT_FG: Self = Self(0x00_000000);
    pub const DEFAULT_BG: Self = Self(0x00_000001);

    pub fn indexed(idx: u8) -> Self { Self(0x01_000000 | idx as u32) }
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self(0x02_000000 | (r as u32) << 16 | (g as u32) << 8 | b as u32)
    }
    pub fn is_default(&self) -> bool { self.0 >> 24 == 0 }
    pub fn is_indexed(&self) -> bool { self.0 >> 24 == 1 }
    pub fn is_rgb(&self) -> bool { self.0 >> 24 == 2 }
    pub fn indexed_index(&self) -> u8 { (self.0 & 0xFF) as u8 }
    pub fn rgb_components(&self) -> (u8, u8, u8) {
        (((self.0 >> 16) & 0xFF) as u8, ((self.0 >> 8) & 0xFF) as u8, (self.0 & 0xFF) as u8)
    }
}

impl Default for PackedColor {
    fn default() -> Self { Self::DEFAULT_FG }
}

impl From<Color> for PackedColor {
    fn from(color: Color) -> Self {
        match color {
            Color::Named(named) => match named {
                NamedColor::Foreground => PackedColor::DEFAULT_FG,
                NamedColor::Background => PackedColor::DEFAULT_BG,
                NamedColor::Black => PackedColor::indexed(0),
                NamedColor::Red => PackedColor::indexed(1),
                NamedColor::Green => PackedColor::indexed(2),
                NamedColor::Yellow => PackedColor::indexed(3),
                NamedColor::Blue => PackedColor::indexed(4),
                NamedColor::Magenta => PackedColor::indexed(5),
                NamedColor::Cyan => PackedColor::indexed(6),
                NamedColor::White => PackedColor::indexed(7),
                NamedColor::BrightBlack => PackedColor::indexed(8),
                NamedColor::BrightRed => PackedColor::indexed(9),
                NamedColor::BrightGreen => PackedColor::indexed(10),
                NamedColor::BrightYellow => PackedColor::indexed(11),
                NamedColor::BrightBlue => PackedColor::indexed(12),
                NamedColor::BrightMagenta => PackedColor::indexed(13),
                NamedColor::BrightCyan => PackedColor::indexed(14),
                NamedColor::BrightWhite => PackedColor::indexed(15),
                _ => PackedColor::DEFAULT_FG,
            },
            Color::Spec(rgb) => PackedColor::rgb(rgb.r, rgb.g, rgb.b),
            Color::Indexed(idx) => PackedColor::indexed(idx),
        }
    }
}

impl PackedColor {
    /// Convert to egui Color32 for rendering
    pub fn to_egui(&self) -> egui::Color32 {
        if self.is_rgb() {
            let (r, g, b) = self.rgb_components();
            egui::Color32::from_rgb(r, g, b)
        } else if self.is_indexed() {
            let idx = self.indexed_index();
            let (r, g, b) = index_to_rgb(idx);
            egui::Color32::from_rgb(r, g, b)
        } else {
            egui::Color32::from_rgb(220, 220, 220)
        }
    }

    /// Convert to egui Color32 using theme colors
    pub fn to_egui_with_theme(&self, theme: &crate::config::Theme) -> egui::Color32 {
        if self.is_rgb() {
            let (r, g, b) = self.rgb_components();
            egui::Color32::from_rgb(r, g, b)
        } else if self.is_indexed() {
            theme.indexed_color(self.indexed_index())
        } else {
            theme.terminal_fg()
        }
    }

    /// Convert to background color for rendering
    pub fn to_egui_bg(&self) -> egui::Color32 {
        if self.is_rgb() {
            let (r, g, b) = self.rgb_components();
            egui::Color32::from_rgb(r, g, b)
        } else if self.is_indexed() {
            let idx = self.indexed_index();
            let (r, g, b) = index_to_rgb(idx);
            egui::Color32::from_rgb(r, g, b)
        } else {
            egui::Color32::from_rgb(30, 30, 30)
        }
    }

    /// Convert to background color using theme
    pub fn to_egui_bg_with_theme(&self, theme: &crate::config::Theme) -> egui::Color32 {
        if self.is_rgb() {
            let (r, g, b) = self.rgb_components();
            egui::Color32::from_rgb(r, g, b)
        } else if self.is_indexed() {
            theme.indexed_color(self.indexed_index())
        } else {
            theme.terminal_bg()
        }
    }

    pub fn is_default_bg(&self) -> bool {
        self.0 == Self::DEFAULT_BG.0
    }
}

fn index_to_rgb(idx: u8) -> (u8, u8, u8) {
    match idx {
        0 => (0, 0, 0),
        1 => (128, 0, 0),
        2 => (0, 128, 0),
        3 => (128, 128, 0),
        4 => (0, 0, 128),
        5 => (128, 0, 128),
        6 => (0, 128, 128),
        7 => (192, 192, 192),
        8 => (128, 128, 128),
        9 => (255, 0, 0),
        10 => (0, 255, 0),
        11 => (255, 255, 0),
        12 => (0, 0, 255),
        13 => (255, 0, 255),
        14 => (0, 255, 255),
        15 => (255, 255, 255),
        16..=231 => {
            let n = idx - 16;
            let r = (n / 36) % 6;
            let g = (n / 6) % 6;
            let b = n % 6;
            (
                if r == 0 { 0 } else { r * 40 + 55 },
                if g == 0 { 0 } else { g * 40 + 55 },
                if b == 0 { 0 } else { b * 40 + 55 },
            )
        }
        232..=255 => {
            let gray = (idx - 232) * 10 + 8;
            (gray, gray, gray)
        }
    }
}

// ============================================================================
// Cell - compatible with the old implementation
// ============================================================================

#[derive(Clone, Copy, Debug, Default)]
pub struct Cell {
    pub char: char,
    pub fg: PackedColor,
    pub bg: PackedColor,
    pub flags: CellFlags,
    pub width: u8,
    /// True if this is a spacer cell (second cell of a wide character)
    pub is_wide_spacer: bool,
}

impl Cell {
    pub fn new(ch: char) -> Self {
        Self { char: ch, width: 1, is_wide_spacer: false, ..Default::default() }
    }

    pub fn blank() -> Self {
        Self { char: ' ', width: 1, is_wide_spacer: false, ..Default::default() }
    }
}

impl From<alacritty_terminal::term::cell::Cell> for Cell {
    fn from(cell: alacritty_terminal::term::cell::Cell) -> Self {
        let flags = CellFlags::from(cell.flags);
        let is_spacer = cell.flags.contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER);
        Self {
            char: cell.c,
            fg: PackedColor::from(cell.fg),
            bg: PackedColor::from(cell.bg),
            flags,
            width: if flags.contains(CellFlags::WIDE_CHAR) { 2 } else { 1 },
            is_wide_spacer: is_spacer,
        }
    }
}

// ============================================================================
// Terminal size
// ============================================================================

#[derive(Clone, Copy, Debug, Default)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
}

impl TermSize {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self { cols, rows }
    }
}

/// Default scrollback history size (number of lines)
const DEFAULT_HISTORY_SIZE: usize = 10000;

/// Dimensions implementation for Term::new
struct TerminalDimensions {
    cols: usize,
    rows: usize,
    history_size: usize,
}

impl Dimensions for TerminalDimensions {
    fn total_lines(&self) -> usize { self.rows + self.history_size }
    fn screen_lines(&self) -> usize { self.rows }
    fn columns(&self) -> usize { self.cols }
    fn history_size(&self) -> usize { self.history_size }
}

// ============================================================================
// Event handling - minimal implementation
// ============================================================================

pub struct TerminalEventProxy {
    _sender: crossbeam_channel::Sender<()>,
}

impl EventListener for TerminalEventProxy {
    fn send_event(&self, _event: Event) {
        // We don't need to handle events for basic rendering
    }
}

// ============================================================================
// Grid wrapper for rendering
// ============================================================================

pub struct TerminalGrid {
    term: Arc<FairMutex<Term<TerminalEventProxy>>>,
}

impl TerminalGrid {
    pub fn new(term: Arc<FairMutex<Term<TerminalEventProxy>>>) -> Self {
        Self { term }
    }

    /// Get cell at viewport position (row, col)
    /// Viewport row 0 is the top of the screen
    /// Viewport row (screen_lines-1) is the bottom of the screen
    pub fn cell(&self, row: usize, col: usize) -> Cell {
        let term = self.term.lock();
        let grid = term.grid();
        let screen_lines = grid.screen_lines();
        let columns = grid.columns();

        if row < screen_lines && col < columns {
            let display_offset = grid.display_offset();

            // From alacritty vi_mode.rs:
            // Top of viewport: Line(-display_offset)
            // Bottom of viewport: Line(-display_offset + screen_lines - 1)
            //
            // So for viewport row N:
            // grid_line = -display_offset + N
            let grid_line = Line(row as i32 - display_offset as i32);
            let point = Point::new(grid_line, Column(col));
            grid[point].clone().into()
        } else {
            Cell::blank()
        }
    }

    pub fn col_count(&self) -> usize {
        self.term.lock().grid().columns()
    }

    pub fn row_count(&self) -> usize {
        self.term.lock().grid().screen_lines()
    }
}

// ============================================================================
// Cursor
// ============================================================================

#[derive(Clone, Copy, Debug, Default)]
pub struct Cursor {
    pub col: usize,
    pub row: usize,
    pub visible: bool,
}

impl Cursor {
    pub fn new(col: usize, row: usize) -> Self {
        Self { col, row, visible: true }
    }
}

// ============================================================================
// Terminal - main wrapper (no PTY, just VTE parsing)
// ============================================================================

pub struct Terminal {
    term: Arc<FairMutex<Term<TerminalEventProxy>>>,
    parser: Processor,
    size: TermSize,
    title: String,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        let (_tx, _rx) = crossbeam_channel::unbounded();
        let event_proxy = TerminalEventProxy { _sender: _tx };

        let dimensions = TerminalDimensions { cols, rows, history_size: DEFAULT_HISTORY_SIZE };

        let term = Arc::new(FairMutex::new(Term::new(
            Config::default(),
            &dimensions,
            event_proxy,
        )));

        Self {
            term,
            parser: Processor::new(),
            size: TermSize::new(cols as u16, rows as u16),
            title: String::new(),
        }
    }

    /// Process output from PTY/SSH backend
    pub fn process_output(&mut self, data: &[u8]) {
        let mut term = self.term.lock();
        self.parser.advance(&mut *term, data);
    }

    /// Scroll terminal history up/down
    /// Positive delta = scroll up (view older content)
    /// Negative delta = scroll down (view newer content)
    pub fn scroll(&mut self, delta: i32) {
        use alacritty_terminal::grid::Scroll;
        let mut term = self.term.lock();
        term.grid_mut().scroll_display(Scroll::Delta(delta));
    }

    /// Get current scroll offset (0 = at bottom, viewing latest content)
    pub fn scroll_offset(&self) -> usize {
        let term = self.term.lock();
        term.grid().display_offset()
    }

    /// Get total history size
    pub fn history_size(&self) -> usize {
        let term = self.term.lock();
        term.grid().history_size()
    }

    /// Set scroll offset directly (for scrollbar)
    pub fn set_scroll_offset(&mut self, offset: usize) {
        use alacritty_terminal::grid::Scroll;
        let term = self.term.lock();
        let current = term.grid().display_offset();
        drop(term);

        // Calculate delta needed to reach target offset
        let delta = offset as i32 - current as i32;
        if delta != 0 {
            let mut term = self.term.lock();
            term.grid_mut().scroll_display(Scroll::Delta(delta));
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.size = TermSize::new(cols as u16, rows as u16);
        let dimensions = TerminalDimensions { cols, rows, history_size: DEFAULT_HISTORY_SIZE };
        let mut term = self.term.lock();
        term.resize(dimensions);
    }

    pub fn grid(&self) -> TerminalGrid {
        TerminalGrid::new(self.term.clone())
    }

    pub fn size(&self) -> (usize, usize) {
        (self.size.cols as usize, self.size.rows as usize)
    }

    pub fn cursor(&self) -> Cursor {
        let term = self.term.lock();
        let grid = term.grid();
        let point = grid.cursor.point;
        let display_offset = grid.display_offset();
        let screen_lines = grid.screen_lines();

        // From cell(): grid_line = viewport_row - display_offset
        // So: viewport_row = grid_line + display_offset
        let viewport_row = (point.line.0 + display_offset as i32) as usize;

        // Only show cursor if it's in visible range (0 to screen_lines-1)
        let viewport_row = if viewport_row < screen_lines {
            viewport_row
        } else {
            // Cursor not visible (scrolled away), return position off-screen
            screen_lines
        };

        Cursor::new(point.column.0, viewport_row)
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn content(&self) -> String {
        let term = self.term.lock();
        let grid = term.grid();
        let display_offset = grid.display_offset();
        let mut result = String::new();

        for row in 0..grid.screen_lines() {
            let grid_line = row as i32 - display_offset as i32;
            for col in 0..grid.columns() {
                let point = Point::new(Line(grid_line), Column(col));
                let cell = &grid[point];
                if cell.c != ' ' && cell.c != '\0' {
                    result.push(cell.c);
                }
            }
            result.push('\n');
        }

        result
    }

    pub fn handle_key(&mut self, key: Key, modifiers: egui::Modifiers) -> Option<Vec<u8>> {
        use Key::*;

        if modifiers.ctrl && !modifiers.alt && !modifiers.shift {
            if let Some(c) = key_to_char(key) {
                if c.is_ascii_lowercase() {
                    return Some(vec![(c as u8) - b'a' + 1]);
                }
                if c.is_ascii_uppercase() {
                    return Some(vec![(c as u8) - b'A' + 1]);
                }
            }
            match key {
                Space => return Some(vec![0]),
                OpenBracket => return Some(vec![27]),
                Backslash => return Some(vec![28]),
                CloseBracket => return Some(vec![29]),
                _ => {}
            }
        }

        let term = self.term.lock();
        let app_cursor = term.mode().contains(TermMode::APP_CURSOR);
        drop(term);

        match key {
            ArrowUp => return Some(if app_cursor { b"\x1bOA".to_vec() } else { b"\x1b[A".to_vec() }),
            ArrowDown => return Some(if app_cursor { b"\x1bOB".to_vec() } else { b"\x1b[B".to_vec() }),
            ArrowRight => return Some(if app_cursor { b"\x1bOC".to_vec() } else { b"\x1b[C".to_vec() }),
            ArrowLeft => return Some(if app_cursor { b"\x1bOD".to_vec() } else { b"\x1b[D".to_vec() }),
            Home => return Some(b"\x1b[H".to_vec()),
            End => return Some(b"\x1b[F".to_vec()),
            Insert => return Some(b"\x1b[2~".to_vec()),
            Delete => return Some(b"\x1b[3~".to_vec()),
            PageUp => return Some(b"\x1b[5~".to_vec()),
            PageDown => return Some(b"\x1b[6~".to_vec()),
            Tab => return Some(b"\t".to_vec()),
            Enter => return Some(b"\r".to_vec()),
            Backspace => return Some(if modifiers.ctrl { b"\x08".to_vec() } else { b"\x7f".to_vec() }),
            Escape => return Some(b"\x1b".to_vec()),
            _ => {}
        }

        match key {
            F1 => return Some(b"\x1b[11~".to_vec()),
            F2 => return Some(b"\x1b[12~".to_vec()),
            F3 => return Some(b"\x1b[13~".to_vec()),
            F4 => return Some(b"\x1b[14~".to_vec()),
            F5 => return Some(b"\x1b[15~".to_vec()),
            F6 => return Some(b"\x1b[17~".to_vec()),
            F7 => return Some(b"\x1b[18~".to_vec()),
            F8 => return Some(b"\x1b[19~".to_vec()),
            F9 => return Some(b"\x1b[20~".to_vec()),
            F10 => return Some(b"\x1b[21~".to_vec()),
            F11 => return Some(b"\x1b[23~".to_vec()),
            F12 => return Some(b"\x1b[24~".to_vec()),
            _ => {}
        }

        None
    }

    pub fn handle_text(&self, text: &str) -> Vec<u8> {
        text.as_bytes().to_vec()
    }
}

fn key_to_char(key: Key) -> Option<char> {
    match key {
        Key::A => Some('a'), Key::B => Some('b'), Key::C => Some('c'),
        Key::D => Some('d'), Key::E => Some('e'), Key::F => Some('f'),
        Key::G => Some('g'), Key::H => Some('h'), Key::I => Some('i'),
        Key::J => Some('j'), Key::K => Some('k'), Key::L => Some('l'),
        Key::M => Some('m'), Key::N => Some('n'), Key::O => Some('o'),
        Key::P => Some('p'), Key::Q => Some('q'), Key::R => Some('r'),
        Key::S => Some('s'), Key::T => Some('t'), Key::U => Some('u'),
        Key::V => Some('v'), Key::W => Some('w'), Key::X => Some('x'),
        Key::Y => Some('y'), Key::Z => Some('z'),
        _ => None,
    }
}