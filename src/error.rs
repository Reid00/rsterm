// src/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Ssh(#[from] SshError),
    #[error(transparent)]
    Terminal(#[from] TerminalError),
    #[error(transparent)]
    Sftp(#[from] SftpError),
    #[error(transparent)]
    Config(#[from] ConfigError),
}

#[derive(Debug, Error)]
pub enum SshError {
    #[error("Connection timeout: {host}:{port}")]
    ConnectionTimeout { host: String, port: u16 },
    #[error("Connection refused: {host}:{port}")]
    ConnectionRefused { host: String, port: u16 },
    #[error("DNS resolution failed: {host}")]
    DnsResolution { host: String },
    #[error("Authentication failed: {reason}")]
    AuthFailed { reason: String },
    #[error("Key file not found: {path}")]
    KeyFileNotFound { path: String },
    #[error("Invalid key or wrong passphrase")]
    InvalidKey,
    #[error("Host key changed")]
    HostKeyChanged,
    #[error("Network lost")]
    NetworkLost,
    #[error("SSH protocol error: {0}")]
    Protocol(String),
    #[error("Channel closed")]
    ChannelClosed,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("PTY creation failed: {0}")]
    PtyCreation(String),
    #[error("PTY process exited (code: {code:?})")]
    PtyExited { code: Option<i32> },
    #[error(transparent)]
    PtyWrite(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("PTY creation failed: {0}")]
    CreationFailed(String),
    #[error("PTY process exited")]
    ProcessExited,
    #[error(transparent)]
    WriteError(#[from] std::io::Error),
    #[error("PTY resize failed: {0}")]
    ResizeFailed(String),
}

#[derive(Debug, Error)]
pub enum SftpError {
    #[error("SFTP not connected")]
    NotConnected,
    #[error("Permission denied: {path}")]
    PermissionDenied { path: String },
    #[error("File not found: {path}")]
    FileNotFound { path: String },
    #[error("Disk full")]
    DiskFull,
    #[error("Transfer interrupted: {reason}")]
    TransferInterrupted { reason: String },
    #[error("SFTP protocol error: {0}")]
    Protocol(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("No platform config directory")]
    NoPlatformConfigDir,
    #[error("Config parse error at {path}: {reason}")]
    ParseError { path: String, reason: String },
    #[error("Theme not found: {0}")]
    ThemeNotFound(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
    #[error(transparent)]
    TomlSer(#[from] toml::ser::Error),
}