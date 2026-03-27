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
use tracing::{error, info, warn};

use crate::backend::PtyBackend;
use crate::error::{PtyError, SshError};
use crate::sftp::SftpClient;
use crate::ssh::{HostKeyVerification, KnownHosts};

/// SSH connection configuration for creating a session
#[derive(Debug)]
pub struct SshConnectionConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuthMethod,
    /// Optional known hosts database for host key verification.
    /// If None, host key verification is skipped (insecure).
    pub known_hosts: Option<KnownHosts>,
}

/// SSH authentication method
#[derive(Debug)]
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
    /// SFTP client (created during session setup, cached here)
    sftp: Arc<tokio::sync::Mutex<Option<Arc<tokio::sync::Mutex<SftpClient>>>>>,
}

/// Client handler for russh
struct Client {
    /// Host being connected to
    host: String,
    /// Port being connected to
    port: u16,
    /// Known hosts database for verification
    known_hosts: Option<KnownHosts>,
    /// Result of host key verification (set during check_server_key)
    verification_result: Option<HostKeyVerification>,
}

impl Client {
    /// Create a new client handler
    fn new(host: String, port: u16, known_hosts: Option<KnownHosts>) -> Self {
        Self {
            host,
            port,
            known_hosts,
            verification_result: None,
        }
    }
}

#[async_trait::async_trait]
impl Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh_keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        match &self.known_hosts {
            Some(known_hosts) => {
                let result = known_hosts.verify(&self.host, self.port, server_public_key);

                match &result {
                    HostKeyVerification::Trusted => {
                        info!("Host key verified for {}:{}", self.host, self.port);
                        self.verification_result = Some(result);
                        Ok(true)
                    }
                    HostKeyVerification::Unknown => {
                        // TOFU: Trust On First Use - accept unknown hosts
                        info!(
                            "Unknown host {}:{}, accepting key (TOFU)",
                            self.host, self.port
                        );
                        // Add to pending so it's available for this session
                        known_hosts.add_pending(&self.host, self.port, server_public_key.clone());
                        // Save to known_hosts file
                        if let Err(e) = known_hosts.add(&self.host, self.port, server_public_key) {
                            warn!("Failed to save known_hosts: {}", e);
                        }
                        self.verification_result = Some(result);
                        Ok(true)
                    }
                    HostKeyVerification::Changed { line } => {
                        // Key changed - potential MITM, reject
                        warn!(
                            "Host key changed for {}:{} at line {}, rejecting connection",
                            self.host, self.port, line
                        );
                        self.verification_result = Some(result);
                        Ok(false)
                    }
                }
            }
            None => {
                // No known_hosts configured - accept all keys (insecure)
                warn!(
                    "No known_hosts configured, accepting key for {}:{} (insecure)",
                    self.host, self.port
                );
                self.verification_result = Some(HostKeyVerification::Unknown);
                Ok(true)
            }
        }
    }
}

impl SshPty {
    /// Create a new SSH PTY connection (async version for use within tokio runtime)
    pub async fn new_async(
        config: &SshConnectionConfig,
        cols: u16,
        rows: u16,
        output_tx: tokio_mpsc::Sender<Vec<u8>>,
    ) -> Result<Self, SshError> {
        info!("Connecting to SSH server {}:{}", config.host, config.port);

        // Connect and set up PTY with timeout
        let (session, channel) = match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            connect_and_setup_pty(config, cols, rows)
        ).await {
            Ok(result) => result?,
            Err(_) => return Err(SshError::Protocol("Connection timeout (30s)".to_string())),
        };

        let alive = Arc::new(AtomicBool::new(true));
        let (command_tx, command_rx) = tokio_mpsc::channel::<SshCommand>(64);

        // Create SFTP client storage
        let sftp: Arc<tokio::sync::Mutex<Option<Arc<tokio::sync::Mutex<SftpClient>>>>> = Arc::new(tokio::sync::Mutex::new(None));

        // Spawn the actor task that owns the channel
        let alive_clone = alive.clone();
        let sftp_clone = sftp.clone();
        tokio::spawn(async move {
            // Create SFTP client
            match SftpClient::new(&session).await {
                Ok(sftp_client) => {
                    *sftp_clone.lock().await = Some(Arc::new(tokio::sync::Mutex::new(sftp_client)));
                    info!("SFTP client initialized");
                }
                Err(e) => {
                    warn!("Failed to create SFTP client: {}", e);
                }
            }
            ssh_actor(channel, session, command_rx, output_tx, alive_clone).await;
        });

        Ok(Self {
            alive,
            command_tx,
            sftp,
        })
    }

    /// Try to get an SFTP client from this SSH session (internal method)
    async fn try_get_sftp(&self) -> Option<Arc<tokio::sync::Mutex<SftpClient>>> {
        let guard = self.sftp.lock().await;
        guard.clone()
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

    // Create client handler with known hosts
    let client = Client::new(
        config.host.clone(),
        config.port,
        config.known_hosts.clone(),
    );

    // Connect to server and perform handshake
    let mut session = russh::client::connect(
        ssh_config,
        (config.host.as_str(), config.port),
        client,
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

    fn supports_sftp(&self) -> bool {
        true
    }

    async fn get_sftp(&self) -> Option<Arc<tokio::sync::Mutex<SftpClient>>> {
        self.try_get_sftp().await
    }
}

impl Drop for SshPty {
    fn drop(&mut self) {
        self.kill();
    }
}