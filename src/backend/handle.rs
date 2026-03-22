// src/backend/handle.rs
use std::sync::Arc;

use tokio::sync::mpsc as tokio_mpsc;

use crate::backend::PtyBackend;
use crate::config::SessionConfig;

/// Handle to a terminal session
pub struct SessionHandle {
    pub id: String,
    pub config: SessionConfig,
    pub backend: Arc<dyn PtyBackend>,
}

/// Commands from the UI to the backend
#[derive(Debug)]
pub enum AppCommand {
    /// Create a new session
    CreateSession {
        session_id: String,
        config: SessionConfig,
        respond_to: tokio_mpsc::Sender<Result<String, String>>,
    },
    /// Write input to a session
    WriteInput {
        session_id: String,
        data: Vec<u8>,
    },
    /// Resize a session
    ResizeSession {
        session_id: String,
        cols: u16,
        rows: u16,
    },
    /// Close a session
    CloseSession {
        session_id: String,
    },
    /// Shutdown the backend
    Shutdown,
}

/// Events from backend to UI
#[derive(Debug)]
pub enum BackendEvent {
    /// Session output received
    SessionOutput {
        session_id: String,
        data: Vec<u8>,
    },
    /// Session closed (PTY died)
    SessionClosed {
        session_id: String,
    },
    /// Session created
    SessionCreated {
        session_id: String,
    },
}