// src/backend/loop.rs
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc as tokio_mpsc;
use tracing::{error, info, warn};

use crate::backend::{AppCommand, BackendEvent, LocalPty, PtyBackend, SessionHandle, SshPty};
use crate::config::SessionConfig;
use crate::platform;
use crate::sftp::SftpClient;
use crate::state::FileEntryInfo;

/// Channel capacity for PTY output
const PTY_CHANNEL_CAPACITY: usize = 256;

/// Extended session handle that includes optional SFTP client
struct SessionState {
    handle: SessionHandle,
    /// SFTP client for SSH sessions (None for local sessions or if not yet initialized)
    sftp: Option<Arc<tokio::sync::Mutex<SftpClient>>>,
}

/// Run the backend event loop
pub async fn backend_loop(
    mut command_rx: tokio_mpsc::Receiver<AppCommand>,
    event_tx: tokio_mpsc::Sender<BackendEvent>,
) {
    let mut sessions: HashMap<String, SessionState> = HashMap::new();

    info!("Backend loop started");

    loop {
        tokio::select! {
            // Handle commands from UI
            Some(cmd) = command_rx.recv() => {
                match cmd {
                    AppCommand::CreateSession { session_id, config, respond_to } => {
                        let result = create_local_session(&mut sessions, &event_tx, session_id, config).await;
                        let _ = respond_to.send(result.map(|id| id).map_err(|e| e.to_string())).await;
                    }
                    AppCommand::CreateSshSession { session_id, ssh_config, respond_to } => {
                        let result = create_ssh_session(&mut sessions, &event_tx, session_id, ssh_config).await;
                        let _ = respond_to.send(result.map(|id| id).map_err(|e| e.to_string())).await;
                    }
                    AppCommand::WriteInput { session_id, data } => {
                        if let Some(session) = sessions.get(&session_id) {
                            if let Err(e) = session.handle.backend.write(&data).await {
                                error!("Failed to write to session {}: {}", session_id, e);
                            }
                        }
                    }
                    AppCommand::ResizeSession { session_id, cols, rows } => {
                        if let Some(session) = sessions.get(&session_id) {
                            if let Err(e) = session.handle.backend.resize(cols, rows).await {
                                error!("Failed to resize session {}: {}", session_id, e);
                            }
                        }
                    }
                    AppCommand::CloseSession { session_id } => {
                        if let Some(session) = sessions.remove(&session_id) {
                            session.handle.backend.kill();
                            info!("Closed session: {}", session_id);
                        }
                    }
                    AppCommand::SftpInit { session_id } => {
                        handle_sftp_init(&mut sessions, &event_tx, session_id).await;
                    }
                    AppCommand::SftpListDir { session_id, path } => {
                        handle_sftp_list_dir(&mut sessions, &event_tx, session_id, path).await;
                    }
                    AppCommand::SftpDownload { session_id, remote_path, local_path } => {
                        handle_sftp_download(&mut sessions, &event_tx, session_id, remote_path, local_path).await;
                    }
                    AppCommand::SftpUpload { session_id, local_path, remote_path } => {
                        handle_sftp_upload(&mut sessions, &event_tx, session_id, local_path, remote_path).await;
                    }
                    AppCommand::SftpMkdir { session_id, path } => {
                        handle_sftp_mkdir(&mut sessions, &event_tx, session_id, path).await;
                    }
                    AppCommand::SftpDelete { session_id, path } => {
                        handle_sftp_delete(&mut sessions, &event_tx, session_id, path).await;
                    }
                    AppCommand::Shutdown => {
                        // Kill all sessions
                        for session in sessions.values() {
                            session.handle.backend.kill();
                        }
                        info!("Backend shutting down");
                        break;
                    }
                }
            }

            else => {
                // Channel closed, clean up all sessions
                for session in sessions.values() {
                    session.handle.backend.kill();
                }
                info!("Backend shutting down (channel closed)");
                break;
            }
        }
    }

    info!("Backend loop ended");
}

