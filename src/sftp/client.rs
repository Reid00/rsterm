// src/sftp/client.rs
//! SFTP client implementation using russh-sftp

use std::path::PathBuf;

use russh::client::Handler;
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc as tokio_mpsc;
use tracing::{error, info, warn};

use crate::error::SftpError;

/// SFTP client for file transfers over SSH
pub struct SftpClient {
    session: SftpSession,
    current_dir: String,
}

/// File or directory entry
#[derive(Clone, Debug)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
}

/// Commands for SFTP operations
pub enum SftpCommand {
    ListDir {
        path: String,
        respond_to: tokio_mpsc::Sender<Result<Vec<FileEntry>, SftpError>>,
    },
    Download {
        remote_path: String,
        local_path: String,
        respond_to: tokio_mpsc::Sender<Result<(), SftpError>>,
    },
    Upload {
        local_path: String,
        remote_path: String,
        respond_to: tokio_mpsc::Sender<Result<(), SftpError>>,
    },
    ChangeDir {
        path: String,
        respond_to: tokio_mpsc::Sender<Result<String, SftpError>>,
    },
    MkDir {
        path: String,
        respond_to: tokio_mpsc::Sender<Result<(), SftpError>>,
    },
    Delete {
        path: String,
        respond_to: tokio_mpsc::Sender<Result<(), SftpError>>,
    },
}

impl SftpClient {
    /// Create a new SFTP client from an SSH session handle
    pub async fn new<H: Handler>(ssh_session: &russh::client::Handle<H>) -> Result<Self, SftpError> {
        info!("Opening SFTP channel");

        // Open a session channel for SFTP subsystem
        let channel = ssh_session
            .channel_open_session()
            .await
            .map_err(|e| SftpError::Protocol(format!("Failed to open channel: {}", e)))?;

        // Request SFTP subsystem
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| SftpError::Protocol(format!("Failed to request SFTP subsystem: {}", e)))?;

