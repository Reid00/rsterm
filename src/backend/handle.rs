use std::path::PathBuf;
use std::time::SystemTime;

use tokio::sync::oneshot;

use crate::backend::SshConnectionConfig;
use crate::config::SessionConfig;
use crate::inspection::{InspectionData, InspectionError, InspectionKind};

pub type RequestId = u64;
pub type TransferId = u64;

#[derive(Clone, Debug)]
pub struct FileEntryInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub permissions: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferOutcome {
    Completed,
    Cancelled,
    Failed(String),
}

#[derive(Debug)]
pub enum AppCommand {
    CreateSession {
        session_id: String,
        config: SessionConfig,
    },
    CreateSshSession {
        session_id: String,
        ssh_config: SshConnectionConfig,
    },
    WriteInput {
        session_id: String,
        data: Vec<u8>,
    },
    ResizeSession {
        session_id: String,
        cols: u16,
        rows: u16,
    },
    CloseSession {
        session_id: String,
    },
    SftpInit {
        session_id: String,
    },
    SftpListDir {
        session_id: String,
        request_id: RequestId,
        path: String,
    },
    SftpMkdir {
        session_id: String,
        request_id: RequestId,
        path: String,
    },
    SftpDelete {
        session_id: String,
        request_id: RequestId,
        path: String,
    },
    SftpRename {
        session_id: String,
        request_id: RequestId,
        from: String,
        to: String,
    },
    SftpChmod {
        session_id: String,
        request_id: RequestId,
        path: String,
        mode: u32,
    },
    SftpUpload {
        session_id: String,
        transfer_id: TransferId,
        local_path: PathBuf,
        remote_path: String,
        overwrite: bool,
    },
    SftpDownload {
        session_id: String,
        transfer_id: TransferId,
        remote_path: String,
        local_path: PathBuf,
        overwrite: bool,
    },
    CancelTransfer {
        session_id: String,
        transfer_id: TransferId,
    },
    Inspect {
        session_id: String,
        request_id: RequestId,
        kind: InspectionKind,
    },
    CancelInspection {
        session_id: String,
        request_id: RequestId,
    },
    Shutdown,
}

#[derive(Debug)]
pub enum BackendEvent {
    SessionCreated {
        session_id: String,
    },
    SessionOutput {
        session_id: String,
        data: Vec<u8>,
    },
    SessionClosed {
        session_id: String,
        reason: Option<String>,
        retryable: bool,
    },
    SessionFailed {
        session_id: String,
        error: String,
        retryable: bool,
    },
    ShellReady {
        session_id: String,
    },
    InspectionResult {
        session_id: String,
        request_id: RequestId,
        result: Result<InspectionData, InspectionError>,
    },
    InspectionReady {
        session_id: String,
    },
    ShellUnavailable {
        session_id: String,
        error: String,
    },
    HostKeyPrompt {
        session_id: String,
        host: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
        respond_to: oneshot::Sender<bool>,
    },
    SftpReady {
        session_id: String,
        cwd: String,
    },
    SftpError {
        session_id: String,
        error: String,
    },
    SftpListResult {
        session_id: String,
        request_id: RequestId,
        path: String,
        result: Result<Vec<FileEntryInfo>, String>,
    },
    SftpOperationResult {
        session_id: String,
        request_id: RequestId,
        path: String,
        result: Result<(), String>,
    },
    SftpTransferProgress {
        session_id: String,
        transfer_id: TransferId,
        bytes_transferred: u64,
        total_bytes: u64,
    },
    SftpTransferComplete {
        session_id: String,
        transfer_id: TransferId,
        outcome: TransferOutcome,
    },
}
