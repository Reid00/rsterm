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

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Cell {
    pub char: char,
    pub fg: PackedColor,
    pub bg: PackedColor,
    pub flags: CellFlags,
    pub width: u8,
}

impl Cell {
    pub fn new(ch: char) -> Self {
        Self { char: ch, width: 1, ..Default::default() }
    }

    pub fn blank() -> Self {
        Self { char: ' ', width: 1, ..Default::default() }
    }
}