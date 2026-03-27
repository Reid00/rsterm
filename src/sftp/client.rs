// src/sftp/client.rs
//! SFTP client implementation using russh-sftp

use std::path::PathBuf;

use russh::client::Handler;
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc as tokio_mpsc;
use tracing::{error, info, warn};

use crate::error::SftpError;

/// Trait abstracting SFTP session operations for testing
#[async_trait::async_trait]
#[mockall::automock]
pub trait SftpSessionOps: Send + Sync + std::fmt::Debug {
    async fn read_dir(&self, path: &str) -> Result<Vec<FileEntry>, SftpError>;
    async fn open(&self, path: &str) -> Result<Box<dyn SftpFileReadOps>, SftpError>;
    async fn create(&self, path: &str) -> Result<Box<dyn SftpFileWriteOps>, SftpError>;
    async fn create_dir(&self, path: &str) -> Result<(), SftpError>;
    async fn remove_dir(&self, path: &str) -> Result<(), SftpError>;
    async fn remove_file(&self, path: &str) -> Result<(), SftpError>;
    async fn canonicalize(&self, path: &str) -> Result<String, SftpError>;
    async fn is_dir(&self, path: &str) -> Result<bool, SftpError>;
    async fn metadata(&self, path: &str) -> Result<FileMetadata, SftpError>;
    async fn rename(&self, from: &str, to: &str) -> Result<(), SftpError>;
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait SftpFileReadOps: Send + Sync {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, SftpError>;
    async fn close(&mut self) -> Result<(), SftpError>;
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait SftpFileWriteOps: Send + Sync {
    async fn write_all(&mut self, buf: &[u8]) -> Result<(), SftpError>;
    async fn close(&mut self) -> Result<(), SftpError>;
}

// ============================================================================
// Real SFTP session implementation wrapping russh_sftp
// ============================================================================

/// Wrapper around russh_sftp::client::SftpSession implementing SftpSessionOps
pub struct RealSftpSession {
    inner: SftpSession,
}

impl std::fmt::Debug for RealSftpSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealSftpSession")
            .finish_non_exhaustive()
    }
}

impl RealSftpSession {
    pub fn new(session: SftpSession) -> Self {
        Self { inner: session }
    }
}

/// Wrapper around russh_sftp file handle for reading
pub struct RealSftpFileReader {
    inner: russh_sftp::client::fs::File,
}

/// Wrapper around russh_sftp file handle for writing
pub struct RealSftpFileWriter {
    inner: russh_sftp::client::fs::File,
}