async fn create_local_session(
    sessions: &mut HashMap<String, SessionState>,
    event_tx: &tokio_mpsc::Sender<BackendEvent>,
    session_id: String,
    config: SessionConfig,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {

    // Create output channel for this session
    let (output_tx, mut output_rx) = tokio_mpsc::channel(PTY_CHANNEL_CAPACITY);

    // Create the PTY backend
    let default_shell = platform::default_shell();
    let shell = config.shell.as_deref().unwrap_or(&default_shell);
    let backend = Arc::new(LocalPty::new(shell, 80, 24, output_tx)?);

    let handle = SessionHandle {
        id: session_id.clone(),
        config,
        backend: backend.clone(),
    };

    let state = SessionState {
        handle,
        sftp: None, // No SFTP for local sessions
    };

    sessions.insert(session_id.clone(), state);

    // Notify UI
    let _ = event_tx.send(BackendEvent::SessionCreated {
        session_id: session_id.clone(),
    }).await;

    // Spawn task to forward output to event channel
    let event_tx_clone = event_tx.clone();
    let session_id_clone = session_id.clone();
    let backend_clone = backend.clone();
    tokio::spawn(async move {
        while let Some(data) = output_rx.recv().await {
            if event_tx_clone.send(BackendEvent::SessionOutput {
                session_id: session_id_clone.clone(),
                data,
            }).await.is_err() {
                break;
            }
        }
        // PTY closed
        if !backend_clone.is_alive() {
            let _ = event_tx_clone.send(BackendEvent::SessionClosed {
                session_id: session_id_clone,
            }).await;
        }
    });

    Ok(session_id)
}

async fn create_ssh_session(
    sessions: &mut HashMap<String, SessionState>,
    event_tx: &tokio_mpsc::Sender<BackendEvent>,
    session_id: String,
    ssh_config: crate::backend::SshConnectionConfig,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {

    // Create output channel for this session
    let (output_tx, mut output_rx) = tokio_mpsc::channel(PTY_CHANNEL_CAPACITY);

    // Create the SSH PTY backend
    let backend = Arc::new(SshPty::new(&ssh_config, 80, 24, output_tx)?);

    // Create a SessionConfig for the handle (with SSH info)
    let config = SessionConfig {
        id: session_id.clone(),
        name: ssh_config.host.clone(),
        host: ssh_config.host.clone(),
        port: ssh_config.port,
        username: ssh_config.username.clone(),
        ..Default::default()
    };

    let handle = SessionHandle {
        id: session_id.clone(),
        config,
        backend: backend.clone(),
    };

    let state = SessionState {
        handle,
        sftp: None, // Will be initialized on demand
    };

    sessions.insert(session_id.clone(), state);

    // Notify UI
    let _ = event_tx.send(BackendEvent::SessionCreated {
        session_id: session_id.clone(),
    }).await;

    // Spawn task to forward output to event channel
    let event_tx_clone = event_tx.clone();
    let session_id_clone = session_id.clone();
    let backend_clone = backend.clone();
    tokio::spawn(async move {
        while let Some(data) = output_rx.recv().await {
            if event_tx_clone.send(BackendEvent::SessionOutput {
                session_id: session_id_clone.clone(),
                data,
            }).await.is_err() {
                break;
            }
        }
        // PTY closed
        if !backend_clone.is_alive() {
            let _ = event_tx_clone.send(BackendEvent::SessionClosed {
                session_id: session_id_clone,
            }).await;
        }
    });

    Ok(session_id)
}

/// Initialize SFTP for an SSH session
async fn handle_sftp_init(
    sessions: &mut HashMap<String, SessionState>,
    event_tx: &tokio_mpsc::Sender<BackendEvent>,
    session_id: String,
) {
    // First, check if session exists and get the backend
    let backend = {
        let session = match sessions.get(&session_id) {
            Some(s) => s,
            None => {
                warn!("SFTP init requested for unknown session: {}", session_id);
                return;
            }
        };

        // Check if SFTP is already initialized
        if session.sftp.is_some() {
            let _ = event_tx.send(BackendEvent::SftpReady { session_id }).await;
            return;
        }

        // Check if backend supports SFTP
        if !session.handle.backend.supports_sftp() {
            let _ = event_tx.send(BackendEvent::SftpError {
                session_id,
                error: "SFTP is not supported for this session type".to_string(),
            }).await;
            return;
        }

        session.handle.backend.clone()
    };

    // Get SFTP client from backend
    match backend.get_sftp().await {
        Some(sftp_client) => {
            // Store the SFTP client in the session
            if let Some(session) = sessions.get_mut(&session_id) {
                session.sftp = Some(sftp_client);
            }
            let _ = event_tx.send(BackendEvent::SftpReady { session_id }).await;
        }
        None => {
            let _ = event_tx.send(BackendEvent::SftpError {
                session_id,
                error: "SFTP client not yet initialized or unavailable".to_string(),
            }).await;
        }
    }
}

/// Handle SFTP directory listing
async fn handle_sftp_list_dir(
    sessions: &mut HashMap<String, SessionState>,
    event_tx: &tokio_mpsc::Sender<BackendEvent>,
    session_id: String,
    path: String,
) {
    let session = match sessions.get_mut(&session_id) {
        Some(s) => s,
        None => {
            warn!("SFTP list dir requested for unknown session: {}", session_id);
            return;
        }
    };

    let sftp = match &session.sftp {
        Some(s) => s.clone(),
        None => {
            let _ = event_tx.send(BackendEvent::SftpError {
                session_id,
                error: "SFTP not initialized".to_string(),
            }).await;
            return;
        }
    };

    let event_tx_clone = event_tx.clone();
    let session_id_clone = session_id.clone();
    let path_clone = path.clone();

    tokio::spawn(async move {
        let mut sftp_guard = sftp.lock().await;
        match sftp_guard.list_dir(&path_clone).await {
            Ok(entries) => {
                let file_infos: Vec<FileEntryInfo> = entries
                    .into_iter()
                    .map(|e| FileEntryInfo {
                        name: e.name,
                        path: e.path,
                        is_dir: e.is_dir,
                        size: e.size,
                        modified: e.modified.map(|t| {
                            let datetime: chrono::DateTime<chrono::Utc> = t.into();
                            datetime.format("%Y-%m-%d %H:%M").to_string()
                        }),
                    })
                    .collect();

                let _ = event_tx_clone.send(BackendEvent::SftpListResult {
                    session_id: session_id_clone,
                    path: path_clone,
                    entries: file_infos,
                }).await;
            }
            Err(e) => {
                let _ = event_tx_clone.send(BackendEvent::SftpError {
                    session_id: session_id_clone,
                    error: e.to_string(),
                }).await;
            }
        }
    });
}

/// Handle SFTP download
async fn handle_sftp_download(
    sessions: &mut HashMap<String, SessionState>,
    event_tx: &tokio_mpsc::Sender<BackendEvent>,
    session_id: String,
    remote_path: String,
    local_path: String,
) {
    let session = match sessions.get_mut(&session_id) {
        Some(s) => s,
        None => {
            warn!("SFTP download requested for unknown session: {}", session_id);
            return;
        }
    };

    let sftp = match &session.sftp {
        Some(s) => s.clone(),
        None => {
            let _ = event_tx.send(BackendEvent::SftpError {
                session_id,
                error: "SFTP not initialized".to_string(),
            }).await;
            return;
        }
    };

    let event_tx_clone = event_tx.clone();
    let session_id_clone = session_id.clone();

    tokio::spawn(async move {
        let mut sftp_guard = sftp.lock().await;
        match sftp_guard.download(&remote_path, &local_path).await {
            Ok(()) => {
                info!("Download complete: {} -> {}", remote_path, local_path);
                // For now, we don't track individual transfers with IDs
                // The UI can refresh the directory after download
            }
            Err(e) => {
                let _ = event_tx_clone.send(BackendEvent::SftpError {
                    session_id: session_id_clone,
                    error: format!("Download failed: {}", e),
                }).await;
            }
        }
    });
}

/// Handle SFTP upload
async fn handle_sftp_upload(
    sessions: &mut HashMap<String, SessionState>,
    event_tx: &tokio_mpsc::Sender<BackendEvent>,
    session_id: String,
    local_path: String,
    remote_path: String,
) {
    let session = match sessions.get_mut(&session_id) {
        Some(s) => s,
        None => {
            warn!("SFTP upload requested for unknown session: {}", session_id);
            return;
        }
    };

    let sftp = match &session.sftp {
        Some(s) => s.clone(),
        None => {
            let _ = event_tx.send(BackendEvent::SftpError {
                session_id,
                error: "SFTP not initialized".to_string(),
            }).await;
            return;
        }
    };

    let event_tx_clone = event_tx.clone();
    let session_id_clone = session_id.clone();

    tokio::spawn(async move {
        let mut sftp_guard = sftp.lock().await;
        match sftp_guard.upload(&local_path, &remote_path).await {
            Ok(()) => {
                info!("Upload complete: {} -> {}", local_path, remote_path);
            }
            Err(e) => {
                let _ = event_tx_clone.send(BackendEvent::SftpError {
                    session_id: session_id_clone,
                    error: format!("Upload failed: {}", e),
                }).await;
            }
        }
    });
}

/// Handle SFTP mkdir
async fn handle_sftp_mkdir(
    sessions: &mut HashMap<String, SessionState>,
    event_tx: &tokio_mpsc::Sender<BackendEvent>,
    session_id: String,
    path: String,
) {
    let session = match sessions.get_mut(&session_id) {
        Some(s) => s,
        None => {
            warn!("SFTP mkdir requested for unknown session: {}", session_id);
            return;
        }
    };

    let sftp = match &session.sftp {
        Some(s) => s.clone(),
        None => {
            let _ = event_tx.send(BackendEvent::SftpError {
                session_id,
                error: "SFTP not initialized".to_string(),
            }).await;
            return;
        }
    };

    let event_tx_clone = event_tx.clone();
    let session_id_clone = session_id.clone();

    tokio::spawn(async move {
        let mut sftp_guard = sftp.lock().await;
        match sftp_guard.mkdir(&path).await {
            Ok(()) => {
                info!("Directory created: {}", path);
            }
            Err(e) => {
                let _ = event_tx_clone.send(BackendEvent::SftpError {
                    session_id: session_id_clone,
                    error: format!("Failed to create directory: {}", e),
                }).await;
            }
        }
    });
}

/// Handle SFTP delete
async fn handle_sftp_delete(
    sessions: &mut HashMap<String, SessionState>,
    event_tx: &tokio_mpsc::Sender<BackendEvent>,
    session_id: String,
    path: String,
) {
    let session = match sessions.get_mut(&session_id) {
        Some(s) => s,
        None => {
            warn!("SFTP delete requested for unknown session: {}", session_id);
            return;
        }
    };

    let sftp = match &session.sftp {
        Some(s) => s.clone(),
        None => {
            let _ = event_tx.send(BackendEvent::SftpError {
                session_id,
                error: "SFTP not initialized".to_string(),
            }).await;
            return;
        }
    };

    let event_tx_clone = event_tx.clone();
    let session_id_clone = session_id.clone();

    tokio::spawn(async move {
        let mut sftp_guard = sftp.lock().await;
        match sftp_guard.delete(&path).await {
            Ok(()) => {
                info!("Deleted: {}", path);
            }
            Err(e) => {
                let _ = event_tx_clone.send(BackendEvent::SftpError {
                    session_id: session_id_clone,
                    error: format!("Failed to delete: {}", e),
                }).await;
            }
        }
    });
}