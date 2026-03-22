// src/backend/ssh.rs
//! SSH PTY backend implementation using russh
//!
//! Uses an actor pattern where a single task owns the channel
//! and processes commands from a command channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use russh::client::{Config, Handle, Handler, Msg};
use russh::{Channel, ChannelMsg, Disconnect};
use tokio::sync::mpsc as tokio_mpsc;
use tracing::{error, info};

use crate::backend::PtyBackend;
use crate::error::{PtyError, SshError};

/// SSH connection configuration for creating a session
pub struct SshConnectionConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuthMethod,
}

/// SSH authentication method
pub enum SshAuthMethod {
    Password(String),
    Key {
        private_key: Vec<u8>,
        passphrase: Option<String>,
    },
    Agent,
}

/// Commands sent to the SSH actor task
enum SshCommand {
    Write(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Kill,
}

/// SSH PTY implementation using russh
pub struct SshPty {
    alive: Arc<AtomicBool>,
    command_tx: tokio_mpsc::Sender<SshCommand>,
    /// Runtime handle for async operations
    #[allow(dead_code)]
    runtime: Arc<tokio::runtime::Runtime>,
}

/// Client handler for russh
struct Client;

#[async_trait::async_trait]
impl Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh_keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // TODO: Implement proper known hosts verification
        // For now, accept all keys (should be configurable in production)
        Ok(true)
    }
}

impl SshPty {
    /// Create a new SSH PTY connection
    pub fn new(
        config: &SshConnectionConfig,
        cols: u16,
        rows: u16,
        output_tx: tokio_mpsc::Sender<Vec<u8>>,
    ) -> Result<Self, SshError> {
        info!("Connecting to SSH server {}:{}", config.host, config.port);

        // Create a dedicated runtime for this SSH session
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("ssh-backend")
            .build()
            .map_err(|e| SshError::Protocol(format!("Failed to create runtime: {}", e)))?;

        let rt = Arc::new(rt);

        // Connect and set up PTY
        let (session, channel) = rt.block_on(async {
            connect_and_setup_pty(config, cols, rows).await
        })?;

        let alive = Arc::new(AtomicBool::new(true));
        let (command_tx, command_rx) = tokio_mpsc::channel::<SshCommand>(64);

        // Spawn the actor task that owns the channel
        let alive_clone = alive.clone();
        rt.spawn(async move {
            ssh_actor(channel, session, command_rx, output_tx, alive_clone).await;
        });

        Ok(Self {
            alive,
            command_tx,
            runtime: rt,
        })
    }
}

/// Actor task that owns the SSH channel and processes commands
async fn ssh_actor(
    mut channel: Channel<Msg>,
    session: Handle<Client>,
    mut command_rx: tokio_mpsc::Receiver<SshCommand>,
    output_tx: tokio_mpsc::Sender<Vec<u8>>,
    alive: Arc<AtomicBool>,
) {
    loop {
        tokio::select! {
            // Handle commands from the application
            Some(cmd) = command_rx.recv() => {
                match cmd {
                    SshCommand::Write(data) => {
                        if let Err(e) = channel.data(&data[..]).await {
                            error!("Failed to write to SSH channel: {}", e);
                            alive.store(false, Ordering::Relaxed);
                            break;
                        }
                    }
                    SshCommand::Resize { cols, rows } => {
                        if let Err(e) = channel.window_change(cols as u32, rows as u32, 0, 0).await {
                            error!("Failed to resize SSH channel: {}", e);
                        }
                    }
                    SshCommand::Kill => {
                        info!("SSH kill command received");
                        let _ = channel.close().await;
                        let _ = session.disconnect(Disconnect::ByApplication, "Closing", "English").await;
                        alive.store(false, Ordering::Relaxed);
                        break;
                    }
                }
            }

            // Handle messages from the SSH channel
            Some(msg) = channel.wait() => {
                match msg {
                    ChannelMsg::Data { ref data } => {
                        if output_tx.send(data.to_vec()).await.is_err() {
                            info!("Output channel closed, stopping SSH actor");
                            alive.store(false, Ordering::Relaxed);
                            break;
                        }
                    }
                    ChannelMsg::ExtendedData { ref data, ext: _ } => {
                        // stderr data - also send to output
                        if output_tx.send(data.to_vec()).await.is_err() {
                            info!("Output channel closed, stopping SSH actor");
                            alive.store(false, Ordering::Relaxed);
                            break;
                        }
                    }
                    ChannelMsg::Eof => {
                        info!("SSH channel EOF received");
                        alive.store(false, Ordering::Relaxed);
                        break;
                    }
                    ChannelMsg::Close => {
                        info!("SSH channel closed");
                        alive.store(false, Ordering::Relaxed);
                        break;
                    }
                    ChannelMsg::ExitStatus { exit_status } => {
                        info!("SSH process exited with status: {}", exit_status);
                        // Don't break - there might be more data
                    }
                    ChannelMsg::ExitSignal {
                        signal_name,
                        core_dumped,
                        error_message,
                        ..
                    } => {
                        error!(
                            "SSH process killed by signal {:?}: {} (core: {})",
                            signal_name, error_message, core_dumped
                        );
                    }
                    _ => {
                        // Ignore other messages
                    }
                }
            }

            else => {
                // Both channels closed
                alive.store(false, Ordering::Relaxed);
                break;
            }
        }
    }

    info!("SSH actor task ended");
}

