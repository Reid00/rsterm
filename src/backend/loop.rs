// src/backend/loop.rs
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc as tokio_mpsc;
use tracing::{error, info};

use crate::backend::{AppCommand, BackendEvent, LocalPty, PtyBackend, SessionHandle, SshPty};
use crate::config::SessionConfig;
use crate::platform;

/// Channel capacity for PTY output
const PTY_CHANNEL_CAPACITY: usize = 256;

/// Run the backend event loop
pub async fn backend_loop(
    mut command_rx: tokio_mpsc::Receiver<AppCommand>,
    event_tx: tokio_mpsc::Sender<BackendEvent>,
) {
    let mut sessions: HashMap<String, SessionHandle> = HashMap::new();

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
                            if let Err(e) = session.backend.write(&data).await {
                                error!("Failed to write to session {}: {}", session_id, e);
                            }
                        }
                    }
                    AppCommand::ResizeSession { session_id, cols, rows } => {
                        if let Some(session) = sessions.get(&session_id) {
                            if let Err(e) = session.backend.resize(cols, rows).await {
                                error!("Failed to resize session {}: {}", session_id, e);
                            }
                        }
                    }
                    AppCommand::CloseSession { session_id } => {
                        if let Some(session) = sessions.remove(&session_id) {
                            session.backend.kill();
                            info!("Closed session: {}", session_id);
                        }
                    }
                    AppCommand::Shutdown => {
                        // Kill all sessions
                        for session in sessions.values() {
                            session.backend.kill();
                        }
                        info!("Backend shutting down");
                        break;
                    }
                }
            }

            else => {
                // Channel closed, clean up all sessions
                for session in sessions.values() {
                    session.backend.kill();
                }
                info!("Backend shutting down (channel closed)");
                break;
            }
        }
    }

    info!("Backend loop ended");
}

async fn create_local_session(
    sessions: &mut HashMap<String, SessionHandle>,
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

    sessions.insert(session_id.clone(), handle);

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
    sessions: &mut HashMap<String, SessionHandle>,
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

    sessions.insert(session_id.clone(), handle);

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