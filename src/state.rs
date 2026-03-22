// src/state.rs
//! Application state management

use std::time::Instant;

use crate::channel::SessionId;

pub type TabId = u64;

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected {
        connected_at: Instant,
        server_banner: Option<String>,
    },
    Reconnecting {
        attempt: u32,
        max_attempts: u32,
        next_retry_at: Instant,
        last_error: String,
    },
    Error {
        error: String,
        can_retry: bool,
    },
}

#[derive(Debug, Clone)]
pub enum TabKind {
    Ssh { session_config_id: String },
    Local { shell: String },
}

pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub kind: TabKind,
    pub session_id: SessionId,
    pub status: ConnectionState,
    // Terminal rendering is handled in App::sessions HashMap
    /// SFTP panel state (only for SSH sessions)
    pub sftp_panel: SftpPanelState,
}

/// SFTP panel state for file browsing UI
#[derive(Debug, Clone)]
pub struct SftpPanelState {
    pub visible: bool,
    pub current_path: String,
    pub entries: Vec<FileEntryInfo>,
    pub selected: Option<usize>,
    pub loading: bool,
    pub error: Option<String>,
}

impl Default for SftpPanelState {
    fn default() -> Self {
        Self {
            visible: false,
            current_path: String::new(),
            entries: Vec::new(),
            selected: None,
            loading: false,
            error: None,
        }
    }
}

/// File entry for display in the SFTP panel
#[derive(Debug, Clone)]
pub struct FileEntryInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<String>,
}

/// Active SFTP transfer with progress tracking
#[derive(Debug, Clone)]
pub struct SftpTransfer {
    pub id: u64,
    pub filename: String,
    pub direction: TransferDirection,
    pub progress: f32,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
}

/// Direction of file transfer
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransferDirection {
    Upload,
    Download,
}

impl SftpTransfer {
    /// Create a new transfer
    pub fn new(id: u64, filename: String, direction: TransferDirection, total_bytes: u64) -> Self {
        Self {
            id,
            filename,
            direction,
            progress: 0.0,
            bytes_transferred: 0,
            total_bytes,
        }
    }

    /// Update transfer progress
    pub fn update_progress(&mut self, bytes_transferred: u64) {
        self.bytes_transferred = bytes_transferred;
        if self.total_bytes > 0 {
            self.progress = bytes_transferred as f32 / self.total_bytes as f32;
        }
    }
}

pub enum ModalDialog {
    None,
    NewConnection(NewConnectionState),
    NewSession {
        name: String,
        host: String,
        port: u16,
        username: String,
        auth_method: AuthMethodUi,
        password: String,
        key_path: String,
        key_passphrase: String,
        error: Option<String>,
    },
    Settings,
    HostKeyConfirm {
        host: String,
        fingerprint: String,
    },
    PasswordPrompt {
        host: String,
        username: String,
    },
    Error {
        title: String,
        message: String,
    },
}

#[derive(Default)]
pub struct NewConnectionState {
    pub name: String,
    pub host: String,
    pub port: String,
    pub username: String,
    pub auth_type: AuthType,
    pub password: String,
    pub key_path: String,
    pub save_session: bool,
}

#[derive(Default, PartialEq)]
pub enum AuthType {
    #[default]
    Password,
    Key,
}

/// Authentication method for UI selection in the New Session dialog
#[derive(Debug, Clone, PartialEq, Default)]
pub enum AuthMethodUi {
    #[default]
    Password,
    KeyFile,
    Agent,
}