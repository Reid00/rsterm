// src/state.rs
//! Application state management

use std::path::PathBuf;
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
    // pub sftp: Option<SftpState>,  // Added in M6
}

pub struct SftpState {
    pub local_path: PathBuf,
    pub remote_path: String,
    pub loading: bool,
    pub error: Option<String>,
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