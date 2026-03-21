// src/channel.rs
use std::path::PathBuf;
use tokio::sync::mpsc;

pub type SessionId = String;
pub type TabId = u64;

#[derive(Clone, Copy, Debug)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

/// All cross-thread channels
pub struct AppChannels {
    pub command_tx: mpsc::Sender<AppCommand>,
    pub command_rx: mpsc::Receiver<AppCommand>,
    pub event_tx: mpsc::Sender<AppEvent>,
    pub event_rx: mpsc::Receiver<AppEvent>,
}

impl AppChannels {
    pub fn new() -> Self {
        let (command_tx, command_rx) = mpsc::channel(256);
        let (event_tx, event_rx) = mpsc::channel(256);
        Self { command_tx, command_rx, event_tx, event_rx }
    }
}

/// Per-session data channels
pub struct SessionChannels {
    pub session_id: SessionId,
    pub input_tx: mpsc::Sender<Vec<u8>>,
    pub output_rx: mpsc::Receiver<Vec<u8>>,
}

impl std::fmt::Debug for SessionChannels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionChannels")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

/// Commands from UI to backend
#[derive(Debug)]
pub enum AppCommand {
    Connect {
        session_config: crate::config::SessionConfig,
        terminal_size: Size,
    },
    OpenLocalPty {
        shell: String,
        terminal_size: Size,
    },
    Disconnect { session_id: SessionId },
    Resize {
        session_id: SessionId,
        cols: u16,
        rows: u16,
    },
    SftpListDir {
        session_id: SessionId,
        path: String,
    },
    SftpUpload {
        session_id: SessionId,
        local_path: PathBuf,
        remote_path: String,
    },
    SftpDownload {
        session_id: SessionId,
        remote_path: String,
        local_path: PathBuf,
    },
    SftpFileOp {
        session_id: SessionId,
        op: SftpFileOperation,
    },
    HostKeyResponse {
        session_id: SessionId,
        accepted: bool,
    },
    PasswordResponse {
        session_id: SessionId,
        password: String,
    },
}

#[derive(Debug)]
pub enum SftpFileOperation {
    Mkdir(String),
    Delete(String),
    Rename { from: String, to: String },
}

/// Events from backend to UI
#[derive(Debug)]
pub enum AppEvent {
    Connected {
        session_id: SessionId,
        channels: SessionChannels,
        server_banner: Option<String>,
    },
    Disconnected {
        session_id: SessionId,
        reason: Option<String>,
    },
    Error {
        session_id: Option<SessionId>,
        error: crate::error::AppError,
    },
    HostKeyVerify {
        session_id: SessionId,
        host: String,
        fingerprint: String,
    },
    PasswordRequired {
        session_id: SessionId,
        host: String,
        username: String,
    },
    Reconnecting {
        session_id: SessionId,
        attempt: u32,
        max_attempts: u32,
    },
    SftpDirListed {
        session_id: SessionId,
        path: String,
        entries: Result<Vec<FileInfo>, crate::error::SftpError>,
    },
    SftpTransferProgress {
        session_id: SessionId,
        progress: f32,
        filename: String,
    },
    SftpTransferDone {
        session_id: SessionId,
        result: Result<(), crate::error::SftpError>,
    },
    SftpFileOpDone {
        session_id: SessionId,
        result: Result<(), crate::error::SftpError>,
    },
}

/// File info for SFTP
#[derive(Debug, Clone)]
pub struct FileInfo {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
}