#[async_trait::async_trait]
impl SftpSessionOps for RealSftpSession {
    async fn read_dir(&self, path: &str) -> Result<Vec<FileEntry>, SftpError> {
        let entries = self.inner.read_dir(path).await.map_err(|e| {
            SftpError::Protocol(format!("Failed to list directory: {}", e))
        })?;

        let mut file_entries = Vec::new();
        for entry in entries {
            let file_name = entry.file_name().to_string();
            let metadata = entry.metadata();

            let is_dir = metadata.is_dir();
            let size = metadata.len();
            let modified = metadata.modified().ok();

            let full_path = if path.ends_with('/') {
                format!("{}{}", path, file_name)
            } else {
                format!("{}/{}", path, file_name)
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

    async fn open(&self, path: &str) -> Result<Box<dyn SftpFileReadOps>, SftpError> {
        let file = self.inner.open(path).await.map_err(|e| {
            SftpError::FileNotFound { path: path.to_string() }
        })?;
        Ok(Box::new(RealSftpFileReader { inner: file }))
    }

    async fn create(&self, path: &str) -> Result<Box<dyn SftpFileWriteOps>, SftpError> {
        use russh_sftp::protocol::OpenFlags;
        let file = self.inner
            .open_with_flags(path, OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE)
            .await
            .map_err(|e| {
                SftpError::Protocol(format!("Failed to create file: {}", e))
            })?;
        Ok(Box::new(RealSftpFileWriter { inner: file }))
    }

    async fn create_dir(&self, path: &str) -> Result<(), SftpError> {
        self.inner.create_dir(path).await.map_err(|e| {
            SftpError::Protocol(format!("Failed to create directory: {}", e))
        })
    }

    async fn remove_dir(&self, path: &str) -> Result<(), SftpError> {
        self.inner.remove_dir(path).await.map_err(|e| {
            SftpError::Protocol(format!("Failed to remove directory: {}", e))
        })
    }

    async fn remove_file(&self, path: &str) -> Result<(), SftpError> {
        self.inner.remove_file(path).await.map_err(|e| {
            SftpError::Protocol(format!("Failed to remove file: {}", e))
        })
    }

    async fn canonicalize(&self, path: &str) -> Result<String, SftpError> {
        self.inner.canonicalize(path).await.map_err(|e| {
            SftpError::Protocol(format!("Failed to canonicalize path: {}", e))
        })
    }

    async fn is_dir(&self, path: &str) -> Result<bool, SftpError> {
        let metadata = self.inner.metadata(path).await.map_err(|_e| {
            SftpError::FileNotFound { path: path.to_string() }
        })?;
        Ok(metadata.is_dir())
    }

    async fn metadata(&self, path: &str) -> Result<FileMetadata, SftpError> {
        let metadata = self.inner.metadata(path).await.map_err(|e| {
            SftpError::Protocol(format!("Failed to read metadata: {}", e))
        })?;
        Ok(FileMetadata {
            is_dir: metadata.is_dir(),
            size: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }

    async fn rename(&self, from: &str, to: &str) -> Result<(), SftpError> {
        self.inner.rename(from, to).await.map_err(|e| {
            SftpError::Protocol(format!("Failed to rename: {}", e))
        })
    }
}

#[async_trait::async_trait]
impl SftpFileReadOps for RealSftpFileReader {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, SftpError> {
        self.inner.read(buf).await.map_err(|e| {
            SftpError::TransferInterrupted { reason: e.to_string() }
        })
    }

    async fn close(&mut self) -> Result<(), SftpError> {
        self.inner.shutdown().await.map_err(|e| {
            SftpError::Protocol(format!("Failed to close file: {}", e))
        })
    }
}

#[async_trait::async_trait]
impl SftpFileWriteOps for RealSftpFileWriter {
    async fn write_all(&mut self, buf: &[u8]) -> Result<(), SftpError> {
        self.inner.write_all(buf).await.map_err(|e| {
            SftpError::TransferInterrupted { reason: e.to_string() }
        })
    }

    async fn close(&mut self) -> Result<(), SftpError> {
        self.inner.flush().await.map_err(|e| {
            SftpError::Protocol(format!("Failed to flush file: {}", e))
        })?;
        self.inner.shutdown().await.map_err(|e| {
            SftpError::Protocol(format!("Failed to close file: {}", e))
        })
    }
}

#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
}

/// SFTP client for file transfers over SSH
pub struct SftpClient {
    session: Box<dyn SftpSessionOps>,
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
            session: Box::new(RealSftpSession::new(sftp)),
            current_dir: cwd,
        })
    }

    /// Create an SFTP client with a custom session (for testing)
    pub fn with_session(session: Box<dyn SftpSessionOps>, cwd: String) -> Self {
        Self {
            session,
            current_dir: cwd,
        }
    }

    /// List directory contents
    pub async fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>, SftpError> {
        let resolved_path = self.resolve_path(path);

        info!("Listing directory: {}", resolved_path);

        self.session.read_dir(&resolved_path).await
    }

    /// Download a file from remote to local
    pub async fn download(&mut self, remote: &str, local: &str) -> Result<(), SftpError> {
        let remote_path = self.resolve_path(remote);
        let local_path = PathBuf::from(local);

        info!("Downloading {} to {}", remote_path, local);

        // Open remote file for reading
        let mut remote_file = self.session.open(&remote_path).await?;

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
            let bytes_read = remote_file.read(&mut buffer).await?;

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
        remote_file.close().await.ok();

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
        let mut remote_file = self.session.create(&remote_path).await?;

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

            remote_file.write_all(&buffer[..bytes_read]).await?;

            total_bytes += bytes_read as u64;
        }

        remote_file.close().await?;

        info!(
            "Upload complete: {} bytes transferred of {} bytes",
            total_bytes, file_size
        );

        Ok(())
    }

