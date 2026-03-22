// src/backend/loop.rs
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc as tokio_mpsc;
use tracing::{error, info};

use crate::backend::{AppCommand, BackendEvent, LocalPty, PtyBackend, SessionHandle};
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
                    AppCommand::CreateSession { config, respond_to } => {
                        let result = create_session(&mut sessions, &event_tx, config).await;
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

            else => break,
        }
    }

    info!("Backend loop ended");
}

async fn create_session(
    sessions: &mut HashMap<String, SessionHandle>,
    event_tx: &tokio_mpsc::Sender<BackendEvent>,
    config: SessionConfig,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let session_id = uuid::Uuid::new_v4().to_string();

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