        // Create SFTP session from the channel stream
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| SftpError::Protocol(format!("Failed to initialize SFTP: {}", e)))?;

        // Get current working directory
        let cwd = sftp
            .canonicalize(".")
            .await
            .unwrap_or_else(|_| "/".to_string());

        info!("SFTP session established, cwd: {:?}", cwd);

        Ok(Self {
            session: sftp,
            current_dir: cwd,
        })
    }

    /// List directory contents
    pub async fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>, SftpError> {
        let resolved_path = self.resolve_path(path);

        info!("Listing directory: {}", resolved_path);

        let entries = self
            .session
            .read_dir(&resolved_path)
            .await
            .map_err(|e| {
                error!("Failed to list directory {}: {}", resolved_path, e);
                SftpError::Protocol(format!("Failed to list directory: {}", e))
            })?;

        let mut file_entries = Vec::new();
        for entry in entries {
            // russh-sftp DirEntry has file_name() returning &str
            let file_name = entry.file_name().to_string();
            let metadata = entry.metadata();

            let is_dir = metadata.is_dir();
            let size = metadata.len();
            let modified = metadata.modified().ok();

            // Build full path
            let full_path = if resolved_path.ends_with('/') {
                format!("{}{}", resolved_path, file_name)
            } else {
                format!("{}/{}", resolved_path, file_name)
            };

            file_entries.push(FileEntry {
                name: file_name,
                path: full_path,
                is_dir,
                size,
                modified,
            });
        }

        // Sort entries: directories first, then by name
        file_entries.sort_by(|a, b| {
            if a.is_dir != b.is_dir {
                b.is_dir.cmp(&a.is_dir)
            } else {
                a.name.to_lowercase().cmp(&b.name.to_lowercase())
            }
        });

        Ok(file_entries)
    }

    /// Download a file from remote to local
    pub async fn download(&mut self, remote: &str, local: &str) -> Result<(), SftpError> {
        let remote_path = self.resolve_path(remote);
        let local_path = PathBuf::from(local);

        info!("Downloading {} to {}", remote_path, local);

        // Open remote file for reading
        let mut remote_file = self
            .session
            .open(&remote_path)
            .await
            .map_err(|e| {
                error!("Failed to open remote file {}: {}", remote_path, e);
                SftpError::FileNotFound {
                    path: remote_path.clone(),
                }
            })?;

        // Create parent directories if needed
        if let Some(parent) = local_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| SftpError::Io(e))?;
        }

        // Create local file for writing
        let mut local_file = tokio::fs::File::create(&local_path)
            .await
            .map_err(|e| {
                error!("Failed to create local file {}: {}", local, e);
                SftpError::Io(e)
            })?;

        // Copy data from remote to local
        let mut buffer = vec![0u8; 64 * 1024]; // 64KB buffer
        let mut total_bytes = 0u64;

        loop {
            let bytes_read = remote_file
                .read(&mut buffer)
                .await
                .map_err(|e| SftpError::TransferInterrupted {
                    reason: e.to_string(),
                })?;

            if bytes_read == 0 {
                break;
            }

            local_file
                .write_all(&buffer[..bytes_read])
                .await
                .map_err(|e| SftpError::TransferInterrupted {
                    reason: e.to_string(),
                })?;

            total_bytes += bytes_read as u64;
        }

        local_file.flush().await.map_err(|e| SftpError::Io(e))?;

        info!("Download complete: {} bytes transferred", total_bytes);

        // Close remote file properly
        remote_file.shutdown().await.ok();

        Ok(())
    }

    /// Upload a file from local to remote
    pub async fn upload(&mut self, local: &str, remote: &str) -> Result<(), SftpError> {
        let remote_path = self.resolve_path(remote);
        let local_path = PathBuf::from(local);

        info!("Uploading {} to {}", local, remote_path);

        // Open local file for reading
        let mut local_file = tokio::fs::File::open(&local_path)
            .await
            .map_err(|e| {
                error!("Failed to open local file {}: {}", local, e);
                SftpError::FileNotFound {
                    path: local.to_string(),
                }
            })?;

        // Get local file size
        let local_metadata = local_file.metadata().await.map_err(|e| SftpError::Io(e))?;
        let file_size = local_metadata.len();

        // Open remote file for writing
        use russh_sftp::protocol::OpenFlags;
        let mut remote_file = self
            .session
            .open_with_flags(
                &remote_path,
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(|e| {
                error!("Failed to create remote file {}: {}", remote_path, e);
                SftpError::Protocol(format!("Failed to create remote file: {}", e))
            })?;

        // Copy data from local to remote
        let mut buffer = vec![0u8; 64 * 1024]; // 64KB buffer
        let mut total_bytes = 0u64;

        loop {
            let bytes_read = local_file
                .read(&mut buffer)
                .await
                .map_err(|e| SftpError::TransferInterrupted {
                    reason: e.to_string(),
                })?;

            if bytes_read == 0 {
                break;
            }

            remote_file
                .write_all(&buffer[..bytes_read])
                .await
                .map_err(|e| SftpError::TransferInterrupted {
                    reason: e.to_string(),
                })?;

            total_bytes += bytes_read as u64;
        }

        remote_file.flush().await.map_err(|e| SftpError::Protocol(e.to_string()))?;

        info!(
            "Upload complete: {} bytes transferred of {} bytes",
            total_bytes, file_size
        );

        // Close remote file properly
        remote_file.shutdown().await.ok();

        Ok(())
    }

    /// Change directory
    pub async fn cd(&mut self, path: &str) -> Result<String, SftpError> {
        let resolved_path = self.resolve_path(path);

        // Verify the directory exists and get canonical path
        let canonical = self
            .session
            .canonicalize(&resolved_path)
            .await
            .map_err(|_e| {
                warn!("Directory not found: {}", resolved_path);
                SftpError::FileNotFound {
                    path: resolved_path.clone(),
                }
            })?;

        // Check if it's a directory
        let metadata = self.session.metadata(&canonical).await.map_err(|e| {
            SftpError::Protocol(format!("Failed to read metadata: {}", e))
        })?;

        if !metadata.is_dir() {
            return Err(SftpError::Protocol(format!(
                "Not a directory: {}",
                canonical
            )));
        }

        self.current_dir = canonical.clone();

        info!("Changed directory to: {}", canonical);

        Ok(canonical)
    }

    /// Create directory
    pub async fn mkdir(&mut self, path: &str) -> Result<(), SftpError> {
        let resolved_path = self.resolve_path(path);

        info!("Creating directory: {}", resolved_path);

        self.session
            .create_dir(&resolved_path)
            .await
            .map_err(|e| {
                error!("Failed to create directory {}: {}", resolved_path, e);
                SftpError::Protocol(format!("Failed to create directory: {}", e))
            })?;

        info!("Directory created: {}", resolved_path);

        Ok(())
    }

    /// Delete file or directory
    pub async fn delete(&mut self, path: &str) -> Result<(), SftpError> {
        let resolved_path = self.resolve_path(path);

        info!("Deleting: {}", resolved_path);

        // Check if it's a file or directory
        let metadata = self
            .session
            .metadata(&resolved_path)
            .await
            .map_err(|_e| SftpError::FileNotFound {
                path: resolved_path.clone(),
            })?;

        if metadata.is_dir() {
            self.session
                .remove_dir(&resolved_path)
                .await
                .map_err(|e| {
                    error!("Failed to remove directory {}: {}", resolved_path, e);
                    SftpError::Protocol(format!("Failed to remove directory: {}", e))
                })?;
        } else {
            self.session
                .remove_file(&resolved_path)
                .await
                .map_err(|e| {
                    error!("Failed to remove file {}: {}", resolved_path, e);
                    SftpError::Protocol(format!("Failed to remove file: {}", e))
                })?;
        }

        info!("Deleted: {}", resolved_path);

        Ok(())
    }

    /// Get current directory
    pub fn pwd(&self) -> &str {
        &self.current_dir
    }

    /// Resolve a path relative to current directory
    fn resolve_path(&self, path: &str) -> String {
        if path.starts_with('/') {
            path.to_string()
        } else if path == "~" || path.starts_with("~/") {
            // Home directory - just return as is, SFTP server will resolve
            path.to_string()
        } else {
            format!("{}/{}", self.current_dir, path)
        }
    }

    /// Get the underlying SFTP session (for advanced operations)
    pub fn session(&self) -> &SftpSession {
        &self.session
    }

    /// Rename a file or directory
    pub async fn rename(&mut self, from: &str, to: &str) -> Result<(), SftpError> {
        let from_path = self.resolve_path(from);
        let to_path = self.resolve_path(to);

        info!("Renaming {} to {}", from_path, to_path);

        self.session
            .rename(&from_path, &to_path)
            .await
            .map_err(|e| {
                error!("Failed to rename {} to {}: {}", from_path, to_path, e);
                SftpError::Protocol(format!("Failed to rename: {}", e))
            })?;

        Ok(())
    }

    /// Check if a file or directory exists
    pub async fn exists(&mut self, path: &str) -> bool {
        let resolved_path = self.resolve_path(path);
        self.session.metadata(&resolved_path).await.is_ok()
    }

    /// Get file or directory metadata
    pub async fn metadata(&mut self, path: &str) -> Result<FileEntry, SftpError> {
        let resolved_path = self.resolve_path(path);

        let canonical = self
            .session
            .canonicalize(&resolved_path)
            .await
            .map_err(|_e| SftpError::FileNotFound {
                path: resolved_path.clone(),
            })?;

        let metadata = self.session.metadata(&canonical).await.map_err(|e| {
            SftpError::Protocol(format!("Failed to read metadata: {}", e))
        })?;

        // Extract name from path
        let name = canonical
            .rsplit('/')
            .next()
            .unwrap_or(&canonical)
            .to_string();

        Ok(FileEntry {
            name,
            path: canonical,
            is_dir: metadata.is_dir(),
            size: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}