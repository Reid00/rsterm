// src/lib.rs
pub mod terminal_v2;
pub mod backend;
pub mod config;
pub mod ui;
pub mod channel;
pub mod state;
pub mod error;
pub mod platform;
pub mod fonts;
pub mod app;
pub mod ssh;
pub mod sftp;

// Re-export terminal_v2 as terminal for backwards compatibility
pub use terminal_v2 as terminal;