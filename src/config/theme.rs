// src/config/theme.rs
use serde::{Deserialize, Serialize};
use egui::Color32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    #[serde(default)]
    pub ui: UiColors,
    #[serde(default)]
    pub terminal: TerminalColorPalette,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiColors {
    pub background: [u8; 3],
    pub foreground: [u8; 3],
    pub tab_active_bg: [u8; 3],
    pub tab_inactive_bg: [u8; 3],
    pub panel_bg: [u8; 3],
    pub selection_bg: [u8; 4],
    pub statusbar_bg: [u8; 3],
    pub accent: [u8; 3],
    pub error: [u8; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalColorPalette {
    #[serde(default = "default_ansi_colors")]
    pub ansi: [[u8; 3]; 16],
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub cursor: [u8; 3],
}

fn default_ansi_colors() -> [[u8; 3]; 16] {
    [
        [0x00, 0x00, 0x00], // Black
        [0xcd, 0x00, 0x00], // Red
        [0x00, 0xcd, 0x00], // Green
        [0xcd, 0xcd, 0x00], // Yellow
        [0x00, 0x00, 0xee], // Blue
        [0xcd, 0x00, 0xcd], // Magenta
        [0x00, 0xcd, 0xcd], // Cyan
        [0xe5, 0xe5, 0xe5], // White
        [0x7f, 0x7f, 0x7f], // Bright Black
        [0xff, 0x00, 0x00], // Bright Red
        [0x00, 0xff, 0x00], // Bright Green
        [0xff, 0xff, 0x00], // Bright Yellow
        [0x5c, 0x5c, 0xff], // Bright Blue
        [0xff, 0x00, 0xff], // Bright Magenta
        [0x00, 0xff, 0xff], // Bright Cyan
        [0xff, 0xff, 0xff], // Bright White
    ]
}

impl Default for TerminalColorPalette {
    fn default() -> Self {
        Self {
            ansi: default_ansi_colors(),
            fg: [0xe5, 0xe5, 0xe5],
            bg: [0x1a, 0x1a, 0x1a],
            cursor: [0xff, 0xff, 0xff],
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::default_dark()
    }
}

impl Theme {
    pub fn default_dark() -> Self {
        Self {
            name: "dark".into(),
            ui: UiColors {
                background: [0x1a, 0x1a, 0x1a],
                foreground: [0xe5, 0xe5, 0xe5],
                tab_active_bg: [0x2d, 0x2d, 0x2d],
                tab_inactive_bg: [0x1a, 0x1a, 0x1a],
                panel_bg: [0x25, 0x25, 0x25],
                selection_bg: [0x40, 0x80, 0xc0, 0x80],
                statusbar_bg: [0x2d, 0x2d, 0x2d],
                accent: [0x40, 0x80, 0xc0],
                error: [0xf4, 0x47, 0x47],
            },
            terminal: TerminalColorPalette::default(),
        }
    }

    pub fn terminal_fg(&self) -> Color32 {
        Color32::from_rgb(self.terminal.fg[0], self.terminal.fg[1], self.terminal.fg[2])
    }

    pub fn terminal_bg(&self) -> Color32 {
        Color32::from_rgb(self.terminal.bg[0], self.terminal.bg[1], self.terminal.bg[2])
    }

    pub fn selection_bg(&self) -> Color32 {
        Color32::from_rgba_unmultiplied(
            self.ui.selection_bg[0],
            self.ui.selection_bg[1],
            self.ui.selection_bg[2],
            self.ui.selection_bg[3],
        )
    }

    pub fn indexed_color(&self, idx: u8) -> Color32 {
        if idx < 16 {
            let c = self.terminal.ansi[idx as usize];
            Color32::from_rgb(c[0], c[1], c[2])
        } else if idx < 232 {
            // 6x6x6 color cube
            let idx = idx - 16;
            let r = (idx / 36) % 6;
            let g = (idx / 6) % 6;
            let b = idx % 6;
            Color32::from_rgb(
                if r > 0 { r * 40 + 55 } else { 0 },
                if g > 0 { g * 40 + 55 } else { 0 },
                if b > 0 { b * 40 + 55 } else { 0 },
            )
        } else {
            // Grayscale
            let gray = (idx - 232) * 10 + 8;
            Color32::from_rgb(gray, gray, gray)
        }
    }
}