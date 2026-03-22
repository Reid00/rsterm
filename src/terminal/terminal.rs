// src/terminal/terminal.rs
use egui::Key;

use crate::terminal::{Cursor, Grid, TerminalParser, TerminalMode};

pub struct Terminal {
    parser: TerminalParser,
    title: String,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            parser: TerminalParser::new(cols, rows),
            title: String::new(),
        }
    }

    pub fn process_output(&mut self, data: &[u8]) {
        self.parser.process(data);
    }

    pub fn grid(&self) -> &Grid { self.parser.grid() }
    pub fn cursor(&self) -> &Cursor { self.parser.cursor() }
    pub fn mode(&self) -> TerminalMode { self.parser.mode() }

    pub fn size(&self) -> (usize, usize) {
        (self.parser.grid().col_count(), self.parser.grid().row_count())
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.parser.resize(cols, rows);
    }

    pub fn title(&self) -> &str { &self.title }

    /// Handle keyboard input, returns bytes to send to PTY
    pub fn handle_key(&mut self, key: Key, modifiers: egui::Modifiers) -> Option<Vec<u8>> {
        use Key::*;

        // Ctrl key combinations
        if modifiers.ctrl && !modifiers.alt && !modifiers.shift {
            if let Some(c) = key_to_char(key) {
                if c >= 'a' && c <= 'z' {
                    return Some(vec![(c as u8) - b'a' + 1]);
                }
                if c >= 'A' && c <= 'Z' {
                    return Some(vec![(c as u8) - b'A' + 1]);
                }
            }
            match key {
                Space => return Some(vec![0]),
                OpenBracket => return Some(vec![27]), // ESC (Ctrl+[)
                Backslash => return Some(vec![28]),   // Ctrl+\
                CloseBracket => return Some(vec![29]), // Ctrl+]
                _ => {}
            }
        }

        // Arrow keys and special keys
        let app_cursor = self.parser.mode().contains(TerminalMode::CURSOR_KEYS_APPLICATION);
        match key {
            ArrowUp => {
                return Some(if app_cursor { b"\x1bOA".to_vec() } else { b"\x1b[A".to_vec() });
            }
            ArrowDown => {
                return Some(if app_cursor { b"\x1bOB".to_vec() } else { b"\x1b[B".to_vec() });
            }
            ArrowRight => {
                return Some(if app_cursor { b"\x1bOC".to_vec() } else { b"\x1b[C".to_vec() });
            }
            ArrowLeft => {
                return Some(if app_cursor { b"\x1bOD".to_vec() } else { b"\x1b[D".to_vec() });
            }
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

        // Function keys F1-F12
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

    /// Handle text input (from IME or regular typing)
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