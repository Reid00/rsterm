// src/backend/handle.rs
use std::sync::Arc;

use tokio::sync::mpsc as tokio_mpsc;

use crate::backend::{PtyBackend, SshConnectionConfig};
use crate::config::SessionConfig;
use crate::state::FileEntryInfo;

/// Handle to a terminal session
pub struct SessionHandle {
    pub id: String,
    pub config: SessionConfig,
    pub backend: Arc<dyn PtyBackend>,
}

/// Commands from the UI to the backend
#[derive(Debug)]
pub enum AppCommand {
    /// Create a new local session
    CreateSession {
        session_id: String,
        config: SessionConfig,
        respond_to: tokio_mpsc::Sender<Result<String, String>>,
    },
    /// Create a new SSH session
    CreateSshSession {
        session_id: String,
        ssh_config: SshConnectionConfig,
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
    /// Initialize SFTP for an SSH session
    SftpInit {
        session_id: String,
    },
    /// List directory contents via SFTP
    SftpListDir {
        session_id: String,
        path: String,
    },
    /// Download a file via SFTP
    SftpDownload {
        session_id: String,
        remote_path: String,
        local_path: String,
    },
    /// Upload a file via SFTP
    SftpUpload {
        session_id: String,
        local_path: String,
        remote_path: String,
    },
    /// Create directory via SFTP
    SftpMkdir {
        session_id: String,
        path: String,
    },
    /// Delete file or directory via SFTP
    SftpDelete {
        session_id: String,
        path: String,
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
    /// SFTP initialized for session
    SftpReady {
        session_id: String,
    },
    /// SFTP initialization failed
    SftpError {
        session_id: String,
        error: String,
    },
    /// SFTP directory listing result
    SftpListResult {
        session_id: String,
        path: String,
        entries: Vec<FileEntryInfo>,
    },
    /// SFTP transfer progress
    SftpTransferProgress {
        session_id: String,
        transfer_id: u64,
        bytes_transferred: u64,
        total_bytes: u64,
    },
    /// SFTP transfer complete
    SftpTransferComplete {
        session_id: String,
        transfer_id: u64,
        success: bool,
        error: Option<String>,
    },
}