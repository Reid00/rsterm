use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Osc52, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, CursorStyle, NamedColor, Processor};
use bitflags::bitflags;

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct CellFlags: u16 {
        const BOLD = 1;
        const ITALIC = 1 << 1;
        const UNDERLINE = 1 << 2;
        const REVERSE = 1 << 3;
        const DIM = 1 << 4;
        const STRIKETHROUGH = 1 << 5;
        const WIDE_CHAR = 1 << 6;
        const HIDDEN = 1 << 7;
        const UNDERCURL = 1 << 8;
        const DOUBLE_UNDERLINE = 1 << 9;
    }
}

impl From<Flags> for CellFlags {
    fn from(flags: Flags) -> Self {
        let mut result = Self::empty();
        for (source, destination) in [
            (Flags::BOLD, Self::BOLD),
            (Flags::ITALIC, Self::ITALIC),
            (Flags::ALL_UNDERLINES, Self::UNDERLINE),
            (Flags::INVERSE, Self::REVERSE),
            (Flags::DIM, Self::DIM),
            (Flags::STRIKEOUT, Self::STRIKETHROUGH),
            (Flags::WIDE_CHAR, Self::WIDE_CHAR),
            (Flags::HIDDEN, Self::HIDDEN),
            (Flags::UNDERCURL, Self::UNDERCURL),
            (Flags::DOUBLE_UNDERLINE, Self::DOUBLE_UNDERLINE),
        ] {
            if flags.intersects(source) {
                result.insert(destination);
            }
        }
        result
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackedColor {
    Named(NamedColor),
    Indexed(u8),
    Rgb([u8; 3]),
}

impl From<Color> for PackedColor {
    fn from(color: Color) -> Self {
        match color {
            Color::Named(named) => Self::Named(named),
            Color::Indexed(index) => Self::Indexed(index),
            Color::Spec(rgb) => Self::Rgb([rgb.r, rgb.g, rgb.b]),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub text: String,
    pub fg: PackedColor,
    pub bg: PackedColor,
    pub flags: CellFlags,
    pub width: u8,
    pub is_wide_spacer: bool,
    pub selected: bool,
}

impl Cell {
    fn from_grid(cell: &alacritty_terminal::term::cell::Cell, selected: bool) -> Self {
        let mut text = String::from(cell.c);
        if let Some(combining) = cell.zerowidth() {
            text.extend(combining);
        }
        Self {
            text,
            fg: cell.fg.into(),
            bg: cell.bg.into(),
            flags: cell.flags.into(),
            width: if cell.flags.contains(Flags::WIDE_CHAR) {
                2
            } else {
                1
            },
            is_wide_spacer: cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
            selected,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorKind {
    #[default]
    Block,
    Beam,
    Underline,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Cursor {
    pub col: usize,
    pub row: usize,
    pub visible: bool,
    pub kind: CursorKind,
    pub blinking: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TerminalModes {
    pub application_cursor: bool,
    pub application_keypad: bool,
    pub bracketed_paste: bool,
    pub alternate_screen: bool,
    pub focus_reporting: bool,
    pub line_feed_new_line: bool,
}

impl From<TermMode> for TerminalModes {
    fn from(mode: TermMode) -> Self {
        Self {
            application_cursor: mode.contains(TermMode::APP_CURSOR),
            application_keypad: mode.contains(TermMode::APP_KEYPAD),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
            alternate_screen: mode.contains(TermMode::ALT_SCREEN),
            focus_reporting: mode.contains(TermMode::FOCUS_IN_OUT),
            line_feed_new_line: mode.contains(TermMode::LINE_FEED_NEW_LINE),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TerminalSnapshot {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
    pub cursor: Cursor,
    pub modes: TerminalModes,
    pub display_offset: usize,
    pub history_size: usize,
    pub color_overrides: Vec<Option<[u8; 3]>>,
}

impl TerminalSnapshot {
    pub fn cell(&self, row: usize, col: usize) -> &Cell {
        &self.cells[row * self.cols + col]
    }
}

struct TerminalDimensions {
    cols: usize,
    rows: usize,
}

impl Dimensions for TerminalDimensions {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

struct TerminalEventProxy(crossbeam_channel::Sender<Event>);

impl EventListener for TerminalEventProxy {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

pub struct Terminal {
    term: Arc<FairMutex<Term<TerminalEventProxy>>>,
    parser: Processor,
    events: crossbeam_channel::Receiver<Event>,
    pending_events: Vec<Event>,
    title: String,
    config: Config,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self::with_history(cols, rows, 10_000)
    }

    pub fn with_history(cols: usize, rows: usize, history: usize) -> Self {
        let (sender, events) = crossbeam_channel::unbounded();
        let dimensions = TerminalDimensions {
            cols: cols.clamp(2, u16::MAX as usize),
            rows: rows.clamp(1, u16::MAX as usize),
        };
        let config = Config {
            scrolling_history: history,
            // Remote processes must not read or overwrite the local clipboard.
            osc52: Osc52::Disabled,
            ..Config::default()
        };
        Self {
            term: Arc::new(FairMutex::new(Term::new(
                config.clone(),
                &dimensions,
                TerminalEventProxy(sender),
            ))),
            parser: Processor::new(),
            events,
            pending_events: Vec::new(),
            title: String::new(),
            config,
        }
    }

    pub fn process_output(&mut self, data: &[u8]) {
        {
            let mut term = self.term.lock();
            self.parser.advance(&mut *term, data);
        }
        self.collect_events();
    }

    fn collect_events(&mut self) {
        for event in self.events.try_iter() {
            match event {
                Event::Title(title) => self.title = title,
                Event::ResetTitle => self.title.clear(),
                Event::PtyWrite(..) | Event::ColorRequest(..) | Event::TextAreaSizeRequest(..) => {
                    self.pending_events.push(event);
                }
                _ => {}
            }
        }
    }

    pub fn drain_replies(&mut self) -> Vec<Vec<u8>> {
        let mut replies = Vec::new();
        self.pending_events.retain(|event| {
            if let Event::PtyWrite(text) = event {
                replies.push(text.as_bytes().to_vec());
                false
            } else {
                true
            }
        });
        replies
    }

    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.pending_events)
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn size(&self) -> (usize, usize) {
        let term = self.term.lock();
        (term.columns(), term.screen_lines())
    }

    pub fn modes(&self) -> TerminalModes {
        (*self.term.lock().mode()).into()
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.term.lock().resize(TerminalDimensions {
            cols: cols.clamp(2, u16::MAX as usize),
            rows: rows.clamp(1, u16::MAX as usize),
        });
    }

    pub fn configure(&mut self, history: usize, cursor: CursorKind, blinking: bool) {
        self.config.scrolling_history = history;
        self.config.default_cursor_style = CursorStyle {
            shape: match cursor {
                CursorKind::Block => CursorShape::Block,
                CursorKind::Beam => CursorShape::Beam,
                CursorKind::Underline => CursorShape::Underline,
            },
            blinking,
        };
        self.term.lock().set_options(self.config.clone());
        self.collect_events();
    }

    pub fn scroll(&mut self, delta: i32) {
        self.term.lock().scroll_display(Scroll::Delta(delta));
        self.collect_events();
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.lock().scroll_display(Scroll::Bottom);
        self.collect_events();
    }

    pub fn clear_selection(&mut self) {
        self.term.lock().selection = None;
    }

    pub fn start_selection(&mut self, row: usize, col: usize, right_half: bool, clicks: usize) {
        let mut term = self.term.lock();
        let point = viewport_point(&term, row, col);
        let ty = match clicks {
            2 => SelectionType::Semantic,
            3.. => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        term.selection = Some(Selection::new(ty, point, selection_side(right_half)));
    }

    pub fn update_selection(&mut self, row: usize, col: usize, right_half: bool) {
        let mut term = self.term.lock();
        let point = viewport_point(&term, row, col);
        if let Some(selection) = &mut term.selection {
            selection.update(point, selection_side(right_half));
        }
    }

    pub fn select_all(&mut self) {
        let mut term = self.term.lock();
        let top = Point::new(Line(-(term.grid().history_size() as i32)), Column(0));
        let bottom = Point::new(
            Line(term.screen_lines() as i32 - 1),
            Column(term.columns() - 1),
        );
        let mut selection = Selection::new(SelectionType::Simple, top, Side::Left);
        selection.update(bottom, Side::Right);
        term.selection = Some(selection);
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term
            .lock()
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        let term = self.term.lock();
        let grid = term.grid();
        let cols = grid.columns();
        let rows = grid.screen_lines();
        let content = term.renderable_content();
        let mut cells = Vec::with_capacity(cols * rows);
        for row in 0..rows {
            for col in 0..cols {
                let point = Point::new(
                    Line(row as i32 - content.display_offset as i32),
                    Column(col),
                );
                let cell = &grid[point];
                let selected = content.selection.is_some_and(|range| {
                    range.contains(point)
                        || (cell.flags.contains(Flags::WIDE_CHAR)
                            && range.contains(Point::new(point.line, Column(col + 1))))
                        || (cell.flags.contains(Flags::WIDE_CHAR_SPACER)
                            && col > 0
                            && range.contains(Point::new(point.line, Column(col - 1))))
                });
                cells.push(Cell::from_grid(cell, selected));
            }
        }
        let cursor_row = content.cursor.point.line.0 + content.display_offset as i32;
        let style = term.cursor_style();
        TerminalSnapshot {
            cols,
            rows,
            cells,
            cursor: Cursor {
                col: content.cursor.point.column.0,
                row: cursor_row.max(0) as usize,
                visible: content.cursor.shape != CursorShape::Hidden
                    && cursor_row >= 0
                    && cursor_row < rows as i32,
                kind: match content.cursor.shape {
                    CursorShape::Beam => CursorKind::Beam,
                    CursorShape::Underline => CursorKind::Underline,
                    _ => CursorKind::Block,
                },
                blinking: style.blinking,
            },
            modes: content.mode.into(),
            display_offset: content.display_offset,
            history_size: grid.history_size(),
            color_overrides: (0..alacritty_terminal::term::color::COUNT)
                .map(|index| content.colors[index].map(|rgb| [rgb.r, rgb.g, rgb.b]))
                .collect(),
        }
    }

    pub fn content(&self) -> String {
        let snapshot = self.snapshot();
        let mut result = String::new();
        for row in 0..snapshot.rows {
            let mut line = String::new();
            for col in 0..snapshot.cols {
                let cell = snapshot.cell(row, col);
                if !cell.is_wide_spacer {
                    line.push_str(&cell.text);
                }
            }
            result.push_str(line.trim_end());
            result.push('\n');
        }
        result
    }

    pub fn paste_bytes(&self, text: &str) -> Vec<u8> {
        if self.modes().bracketed_paste {
            // ESC inside a paste could terminate bracketed mode in the remote application.
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        }
    }
}

fn viewport_point(term: &Term<TerminalEventProxy>, row: usize, col: usize) -> Point {
    Point::new(
        Line(row.min(term.screen_lines() - 1) as i32 - term.grid().display_offset() as i32),
        Column(col.min(term.columns() - 1)),
    )
}

fn selection_side(right_half: bool) -> Side {
    if right_half {
        Side::Right
    } else {
        Side::Left
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_preserves_spaces_styles_and_split_utf8() {
        let mut term = Terminal::new(20, 3);
        term.process_output(b"a b \x1b[1;3;4;31m");
        let bytes = "中e\u{301}".as_bytes();
        term.process_output(&bytes[..1]);
        term.process_output(&bytes[1..]);
        let frame = term.snapshot();
        assert_eq!(frame.cell(0, 4).text, "中");
        assert_eq!(frame.cell(0, 4).width, 2);
        assert!(frame.cell(0, 5).is_wide_spacer);
        assert_eq!(frame.cell(0, 6).text, "e\u{301}");
        assert!(frame
            .cell(0, 6)
            .flags
            .contains(CellFlags::BOLD | CellFlags::ITALIC | CellFlags::UNDERLINE));
        assert!(term.content().starts_with("a b 中e\u{301}"));
    }

    #[test]
    fn output_updates_title_cursor_and_returns_device_replies() {
        let mut term = Terminal::new(80, 24);
        term.process_output(b"\x1b]0;server\x07\x1b[?25l\x1b[6n");
        assert_eq!(term.title(), "server");
        assert!(!term.snapshot().cursor.visible);
        assert_eq!(term.drain_replies(), vec![b"\x1b[1;1R".to_vec()]);
        assert!(term.drain_replies().is_empty());
        term.process_output(b"\x1b[?25h");
        assert!(term.snapshot().cursor.visible);
    }

    #[test]
    fn selecting_either_half_highlights_the_entire_wide_character() {
        let mut term = Terminal::new(10, 2);
        term.process_output("中a".as_bytes());
        for col in [0, 1] {
            term.start_selection(0, col, false, 1);
            term.update_selection(0, col, true);
            let frame = term.snapshot();
            assert!(frame.cell(0, 0).selected);
            assert!(frame.cell(0, 1).selected);
            assert!(!frame.cell(0, 2).selected);
        }
    }

    #[test]
    fn query_events_remain_ordered_and_resize_updates_device_status() {
        let mut term = Terminal::new(80, 24);
        term.resize(101, 37);
        term.process_output(b"\x1b[37;101H\x1b[6n\x1b]10;?\x07\x1b[14t\x1b[6n");
        let events = term.drain_events();
        assert_eq!(events.len(), 4);
        assert!(matches!(&events[0], Event::PtyWrite(reply) if reply == "\x1b[37;101R"));
        assert!(matches!(&events[1], Event::ColorRequest(256, _)));
        assert!(matches!(&events[2], Event::TextAreaSizeRequest(_)));
        assert!(matches!(&events[3], Event::PtyWrite(reply) if reply == "\x1b[37;101R"));
        assert!(term.drain_events().is_empty());
    }

    #[test]
    fn selection_handles_soft_wrap_and_wide_cells() {
        let mut term = Terminal::new(6, 3);
        term.process_output("ab中cdef".as_bytes());
        term.start_selection(0, 0, false, 1);
        term.update_selection(1, 1, true);
        assert_eq!(term.selection_text().as_deref(), Some("ab中cdef"));
    }

    #[test]
    fn scrollback_configuration_and_cursor_visibility() {
        let mut term = Terminal::with_history(10, 2, 3);
        term.process_output(b"1\r\n2\r\n3\r\n4\r\n5\r\n6");
        term.scroll(3);
        let frame = term.snapshot();
        assert_eq!(frame.history_size, 3);
        assert_eq!(frame.display_offset, 3);
        assert!(!frame.cursor.visible);
        term.scroll_to_bottom();
        assert!(term.snapshot().cursor.visible);
        term.configure(1, CursorKind::Beam, false);
        assert_eq!(term.snapshot().history_size, 1);
    }

    #[test]
    fn alternate_screen_restore_and_selection_follow_viewport_history() {
        let mut term = Terminal::new(10, 2);
        term.process_output(b"one\r\ntwo\r\nthree");
        term.scroll(1);
        term.start_selection(0, 0, false, 1);
        term.update_selection(1, 2, true);
        assert_eq!(term.selection_text().as_deref(), Some("one\ntwo"));
        term.clear_selection();
        term.scroll_to_bottom();
        term.process_output(b"\x1b[?1049h\x1b[Hother");
        assert!(term.snapshot().modes.alternate_screen);
        assert!(term.content().starts_with("other"));
        term.process_output(b"\x1b[?1049l");
        assert!(!term.snapshot().modes.alternate_screen);
        assert!(term.content().contains("three"));
    }

    #[test]
    fn paste_and_application_modes_follow_escape_sequences() {
        let mut term = Terminal::new(10, 2);
        assert_eq!(term.paste_bytes("a\nb"), b"a\rb");
        term.process_output(b"\x1b[?2004h\x1b[?1h\x1b=");
        assert!(term.modes().application_cursor);
        assert!(term.modes().application_keypad);
        assert_eq!(term.paste_bytes("a\nb\x1b"), b"\x1b[200~a\nb\x1b[201~");
    }
}