    /// Change directory
    pub async fn cd(&mut self, path: &str) -> Result<String, SftpError> {
        let resolved_path = self.resolve_path(path);

        // Verify the directory exists and get canonical path
        let canonical = self.session.canonicalize(&resolved_path).await.map_err(|_e| {
            warn!("Directory not found: {}", resolved_path);
            SftpError::FileNotFound {
                path: resolved_path.clone(),
            }
        })?;

        // Check if it's a directory
        let metadata = self.session.metadata(&canonical).await?;

        if !metadata.is_dir {
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

        self.session.create_dir(&resolved_path).await?;

        info!("Directory created: {}", resolved_path);

        Ok(())
    }

    /// Delete file or directory
    pub async fn delete(&mut self, path: &str) -> Result<(), SftpError> {
        let resolved_path = self.resolve_path(path);

        info!("Deleting: {}", resolved_path);

        // Check if it's a file or directory
        let metadata = self.session.metadata(&resolved_path).await.map_err(|_e| SftpError::FileNotFound {
            path: resolved_path.clone(),
        })?;

        if metadata.is_dir {
            self.session.remove_dir(&resolved_path).await?;
        } else {
            self.session.remove_file(&resolved_path).await?;
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

    /// Rename a file or directory
    pub async fn rename(&mut self, from: &str, to: &str) -> Result<(), SftpError> {
        let from_path = self.resolve_path(from);
        let to_path = self.resolve_path(to);

        info!("Renaming {} to {}", from_path, to_path);

        self.session.rename(&from_path, &to_path).await?;

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

        let canonical = self.session.canonicalize(&resolved_path).await.map_err(|_e| SftpError::FileNotFound {
            path: resolved_path.clone(),
        })?;

        let metadata = self.session.metadata(&canonical).await?;

        // Extract name from path
        let name = canonical
            .rsplit('/')
            .next()
            .unwrap_or(&canonical)
            .to_string();

        Ok(FileEntry {
            name,
            path: canonical,
            is_dir: metadata.is_dir,
            size: metadata.size,
            modified: metadata.modified,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_client(mock: MockSftpSessionOps) -> SftpClient {
        SftpClient {
            session: Box::new(mock),
            current_dir: "/home/user".to_string(),
        }
    }

    #[tokio::test]
    async fn test_list_dir_success() {
        let mut mock = MockSftpSessionOps::new();
        // resolve_path(".") appends "." to current_dir: "/home/user" + "/" + "." = "/home/user/."
        // Mock returns entries sorted (directories first) as RealSftpSession::read_dir does
        mock.expect_read_dir()
            .with(mockall::predicate::eq("/home/user/."))
            .returning(|_| Box::pin(async move {
                Ok(vec![
                    FileEntry { name: "folder".into(), path: "/home/user/folder".into(), is_dir: true, size: 0, modified: None },
                    FileEntry { name: "file.txt".into(), path: "/home/user/file.txt".into(), is_dir: false, size: 100, modified: None },
                ])
            }));
        let mut client = create_test_client(mock);
        let result = client.list_dir(".").await.unwrap();
        assert_eq!(result.len(), 2);
        assert!(result[0].is_dir); // directories sorted first
    }

    #[tokio::test]
    async fn test_list_dir_empty() {
        let mut mock = MockSftpSessionOps::new();
        mock.expect_read_dir()
            .with(mockall::predicate::eq("/home/user/."))
            .returning(|_| Box::pin(async move { Ok(vec![]) }));
        let mut client = create_test_client(mock);
        let result = client.list_dir(".").await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_list_dir_error() {
        let mut mock = MockSftpSessionOps::new();
        mock.expect_read_dir()
            .with(mockall::predicate::eq("/home/user/."))
            .returning(|_| Box::pin(async move { Err(SftpError::PermissionDenied { path: "/home/user/.".to_string() }) }));
        let mut client = create_test_client(mock);
        let result = client.list_dir(".").await;
        assert!(matches!(result, Err(SftpError::PermissionDenied { .. })));
    }

    #[tokio::test]
    async fn test_mkdir_success() {
        let mut mock = MockSftpSessionOps::new();
        mock.expect_create_dir()
            .with(mockall::predicate::eq("/home/user/newdir"))
            .returning(|_| Box::pin(async move { Ok(()) }));
        let mut client = create_test_client(mock);
        let result = client.mkdir("newdir").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mkdir_error() {
        let mut mock = MockSftpSessionOps::new();
        mock.expect_create_dir()
            .with(mockall::predicate::eq("/home/user/no-permission"))
            .returning(|_| Box::pin(async move { Err(SftpError::PermissionDenied { path: "/home/user/no-permission".to_string() }) }));
        let mut client = create_test_client(mock);
        let result = client.mkdir("no-permission").await;
        assert!(matches!(result, Err(SftpError::PermissionDenied { .. })));
    }

    #[tokio::test]
    async fn test_delete_file() {
        let mut mock = MockSftpSessionOps::new();
        // delete() first calls metadata to check if it's a file or directory
        mock.expect_metadata()
            .with(mockall::predicate::eq("/home/user/testfile.txt"))
            .returning(|_| Box::pin(async move { Ok(FileMetadata { is_dir: false, size: 100, modified: None }) }));
        mock.expect_remove_file()
            .with(mockall::predicate::eq("/home/user/testfile.txt"))
            .returning(|_| Box::pin(async move { Ok(()) }));
        let mut client = create_test_client(mock);
        let result = client.delete("testfile.txt").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_delete_directory() {
        let mut mock = MockSftpSessionOps::new();
        // delete() first calls metadata to check if it's a file or directory
        mock.expect_metadata()
            .with(mockall::predicate::eq("/home/user/testdir"))
            .returning(|_| Box::pin(async move { Ok(FileMetadata { is_dir: true, size: 0, modified: None }) }));
        mock.expect_remove_dir()
            .with(mockall::predicate::eq("/home/user/testdir"))
            .returning(|_| Box::pin(async move { Ok(()) }));
        let mut client = create_test_client(mock);
        let result = client.delete("testdir").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_upload_success() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_file = temp_dir.path().join("test.txt");
        tokio::fs::write(&temp_file, b"test content").await.unwrap();

        let mut mock = MockSftpSessionOps::new();
        mock.expect_create()
            .with(mockall::predicate::eq("/home/user/test.txt"))
            .returning(|_| {
                let mut file_mock = MockSftpFileWriteOps::new();
                file_mock.expect_write_all()
                    .returning(|_| Box::pin(async move { Ok(()) }));
                file_mock.expect_close()
                    .returning(|| Box::pin(async move { Ok(()) }));
                Box::pin(async move { Ok(Box::new(file_mock) as Box<dyn SftpFileWriteOps>) })
            });

        let mut client = create_test_client(mock);
        let result = client.upload(temp_file.to_str().unwrap(), "test.txt").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_upload_file_not_found() {
        let mock = MockSftpSessionOps::new();
        // No expectations needed - upload fails before calling session methods

        let mut client = create_test_client(mock);
        let result = client.upload("/nonexistent/file.txt", "remote.txt").await;
        assert!(matches!(result, Err(SftpError::FileNotFound { .. })));
    }

    #[tokio::test]
    async fn test_download_success() {
        let temp_dir = tempfile::tempdir().unwrap();
        let local_path = temp_dir.path().join("downloaded.txt");

        let mut mock = MockSftpSessionOps::new();
        mock.expect_open()
            .with(mockall::predicate::eq("/home/user/remote.txt"))
            .returning(|_| {
                let mut file_mock = MockSftpFileReadOps::new();
                // First read returns some data
                file_mock.expect_read()
                    .times(1)
                    .returning(|buf| {
                        let data = b"test data";
                        buf[..data.len()].copy_from_slice(data);
                        Box::pin(async move { Ok(data.len()) })
                    });
                // Second read returns 0 (EOF)
                file_mock.expect_read()
                    .times(1)
                    .returning(|_| Box::pin(async move { Ok(0) }));
                file_mock.expect_close()
                    .returning(|| Box::pin(async move { Ok(()) }));
                Box::pin(async move { Ok(Box::new(file_mock) as Box<dyn SftpFileReadOps>) })
            });

        let mut client = create_test_client(mock);
        let result = client.download("remote.txt", local_path.to_str().unwrap()).await;
        assert!(result.is_ok());

        // Verify the downloaded content
        let content = tokio::fs::read_to_string(&local_path).await.unwrap();
        assert_eq!(content, "test data");
    }

    #[tokio::test]
    async fn test_cd_success() {
        let mut mock = MockSftpSessionOps::new();
        mock.expect_canonicalize()
            .with(mockall::predicate::eq("/home/user/subdir"))
            .returning(|_| Box::pin(async move { Ok("/home/user/subdir".to_string()) }));
        mock.expect_metadata()
            .with(mockall::predicate::eq("/home/user/subdir"))
            .returning(|_| Box::pin(async move { Ok(FileMetadata { is_dir: true, size: 0, modified: None }) }));

        let mut client = create_test_client(mock);
        let result = client.cd("subdir").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "/home/user/subdir");
        assert_eq!(client.pwd(), "/home/user/subdir");
    }

    #[tokio::test]
    async fn test_cd_not_directory() {
        let mut mock = MockSftpSessionOps::new();
        mock.expect_canonicalize()
            .with(mockall::predicate::eq("/home/user/notadir"))
            .returning(|_| Box::pin(async move { Ok("/home/user/notadir".to_string()) }));
        mock.expect_metadata()
            .with(mockall::predicate::eq("/home/user/notadir"))
            .returning(|_| Box::pin(async move { Ok(FileMetadata { is_dir: false, size: 100, modified: None }) }));

        let mut client = create_test_client(mock);
        let result = client.cd("notadir").await;
        assert!(matches!(result, Err(SftpError::Protocol(_))));
    }
}