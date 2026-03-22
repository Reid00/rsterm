// src/terminal/cell.rs
use bitflags::bitflags;

bitflags! {
    #[derive(Clone, Copy, Debug, Default)]
    pub struct CellFlags: u8 {
        const BOLD = 0x01;
        const ITALIC = 0x02;
        const UNDERLINE = 0x04;
        const BLINK = 0x08;
        const REVERSE = 0x10;
        const DIM = 0x20;
        const STRIKETHROUGH = 0x40;
    }
}

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

impl PackedColor {
    /// Convert to egui Color32 for rendering
    pub fn to_egui(&self) -> egui::Color32 {
        if self.is_rgb() {
            let (r, g, b) = self.rgb_components();
            egui::Color32::from_rgb(r, g, b)
        } else if self.is_indexed() {
            // Convert 256-color index to RGB using standard xterm palette
            let idx = self.indexed_index();
            let (r, g, b) = index_to_rgb(idx);
            egui::Color32::from_rgb(r, g, b)
        } else {
            // Default colors - use standard terminal defaults
            egui::Color32::from_rgb(200, 200, 200) // Default foreground (light gray)
        }
    }
}

/// Convert a 256-color index to RGB using the xterm color palette
fn index_to_rgb(idx: u8) -> (u8, u8, u8) {
    match idx {
        // Standard colors (0-7)
        0 => (0, 0, 0),         // Black
        1 => (128, 0, 0),       // Red
        2 => (0, 128, 0),       // Green
        3 => (128, 128, 0),     // Yellow
        4 => (0, 0, 128),       // Blue
        5 => (128, 0, 128),     // Magenta
        6 => (0, 128, 128),     // Cyan
        7 => (192, 192, 192),   // White
        // Bright colors (8-15)
        8 => (128, 128, 128),   // Bright Black
        9 => (255, 0, 0),       // Bright Red
        10 => (0, 255, 0),      // Bright Green
        11 => (255, 255, 0),    // Bright Yellow
        12 => (0, 0, 255),      // Bright Blue
        13 => (255, 0, 255),    // Bright Magenta
        14 => (0, 255, 255),    // Bright Cyan
        15 => (255, 255, 255),  // Bright White
        // 216 color cube (16-231)
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
        // Grayscale (232-255)
        232..=255 => {
            let gray = (idx - 232) as u8 * 10 + 8;
            (gray, gray, gray)
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Cell {
    pub char: char,
    pub fg: PackedColor,
    pub bg: PackedColor,
    pub flags: CellFlags,
    pub width: u8,
    _padding: [u8; 2],
}

impl Cell {
    pub fn new(ch: char) -> Self {
        Self { char: ch, width: 1, _padding: [0; 2], ..Default::default() }
    }

    pub fn blank() -> Self {
        Self { char: ' ', width: 1, _padding: [0; 2], ..Default::default() }
    }
}