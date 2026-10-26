use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use russh::client::{Config, DisconnectReason, Handle, Handler, Msg};
use russh::{Channel, ChannelMsg, Disconnect};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::backend::BackendEvent;
use crate::error::SshError;
use crate::sftp::SftpClient;
use crate::ssh::{HostKeyVerification, KnownHosts};

#[derive(Clone, Debug)]
pub struct SshConnectionConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: SshAuthMethod,
    pub known_hosts: KnownHosts,
    pub timeout: Duration,
    pub keepalive_interval: Duration,
    pub keepalive_max: u32,
}

#[derive(Clone)]
pub enum SshAuthMethod {
    Password(String),
    Key {
        private_key: Vec<u8>,
        passphrase: Option<String>,
    },
    Agent,
}

impl fmt::Debug for SshAuthMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Password(_) => "Password([redacted])",
            Self::Key { .. } => "Key([redacted])",
            Self::Agent => "Agent",
        })
    }
}

struct Client {
    session_id: String,
    host: String,
    port: u16,
    known_hosts: KnownHosts,
    events: mpsc::Sender<BackendEvent>,
    cancel: CancellationToken,
    disconnected: watch::Sender<Option<String>>,
    failure: Arc<parking_lot::Mutex<Option<SshError>>>,
}

#[async_trait::async_trait]
impl Handler for Client {
    type Error = SshError;

    async fn check_server_key(
        &mut self,
        key: &russh_keys::key::PublicKey,
    ) -> Result<bool, SshError> {
        match self.verify_server_key(key).await {
            Ok(trusted) => Ok(trusted),
            Err(error) => {
                let message = error.to_string();
                *self.failure.lock() = Some(error);
                Err(SshError::HostKeyVerification(message))
            }
        }
    }

    async fn disconnected(
        &mut self,
        reason: DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        let message = match reason {
            DisconnectReason::ReceivedDisconnect(info) => format!("Server disconnected: {info:?}"),
            DisconnectReason::Error(error) => error.to_string(),
        };
        let _ = self.disconnected.send(Some(message));
        Ok(())
    }
}

impl Client {
    async fn verify_server_key(&self, key: &russh_keys::key::PublicKey) -> Result<bool, SshError> {
        let known = self.known_hosts.clone();
        let host = self.host.clone();
        let port = self.port;
        let public_key = key.clone();
        let verification =
            tokio::task::spawn_blocking(move || known.verify(&host, port, &public_key))
                .await
                .map_err(|error| SshError::HostKeyVerification(error.to_string()))?
                .map_err(|error| SshError::HostKeyVerification(error.to_string()))?;
        match verification {
            HostKeyVerification::Trusted => Ok(true),
            HostKeyVerification::Changed { .. } => Err(SshError::HostKeyChanged),
            HostKeyVerification::Unknown => {
                let (respond_to, response) = oneshot::channel();
                let prompt = BackendEvent::HostKeyPrompt {
                    session_id: self.session_id.clone(),
                    host: self.host.clone(),
                    port: self.port,
                    algorithm: key.name().to_string(),
                    fingerprint: format!("SHA256:{}", key.fingerprint()),
                    respond_to,
                };
                tokio::select! {
                    biased;
                    _ = self.cancel.cancelled() => return Err(SshError::Cancelled),
                    result = self.events.send(prompt) => result.map_err(|_| SshError::HostKeyRejected)?,
                }
                let accepted = tokio::select! {
                    biased;
                    _ = self.cancel.cancelled() => return Err(SshError::Cancelled),
                    result = response => result.unwrap_or(false),
                };
                if !accepted {
                    return Err(SshError::HostKeyRejected);
                }
                let known = self.known_hosts.clone();
                let host = self.host.clone();
                let public_key = key.clone();
                tokio::task::spawn_blocking(move || known.add(&host, port, &public_key))
                    .await
                    .map_err(|error| SshError::HostKeyVerification(error.to_string()))?
                    .map_err(|error| SshError::HostKeyVerification(error.to_string()))?;
                Ok(true)
            }
        }
    }
}

pub struct SshTransport {
    session: Arc<Handle<Client>>,
    disconnected: watch::Receiver<Option<String>>,
    timeout: Duration,
    cancel: CancellationToken,
}

