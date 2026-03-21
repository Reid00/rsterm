// src/terminal/mode.rs
use bitflags::bitflags;

bitflags! {
    #[derive(Clone, Copy, Debug, Default)]
    pub struct TerminalMode: u32 {
        const CURSOR_KEYS_APPLICATION = 0x0001;
        const LINE_FEED_NEW_LINE = 0x0002;
        const INSERT_MODE = 0x0004;
        const AUTO_WRAP = 0x0008;
        const REVERSE_VIDEO = 0x0010;
        const CURSOR_VISIBLE = 0x0020;
        const ALTERNATE_SCREEN = 0x0040;
        const MOUSE_TRACKING = 0x0080;
        const BRACKETED_PASTE = 0x0100;
    }
}