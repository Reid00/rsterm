// src/channel.rs
//! Channel types for UI-backend communication.
//!
//! Re-exports types from the backend module and provides helper types.

// Re-export the main channel types from backend
pub use crate::backend::{AppCommand, BackendEvent};

use tokio::sync::mpsc;

pub type SessionId = String;
pub type TabId = u64;

#[derive(Clone, Copy, Debug)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

/// All cross-thread channels for UI-backend communication
pub struct AppChannels {
    pub command_tx: mpsc::Sender<AppCommand>,
    pub command_rx: mpsc::Receiver<AppCommand>,
    pub event_tx: mpsc::Sender<BackendEvent>,
    pub event_rx: mpsc::Receiver<BackendEvent>,
}

impl AppChannels {
    pub fn new() -> Self {
        let (command_tx, command_rx) = mpsc::channel(256);
        let (event_tx, event_rx) = mpsc::channel(256);
        Self { command_tx, command_rx, event_tx, event_rx }
    }
}

impl Default for AppChannels {
    fn default() -> Self {
        Self::new()
    }
}