impl SshTransport {
    pub async fn connect(
        config: &SshConnectionConfig,
        session_id: String,
        events: mpsc::Sender<BackendEvent>,
        cancel: CancellationToken,
    ) -> Result<Self, SshError> {
        if config.host.is_empty()
            || config.host.chars().any(char::is_whitespace)
            || config.host.contains([',', '\0', '*', '?', '!', '|'])
        {
            return Err(SshError::Protocol("Invalid SSH host name".into()));
        }
        if config.port == 0
            || config.username.is_empty()
            || config.username.contains('\0')
            || config.timeout.is_zero()
            || config.keepalive_interval.is_zero()
            || config.keepalive_max == 0
        {
            return Err(SshError::Protocol("Invalid SSH connection settings".into()));
        }
        let (disconnected_tx, disconnected) = watch::channel(None);
        let failure = Arc::new(parking_lot::Mutex::new(None));
        let transport_cancel = cancel.child_token();
        let disconnect_error = disconnected.clone();
        let client = Client {
            session_id,
            host: config.host.clone(),
            port: config.port,
            known_hosts: config.known_hosts.clone(),
            events,
            cancel: transport_cancel.clone(),
            disconnected: disconnected_tx,
            failure: failure.clone(),
        };
        let ssh_config = Arc::new(Config {
            keepalive_interval: Some(config.keepalive_interval),
            keepalive_max: config.keepalive_max as usize,
            ..Config::default()
        });
        let connection = async {
            let socket =
                tokio::net::TcpStream::connect((config.host.as_str(), config.port)).await?;
            let stream = CancellableStream {
                stream: socket,
                cancelled: Box::pin(transport_cancel.clone().cancelled_owned()),
            };
            let mut session = russh::client::connect_stream(ssh_config, stream, client).await?;
            let authentication = tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(SshError::Cancelled),
                result = authenticate(&mut session, config) => result,
            };
            if let Err(error) = authentication {
                let _ = session
                    .disconnect(Disconnect::ByApplication, "Authentication ended", "en")
                    .await;
                return Err(error);
            }
            Ok(Self {
                session: Arc::new(session),
                disconnected,
                timeout: config.timeout,
                cancel: transport_cancel.clone(),
            })
        };
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(SshError::Cancelled),
            result = tokio::time::timeout(config.timeout, connection) => result.unwrap_or_else(|_| Err(SshError::ConnectionTimeout {
                host: config.host.clone(), port: config.port,
            })),
        };
        if result.is_err() {
            transport_cancel.cancel();
            if let Some(error) = failure.lock().take() {
                return Err(error);
            }
            if matches!(result, Err(SshError::Russh(russh::Error::Disconnect))) {
                if let Some(error) = disconnect_error.borrow().clone() {
                    return Err(SshError::Protocol(error));
                }
            }
        }
        result
    }

    pub async fn open_shell(&self, cols: u16, rows: u16) -> Result<Channel<Msg>, SshError> {
        tokio::time::timeout(self.timeout, async {
            let mut channel = self.session.channel_open_session().await?;
            channel
                .request_pty(true, "xterm-256color", cols.into(), rows.into(), 0, 0, &[])
                .await?;
            wait_request_reply(&mut channel).await?;
            channel.request_shell(true).await?;
            wait_request_reply(&mut channel).await?;
            Ok(channel)
        })
        .await
        .map_err(|_| SshError::Protocol("Shell setup timed out".into()))?
    }

    pub async fn sftp(&self) -> Result<SftpClient, crate::error::SftpError> {
        SftpClient::new(self.session.as_ref(), self.timeout).await
    }

    pub(crate) async fn open_inspection_channel(&self) -> Result<Channel<Msg>, russh::Error> {
        self.session.channel_open_session().await
    }

    pub(crate) fn stop_stream(&self) {
        self.cancel.cancel();
    }

    pub fn disconnection(&self) -> watch::Receiver<Option<String>> {
        self.disconnected.clone()
    }

    pub async fn disconnect(&self) {
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            self.session
                .disconnect(Disconnect::ByApplication, "Session closed", "en"),
        )
        .await;
        self.cancel.cancel();
    }
}

impl Drop for SshTransport {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

struct CancellableStream {
    stream: tokio::net::TcpStream,
    cancelled: Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl AsyncRead for CancellableStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.cancelled.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(std::io::ErrorKind::ConnectionAborted.into()));
        }
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for CancellableStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.cancelled.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(std::io::ErrorKind::ConnectionAborted.into()));
        }
        Pin::new(&mut self.stream).poll_write(cx, data)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if self.cancelled.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(std::io::ErrorKind::ConnectionAborted.into()));
        }
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

pub(crate) async fn wait_request_reply(channel: &mut Channel<Msg>) -> Result<(), SshError> {
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Success) => return Ok(()),
            Some(ChannelMsg::Failure) => {
                return Err(SshError::Protocol("Server rejected channel request".into()))
            }
            Some(ChannelMsg::Close | ChannelMsg::Eof) | None => {
                return Err(SshError::ChannelClosed)
            }
            _ => {}
        }
    }
}