async fn connect_and_setup_pty(
    config: &SshConnectionConfig,
    cols: u16,
    rows: u16,
) -> Result<(Handle<Client>, Channel<Msg>), SshError> {
    // Configure SSH session
    let ssh_config = Arc::new(Config::default());

    // Connect to server and perform handshake
    let mut session = russh::client::connect(
        ssh_config,
        (config.host.as_str(), config.port),
        Client,
    )
    .await
    .map_err(|e| SshError::Protocol(format!("Connection failed: {}", e)))?;

    // Authenticate based on method
    match &config.auth {
        SshAuthMethod::Password(password) => {
            let success = session
                .authenticate_password(&config.username, password)
                .await
                .map_err(|e| SshError::AuthFailed { reason: e.to_string() })?;

            if !success {
                return Err(SshError::AuthFailed {
                    reason: "Password authentication rejected".to_string(),
                });
            }
        }
        SshAuthMethod::Key { private_key, passphrase } => {
            let key_str = std::str::from_utf8(private_key)
                .map_err(|_| SshError::InvalidKey)?;
            let key_pair = russh_keys::decode_secret_key(key_str, passphrase.as_deref())
                .map_err(|e| {
                    error!("Failed to decode key: {}", e);
                    SshError::InvalidKey
                })?;

            let success = session
                .authenticate_publickey(&config.username, Arc::new(key_pair))
                .await
                .map_err(|e| SshError::AuthFailed { reason: e.to_string() })?;

            if !success {
                return Err(SshError::AuthFailed {
                    reason: "Key authentication rejected".to_string(),
                });
            }
        }
        SshAuthMethod::Agent => {
            // Use SSH agent for authentication
            let mut agent = russh_keys::agent::client::AgentClient::connect_env()
                .await
                .map_err(|e| {
                    SshError::AuthFailed {
                        reason: format!(
                            "Failed to connect to SSH agent: {}. Is ssh-agent running?",
                            e
                        ),
                    }
                })?;

            // Get identities (public keys) from agent and try each one
            let identities = agent
                .request_identities()
                .await
                .map_err(|e| SshError::AuthFailed {
                    reason: format!("Failed to get identities from agent: {}", e),
                })?;

            let mut authenticated = false;
            for identity in identities {
                // authenticate_future takes (user, public_key, signer)
                // and returns (signer, result) - we need to get the agent back
                let (returned_agent, result) = session
                    .authenticate_future(&config.username, identity, agent)
                    .await;

                // Update agent for next iteration
                agent = returned_agent;

                match result {
                    Ok(success) if success => {
                        authenticated = true;
                        break;
                    }
                    Ok(_) => continue, // Try next key
                    Err(e) => {
                        error!("Agent auth error for key: {}", e);
                        continue;
                    }
                }
            }

            if !authenticated {
                return Err(SshError::AuthFailed {
                    reason: "Agent authentication failed - no valid keys".to_string(),
                });
            }
        }
    }

    info!("SSH authentication successful for user {}", config.username);

    // Open a channel and request PTY
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| SshError::Protocol(format!("Failed to open session: {}", e)))?;

    // Request PTY with terminal type
    channel
        .request_pty(true, "xterm-256color", cols as u32, rows as u32, 0, 0, &[])
        .await
        .map_err(|e| SshError::Protocol(format!("Failed to request PTY: {}", e)))?;

    // Request shell
    channel
        .exec(true, "bash")
        .await
        .map_err(|e| SshError::Protocol(format!("Failed to start shell: {}", e)))?;

    info!("SSH PTY session established ({}x{})", cols, rows);

    Ok((session, channel))
}

#[async_trait::async_trait]
impl PtyBackend for SshPty {
    async fn write(&self, data: &[u8]) -> Result<(), PtyError> {
        if !self.alive.load(Ordering::Relaxed) {
            return Err(PtyError::ProcessExited);
        }

        self.command_tx
            .send(SshCommand::Write(data.to_vec()))
            .await
            .map_err(|_| PtyError::ProcessExited)?;

        Ok(())
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        if !self.alive.load(Ordering::Relaxed) {
            return Err(PtyError::ResizeFailed("Channel closed".to_string()));
        }

        self.command_tx
            .send(SshCommand::Resize { cols, rows })
            .await
            .map_err(|_| PtyError::ResizeFailed("Channel closed".to_string()))?;

        Ok(())
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    fn kill(&self) {
        info!("Killing SSH session");

        // Send kill command (ignore errors if channel already closed)
        let _ = self.command_tx.try_send(SshCommand::Kill);

        self.alive.store(false, Ordering::Relaxed);
    }
}

impl Drop for SshPty {
    fn drop(&mut self) {
        self.kill();
    }
}