async fn authenticate(
    session: &mut Handle<Client>,
    config: &SshConnectionConfig,
) -> Result<(), SshError> {
    let authenticated = match &config.auth {
        SshAuthMethod::Password(password) => {
            session
                .authenticate_password(&config.username, password)
                .await?
        }
        SshAuthMethod::Key {
            private_key,
            passphrase,
        } => {
            let private_key = private_key.clone();
            let passphrase = passphrase.clone();
            let key = tokio::task::spawn_blocking(move || {
                let text = std::str::from_utf8(&private_key).map_err(|_| SshError::InvalidKey)?;
                russh_keys::decode_secret_key(text, passphrase.as_deref())
                    .map_err(|_| SshError::InvalidKey)
            })
            .await
            .map_err(|_| SshError::InvalidKey)??;
            session
                .authenticate_publickey(&config.username, Arc::new(key))
                .await?
        }
        SshAuthMethod::Agent => {
            let mut agent = russh_keys::agent::client::AgentClient::connect_env()
                .await
                .map_err(|error| SshError::AuthFailed {
                    reason: error.to_string(),
                })?;
            let identities =
                agent
                    .request_identities()
                    .await
                    .map_err(|error| SshError::AuthFailed {
                        reason: error.to_string(),
                    })?;
            let mut success = false;
            for identity in identities {
                let (returned_agent, result) = session
                    .authenticate_future(&config.username, identity, agent)
                    .await;
                agent = returned_agent;
                if result.map_err(|error| SshError::AuthFailed {
                    reason: error.to_string(),
                })? {
                    success = true;
                    break;
                }
            }
            success
        }
    };
    if authenticated {
        Ok(())
    } else {
        Err(SshError::AuthFailed {
            reason: "Authentication rejected".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted() {
        assert!(!format!("{:?}", SshAuthMethod::Password("secret".into())).contains("secret"));
        assert!(!format!(
            "{:?}",
            SshAuthMethod::Key {
                private_key: b"secret".to_vec(),
                passphrase: Some("secret".into())
            }
        )
        .contains("secret"));
    }

    #[tokio::test]
    async fn unknown_host_requires_acceptance_and_rejection_is_not_saved() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let known_hosts = KnownHosts::load_from(file.path().to_path_buf()).unwrap();
        let (events, mut rx) = mpsc::channel(1);
        let (disconnected, _) = watch::channel(None);
        let client = Client {
            session_id: "s".into(),
            host: "host".into(),
            port: 22,
            known_hosts: known_hosts.clone(),
            events,
            cancel: CancellationToken::new(),
            disconnected,
            failure: Arc::new(parking_lot::Mutex::new(None)),
        };
        let key = russh_keys::key::KeyPair::generate_ed25519()
            .unwrap()
            .clone_public_key()
            .unwrap();
        let checking = tokio::spawn(async move { client.verify_server_key(&key).await });
        match rx.recv().await.unwrap() {
            BackendEvent::HostKeyPrompt { respond_to, .. } => respond_to.send(false).unwrap(),
            _ => panic!("Expected host prompt"),
        }
        assert!(matches!(
            checking.await.unwrap(),
            Err(SshError::HostKeyRejected)
        ));
        assert!(std::fs::read_to_string(file.path()).unwrap().is_empty());
    }

    #[tokio::test]
    async fn mismatch_and_database_errors_never_offer_acceptance() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let known_hosts = KnownHosts::load_from(file.path().to_path_buf()).unwrap();
        let first = russh_keys::key::KeyPair::generate_ed25519()
            .unwrap()
            .clone_public_key()
            .unwrap();
        let other = russh_keys::key::KeyPair::generate_ed25519()
            .unwrap()
            .clone_public_key()
            .unwrap();
        known_hosts.add("host", 22, &first).unwrap();
        let (events, mut rx) = mpsc::channel(2);
        let (disconnected, _) = watch::channel(None);
        let client = Client {
            session_id: "s".into(),
            host: "host".into(),
            port: 22,
            known_hosts,
            events,
            cancel: CancellationToken::new(),
            disconnected,
            failure: Arc::new(parking_lot::Mutex::new(None)),
        };
        assert!(matches!(
            client.verify_server_key(&other).await,
            Err(SshError::HostKeyChanged)
        ));
        std::fs::write(file.path(), "malformed").unwrap();
        assert!(matches!(
            client.verify_server_key(&first).await,
            Err(SshError::HostKeyVerification(_))
        ));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn cancellation_unblocks_host_prompt() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let (events, mut rx) = mpsc::channel(1);
        let (disconnected, _) = watch::channel(None);
        let cancel = CancellationToken::new();
        let client = Client {
            session_id: "s".into(),
            host: "host".into(),
            port: 22,
            known_hosts: KnownHosts::load_from(file.path().to_path_buf()).unwrap(),
            events,
            cancel: cancel.clone(),
            disconnected,
            failure: Arc::new(parking_lot::Mutex::new(None)),
        };
        let key = russh_keys::key::KeyPair::generate_ed25519()
            .unwrap()
            .clone_public_key()
            .unwrap();
        let checking = tokio::spawn(async move { client.verify_server_key(&key).await });
        let _prompt = rx.recv().await.unwrap();
        cancel.cancel();
        assert!(matches!(checking.await.unwrap(), Err(SshError::Cancelled)));
    }
}
