use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use russh::client::Handler;
use russh_sftp::client::{error::Error as ProtocolError, RawSftpSession};
use russh_sftp::protocol::{FileAttributes, OpenFlags, Packet, StatusCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::error::SftpError;

const BUFFER_SIZE: usize = 64 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[allow(clippy::double_must_use)]
#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait SftpSessionOps: Send + Sync + std::fmt::Debug {
    async fn read_dir(&self, path: &str) -> Result<Vec<FileEntry>, SftpError>;
    async fn open(&self, path: &str) -> Result<Box<dyn SftpFileReadOps>, SftpError>;
    async fn create_exclusive(&self, path: &str) -> Result<Box<dyn SftpFileWriteOps>, SftpError>;
    async fn create_dir(&self, path: &str) -> Result<(), SftpError>;
    async fn remove_dir(&self, path: &str) -> Result<(), SftpError>;
    async fn remove_file(&self, path: &str) -> Result<(), SftpError>;
    async fn canonicalize(&self, path: &str) -> Result<String, SftpError>;
    async fn metadata(&self, path: &str) -> Result<FileMetadata, SftpError>;
    async fn symlink_metadata(&self, path: &str) -> Result<FileMetadata, SftpError>;
    async fn rename(&self, from: &str, to: &str, overwrite: bool) -> Result<(), SftpError>;
    async fn chmod(&self, path: &str, mode: u32) -> Result<(), SftpError>;
}

#[allow(clippy::double_must_use)]
#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait SftpFileReadOps: Send + Sync {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, SftpError>;
    async fn close(&mut self) -> Result<(), SftpError>;
}

#[allow(clippy::double_must_use)]
#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait SftpFileWriteOps: Send + Sync {
    async fn write_all(&mut self, buf: &[u8]) -> Result<(), SftpError>;
    async fn close(&mut self) -> Result<(), SftpError>;
}

pub struct RealSftpSession {
    inner: Arc<RawSftpSession>,
    posix_rename: bool,
    read_size: usize,
    write_size: usize,
}

impl std::fmt::Debug for RealSftpSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealSftpSession").finish_non_exhaustive()
    }
}

impl RealSftpSession {
    async fn new<S>(stream: S, timeout: Duration) -> Result<Self, SftpError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let mut inner = RawSftpSession::new(stream);
        inner.set_timeout(timeout.as_secs().max(1)).await;
        let version = inner.init().await.map_err(protocol_error)?;
        let posix_rename = version
            .extensions
            .get("posix-rename@openssh.com")
            .is_some_and(|version| version == "1");
        let mut read_size = BUFFER_SIZE;
        let mut write_size = 32 * 1024;
        if version
            .extensions
            .get("limits@openssh.com")
            .is_some_and(|version| version == "1")
        {
            let limits = inner.limits().await.map_err(protocol_error)?;
            if limits.max_read_len > 0 {
                read_size = limits.max_read_len.min(BUFFER_SIZE as u64) as usize;
            }
            if limits.max_write_len > 0 {
                write_size = limits.max_write_len.min(BUFFER_SIZE as u64) as usize;
            }
            inner.set_limits(Arc::new(limits.into()));
        }
        Ok(Self {
            inner: Arc::new(inner),
            posix_rename,
            read_size,
            write_size,
        })
    }
}

struct RealSftpFile {
    session: Arc<RawSftpSession>,
    handle: Option<String>,
    offset: u64,
    chunk_size: usize,
}

#[async_trait::async_trait]
impl SftpSessionOps for RealSftpSession {
    async fn read_dir(&self, path: &str) -> Result<Vec<FileEntry>, SftpError> {
        let handle = self
            .inner
            .opendir(path)
            .await
            .map_err(protocol_error)?
            .handle;
        let result = async {
            let mut entries = Vec::new();
            loop {
                match self.inner.readdir(&handle).await {
                    Ok(listing) => {
                        if listing.files.is_empty() {
                            break;
                        }
                        for entry in listing.files {
                            if entry.filename == "." || entry.filename == ".." {
                                continue;
                            }
                            if entry.filename.contains(['/', '\0']) {
                                return Err(SftpError::Protocol(
                                    "Invalid directory entry name".into(),
                                ));
                            }
                            let metadata = metadata(entry.attrs);
                            entries.push(FileEntry {
                                path: remote_join(path, &entry.filename),
                                name: entry.filename,
                                is_dir: metadata.is_dir,
                                is_symlink: metadata.is_symlink,
                                size: metadata.size,
                                modified: metadata.modified,
                                permissions: metadata.permissions,
                            });
                        }
                    }
                    Err(ProtocolError::Status(status)) if status.status_code == StatusCode::Eof => {
                        break
                    }
                    Err(error) => return Err(protocol_error(error)),
                }
            }
            entries.sort_by(|a, b| {
                b.is_dir
                    .cmp(&a.is_dir)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            Ok(entries)
        }
        .await;
        let close = self.inner.close(handle).await.map_err(protocol_error);
        let entries = result?;
        close?;
        Ok(entries)
    }

    async fn open(&self, path: &str) -> Result<Box<dyn SftpFileReadOps>, SftpError> {
        let handle = self
            .inner
            .open(path, OpenFlags::READ, FileAttributes::empty())
            .await
            .map_err(|error| path_error(error, path))?
            .handle;
        Ok(Box::new(RealSftpFile {
            session: self.inner.clone(),
            handle: Some(handle),
            offset: 0,
            chunk_size: self.read_size,
        }))
    }

    async fn create_exclusive(&self, path: &str) -> Result<Box<dyn SftpFileWriteOps>, SftpError> {
        let mut attrs = FileAttributes::empty();
        attrs.permissions = Some(0o600);
        let handle = self
            .inner
            .open(
                path,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                attrs,
            )
            .await
            .map_err(|error| path_error(error, path))?
            .handle;
        Ok(Box::new(RealSftpFile {
            session: self.inner.clone(),
            handle: Some(handle),
            offset: 0,
            chunk_size: self.write_size,
        }))
    }

    async fn create_dir(&self, path: &str) -> Result<(), SftpError> {
        self.inner
            .mkdir(path, FileAttributes::empty())
            .await
            .map(|_| ())
            .map_err(|error| path_error(error, path))
    }
    async fn remove_dir(&self, path: &str) -> Result<(), SftpError> {
        self.inner
            .rmdir(path)
            .await
            .map(|_| ())
            .map_err(|error| path_error(error, path))
    }
    async fn remove_file(&self, path: &str) -> Result<(), SftpError> {
        self.inner
            .remove(path)
            .await
            .map(|_| ())
            .map_err(|error| path_error(error, path))
    }
    async fn canonicalize(&self, path: &str) -> Result<String, SftpError> {
        self.inner
            .realpath(path)
            .await
            .map_err(|error| path_error(error, path))?
            .files
            .into_iter()
            .next()
            .map(|entry| entry.filename)
            .ok_or_else(|| SftpError::Protocol("Server returned no canonical path".into()))
    }
    async fn metadata(&self, path: &str) -> Result<FileMetadata, SftpError> {
        self.inner
            .stat(path)
            .await
            .map(|attrs| metadata(attrs.attrs))
            .map_err(|error| path_error(error, path))
    }
    async fn symlink_metadata(&self, path: &str) -> Result<FileMetadata, SftpError> {
        self.inner
            .lstat(path)
            .await
            .map(|attrs| metadata(attrs.attrs))
            .map_err(|error| path_error(error, path))
    }
    async fn rename(&self, from: &str, to: &str, overwrite: bool) -> Result<(), SftpError> {
        if !overwrite {
            return self
                .inner
                .rename(from, to)
                .await
                .map(|_| ())
                .map_err(|error| path_error(error, to));
        }
        if !self.posix_rename {
            return Err(SftpError::Unsupported(
                "Server does not advertise posix-rename@openssh.com; safe overwrite is unavailable"
                    .into(),
            ));
        }
        let mut data = Vec::new();
        for path in [from, to] {
            data.extend_from_slice(&(path.len() as u32).to_be_bytes());
            data.extend_from_slice(path.as_bytes());
        }
        match self
            .inner
            .extended("posix-rename@openssh.com", data)
            .await
            .map_err(protocol_error)?
        {
            Packet::Status(status) if status.status_code == StatusCode::Ok => Ok(()),
            Packet::Status(status) => Err(path_error(ProtocolError::Status(status), to)),
            _ => Err(SftpError::Protocol("Invalid POSIX rename response".into())),
        }
    }
    async fn chmod(&self, path: &str, mode: u32) -> Result<(), SftpError> {
        let mut attrs = FileAttributes::empty();
        attrs.permissions = Some(mode & 0o7777);
        self.inner
            .setstat(path, attrs)
            .await
            .map(|_| ())
            .map_err(|error| path_error(error, path))
    }
}

#[async_trait::async_trait]
impl SftpFileReadOps for RealSftpFile {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, SftpError> {
        let handle = self.handle.as_ref().ok_or(SftpError::NotConnected)?;
        match self
            .session
            .read(handle, self.offset, buf.len().min(self.chunk_size) as u32)
            .await
        {
            Ok(data) => {
                if data.data.len() > buf.len() {
                    return Err(SftpError::Protocol("Oversized SFTP read response".into()));
                }
                let count = data.data.len();
                buf[..count].copy_from_slice(&data.data);
                self.offset += count as u64;
                Ok(count)
            }
            Err(ProtocolError::Status(status)) if status.status_code == StatusCode::Eof => Ok(0),
            Err(error) => Err(protocol_error(error)),
        }
    }
    async fn close(&mut self) -> Result<(), SftpError> {
        if let Some(handle) = self.handle.take() {
            self.session.close(handle).await.map_err(protocol_error)?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl SftpFileWriteOps for RealSftpFile {
    async fn write_all(&mut self, buf: &[u8]) -> Result<(), SftpError> {
        let handle = self.handle.as_ref().ok_or(SftpError::NotConnected)?;
        for chunk in buf.chunks(self.chunk_size) {
            self.session
                .write(handle, self.offset, chunk.to_vec())
                .await
                .map_err(protocol_error)?;
            self.offset += chunk.len() as u64;
        }
        Ok(())
    }
    async fn close(&mut self) -> Result<(), SftpError> {
        if let Some(handle) = self.handle.take() {
            self.session.close(handle).await.map_err(protocol_error)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct FileMetadata {
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub permissions: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub permissions: Option<u32>,
}

#[derive(Clone)]
pub struct SftpClient {
    session: Arc<dyn SftpSessionOps>,
    home: String,
}

impl SftpClient {
    pub async fn new<H: Handler>(
        ssh: &russh::client::Handle<H>,
        timeout: Duration,
    ) -> Result<Self, SftpError> {
        tokio::time::timeout(timeout, async {
            let mut channel = ssh
                .channel_open_session()
                .await
                .map_err(|error| SftpError::Protocol(error.to_string()))?;
            channel
                .request_subsystem(true, "sftp")
                .await
                .map_err(|error| SftpError::Protocol(error.to_string()))?;
            crate::backend::wait_request_reply(&mut channel)
                .await
                .map_err(|error| SftpError::Protocol(error.to_string()))?;
            let session = RealSftpSession::new(channel.into_stream(), timeout).await?;
            let home = session.canonicalize(".").await?;
            if !home.starts_with('/') {
                return Err(SftpError::Protocol(
                    "Server returned a relative home directory".into(),
                ));
            }
            Ok(Self {
                session: Arc::new(session),
                home,
            })
        })
        .await
        .map_err(|_| SftpError::Protocol("SFTP initialization timed out".into()))?
    }

    pub fn with_session(session: Box<dyn SftpSessionOps>, home: String) -> Self {
        Self {
            session: Arc::from(session),
            home,
        }
    }

    pub fn pwd(&self) -> &str {
        &self.home
    }

    pub fn resolve_path(&self, path: &str) -> Result<String, SftpError> {
        if path.contains('\0') {
            return Err(SftpError::Protocol("Paths cannot contain NUL".into()));
        }
        if path.starts_with('/') {
            return Ok(path.to_string());
        }
        if path == "~" || path.is_empty() || path == "." {
            return Ok(self.home.clone());
        }
        if let Some(relative) = path.strip_prefix("~/") {
            return Ok(remote_join(&self.home, relative));
        }
        if path.starts_with('~') {
            return Err(SftpError::Unsupported(
                "Only the authenticated user's home (~) can be expanded".into(),
            ));
        }
        Ok(remote_join(&self.home, path))
    }

    pub async fn list_dir(&self, path: &str) -> Result<Vec<FileEntry>, SftpError> {
        self.session.read_dir(&self.resolve_path(path)?).await
    }
    pub async fn mkdir(&self, path: &str) -> Result<(), SftpError> {
        self.session.create_dir(&self.resolve_path(path)?).await
    }
    pub async fn delete(&self, path: &str) -> Result<(), SftpError> {
        let path = self.resolve_path(path)?;
        let path = path.trim_end_matches('/');
        if path.is_empty() {
            return Err(SftpError::Protocol("Cannot delete the remote root".into()));
        }
        let info = self.session.symlink_metadata(path).await?;
        if info.is_dir && !info.is_symlink {
            self.session.remove_dir(path).await
        } else {
            self.session.remove_file(path).await
        }
    }
    pub async fn rename(&self, from: &str, to: &str) -> Result<(), SftpError> {
        self.session
            .rename(&self.resolve_path(from)?, &self.resolve_path(to)?, false)
            .await
    }
    pub async fn chmod(&self, path: &str, mode: u32) -> Result<(), SftpError> {
        self.session.chmod(&self.resolve_path(path)?, mode).await
    }
    pub async fn metadata(&self, path: &str) -> Result<FileMetadata, SftpError> {
        self.session.metadata(&self.resolve_path(path)?).await
    }

    pub async fn upload(
        &self,
        local: &Path,
        remote: &str,
        overwrite: bool,
        cancel: &CancellationToken,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), SftpError> {
        check_cancel(cancel)?;
        let destination = self.resolve_path(remote)?;
        let temporary = remote_temp_path(&destination)?;
        let mut local_file = tokio::fs::File::open(local).await?;
        let total = local_file.metadata().await?.len();
        check_cancel(cancel)?;
        let mut remote_file = self.session.create_exclusive(&temporary).await?;
        let copied = async {
            let mut buffer = vec![0; BUFFER_SIZE];
            let mut done = 0;
            let mut report = Progress::new(progress, total);
            loop {
                check_cancel(cancel)?;
                let count = local_file.read(&mut buffer).await?;
                if count == 0 {
                    break;
                }
                check_cancel(cancel)?;
                remote_file.write_all(&buffer[..count]).await?;
                done += count as u64;
                report.update(done);
            }
            check_cancel(cancel)?;
            report.finish(done);
            Ok::<_, SftpError>(())
        }
        .await;
        let closed = remote_file.close().await;
        let result = async {
            copied?;
            closed?;
            check_cancel(cancel)?;
            self.session
                .rename(&temporary, &destination, overwrite)
                .await
        }
        .await;
        if let Err(error) = result {
            let cleanup = self.session.remove_file(&temporary).await;
            if let Err(cleanup) = cleanup {
                return Err(SftpError::TransferInterrupted {
                    reason: format!(
                        "{error}; temporary file {temporary} could not be removed: {cleanup}"
                    ),
                });
            }
            return Err(error);
        }
        Ok(())
    }

    pub async fn download(
        &self,
        remote: &str,
        local: &Path,
        overwrite: bool,
        cancel: &CancellationToken,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), SftpError> {
        check_cancel(cancel)?;
        let source = self.resolve_path(remote)?;
        let total = self.session.metadata(&source).await?.size;
        let temporary = local_temp_path(local)?;
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut local_file = options.open(&temporary).await?;
        let result = async {
            check_cancel(cancel)?;
            let mut remote_file = self.session.open(&source).await?;
            let copied = async {
                let mut buffer = vec![0; BUFFER_SIZE];
                let mut done = 0;
                let mut report = Progress::new(progress, total);
                loop {
                    check_cancel(cancel)?;
                    let count = remote_file.read(&mut buffer).await?;
                    if count == 0 {
                        break;
                    }
                    check_cancel(cancel)?;
                    local_file.write_all(&buffer[..count]).await?;
                    done += count as u64;
                    report.update(done);
                }
                check_cancel(cancel)?;
                local_file.flush().await?;
                local_file.sync_all().await?;
                report.finish(done);
                Ok::<_, SftpError>(())
            }
            .await;
            let closed = remote_file.close().await;
            copied?;
            closed?;
            check_cancel(cancel)?;
            Ok::<_, SftpError>(())
        }
        .await;
        drop(local_file);
        let result = match result {
            Ok(()) => {
                if let Err(error) = check_cancel(cancel) {
                    Err(error)
                } else if overwrite {
                    tokio::fs::rename(&temporary, local)
                        .await
                        .map_err(SftpError::Io)
                } else {
                    tokio::fs::hard_link(&temporary, local)
                        .await
                        .map_err(|error| {
                            if error.kind() == std::io::ErrorKind::AlreadyExists {
                                SftpError::AlreadyExists {
                                    path: local.display().to_string(),
                                }
                            } else {
                                SftpError::Io(error)
                            }
                        })
                }
            }
            Err(error) => Err(error),
        };
        match tokio::fs::remove_file(&temporary).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(SftpError::TransferInterrupted {
                    reason: format!(
                        "Temporary file {} could not be removed: {error}",
                        temporary.display()
                    ),
                })
            }
        }
        result
    }
}

struct Progress<'a> {
    callback: &'a (dyn Fn(u64, u64) + Send + Sync),
    total: u64,
    last: Instant,
}
impl<'a> Progress<'a> {
    fn new(callback: &'a (dyn Fn(u64, u64) + Send + Sync), total: u64) -> Self {
        callback(0, total);
        Self {
            callback,
            total,
            last: Instant::now(),
        }
    }
    fn update(&mut self, done: u64) {
        if self.last.elapsed() >= PROGRESS_INTERVAL {
            (self.callback)(done, self.total);
            self.last = Instant::now();
        }
    }
    fn finish(&self, done: u64) {
        (self.callback)(done, self.total);
    }
}

fn metadata(attrs: FileAttributes) -> FileMetadata {
    FileMetadata {
        is_dir: attrs.is_dir(),
        is_symlink: attrs.is_symlink(),
        size: attrs.len(),
        modified: attrs.modified().ok(),
        permissions: attrs.permissions,
    }
}
fn protocol_error(error: ProtocolError) -> SftpError {
    SftpError::Protocol(error.to_string())
}
fn path_error(error: ProtocolError, path: &str) -> SftpError {
    match error {
        ProtocolError::Status(status) if status.status_code == StatusCode::NoSuchFile => {
            SftpError::FileNotFound { path: path.into() }
        }
        ProtocolError::Status(status) if status.status_code == StatusCode::PermissionDenied => {
            SftpError::PermissionDenied { path: path.into() }
        }
        error => protocol_error(error),
    }
}
fn check_cancel(cancel: &CancellationToken) -> Result<(), SftpError> {
    if cancel.is_cancelled() {
        Err(SftpError::Cancelled)
    } else {
        Ok(())
    }
}
pub fn remote_parent(path: &str) -> String {
    if path.chars().all(|character| character == '/') {
        return "/".into();
    }
    let path = path.trim_end_matches('/');
    match path.rsplit_once('/') {
        Some(("", _)) => "/".into(),
        Some((parent, _)) => parent.into(),
        None => ".".into(),
    }
}
fn remote_join(parent: &str, child: &str) -> String {
    format!("{}/{}", parent.trim_end_matches('/'), child)
}
fn remote_temp_path(destination: &str) -> Result<String, SftpError> {
    if destination.ends_with('/') {
        return Err(SftpError::Protocol("Destination must name a file".into()));
    }
    Ok(remote_join(
        &remote_parent(destination),
        &format!(".rsterm-{}.part", uuid::Uuid::new_v4()),
    ))
}
fn local_temp_path(destination: &Path) -> Result<PathBuf, SftpError> {
    if destination.file_name().is_none() {
        return Err(SftpError::Protocol("Destination must name a file".into()));
    }
    Ok(destination
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".rsterm-{}.part", uuid::Uuid::new_v4())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(mock: MockSftpSessionOps) -> SftpClient {
        SftpClient::with_session(Box::new(mock), "/home/user".into())
    }
    fn info(is_dir: bool, is_symlink: bool) -> FileMetadata {
        FileMetadata {
            is_dir,
            is_symlink,
            size: 4,
            modified: None,
            permissions: None,
        }
    }

    #[test]
    fn remote_home_expansion_is_local_and_immutable() {
        let client = client(MockSftpSessionOps::new());
        assert_eq!(client.resolve_path("~/docs").unwrap(), "/home/user/docs");
        assert_eq!(client.resolve_path("~").unwrap(), "/home/user");
        assert_eq!(client.resolve_path(".").unwrap(), "/home/user");
        assert!(client.resolve_path("~other").is_err());
        assert!(client.resolve_path("bad\0name").is_err());
    }

    #[tokio::test]
    async fn deleting_a_symlink_never_removes_its_target_directory() {
        let mut mock = MockSftpSessionOps::new();
        mock.expect_symlink_metadata()
            .with(mockall::predicate::eq("/home/user/link"))
            .returning(|_| Ok(info(true, true)));
        mock.expect_remove_file()
            .with(mockall::predicate::eq("/home/user/link"))
            .times(1)
            .returning(|_| Ok(()));
        mock.expect_remove_dir().never();
        client(mock).delete("link/").await.unwrap();
    }

    #[tokio::test]
    async fn requests_can_run_concurrently_without_shared_directory_state() {
        let gate = Arc::new(tokio::sync::Barrier::new(2));
        let mut mock = MockSftpSessionOps::new();
        mock.expect_read_dir().returning(|_| Ok(vec![]));
        let client = client(mock);
        let first = async {
            gate.wait().await;
            client.list_dir("/first").await.unwrap()
        };
        let second = async {
            gate.wait().await;
            client.list_dir("/second").await.unwrap()
        };
        tokio::join!(first, second);
        assert_eq!(client.pwd(), "/home/user");
    }

    #[tokio::test]
    async fn cancelled_upload_removes_only_its_exclusive_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        tokio::fs::write(&source, b"data").await.unwrap();
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        let mut mock = MockSftpSessionOps::new();
        mock.expect_create_exclusive()
            .withf(|path| path.starts_with("/home/user/.rsterm-") && path.ends_with(".part"))
            .returning(move |_| {
                let mut file = MockSftpFileWriteOps::new();
                let trigger = trigger.clone();
                file.expect_write_all().returning(move |_| {
                    trigger.cancel();
                    Ok(())
                });
                file.expect_close().times(1).returning(|| Ok(()));
                Ok(Box::new(file))
            });
        mock.expect_rename().never();
        mock.expect_remove_file()
            .withf(|path| path.starts_with("/home/user/.rsterm-") && path.ends_with(".part"))
            .times(1)
            .returning(|_| Ok(()));
        let result = client(mock)
            .upload(&source, "destination", true, &cancel, &|_, _| {})
            .await;
        assert!(matches!(result, Err(SftpError::Cancelled)));
    }

    #[tokio::test]
    async fn failed_exclusive_create_does_not_remove_someone_elses_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        tokio::fs::write(&source, b"data").await.unwrap();
        let mut mock = MockSftpSessionOps::new();
        mock.expect_create_exclusive().returning(|_| {
            Err(SftpError::AlreadyExists {
                path: "temporary".into(),
            })
        });
        mock.expect_remove_file().never();
        assert!(client(mock)
            .upload(
                &source,
                "destination",
                false,
                &CancellationToken::new(),
                &|_, _| {}
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn failed_download_close_preserves_destination() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("destination");
        tokio::fs::write(&destination, b"original").await.unwrap();
        let mut mock = MockSftpSessionOps::new();
        mock.expect_metadata().returning(|_| Ok(info(false, false)));
        mock.expect_open().returning(|_| {
            let mut file = MockSftpFileReadOps::new();
            file.expect_read().returning(|_| Ok(0));
            file.expect_close()
                .returning(|| Err(SftpError::Protocol("close failed".into())));
            Ok(Box::new(file))
        });
        assert!(client(mock)
            .download(
                "source",
                &destination,
                true,
                &CancellationToken::new(),
                &|_, _| {}
            )
            .await
            .is_err());
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"original");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn progress_updates_are_bounded_between_initial_and_final_notifications() {
        let notifications = std::sync::atomic::AtomicUsize::new(0);
        let callback = |_, _| {
            notifications.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        };
        let mut progress = Progress::new(&callback, 1000);
        for done in 1..1000 {
            progress.update(done);
        }
        progress.finish(1000);
        assert_eq!(notifications.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn failed_commit_never_deletes_existing_remote_destination() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        tokio::fs::write(&source, b"data").await.unwrap();
        let mut mock = MockSftpSessionOps::new();
        mock.expect_create_exclusive().returning(|_| {
            let mut file = MockSftpFileWriteOps::new();
            file.expect_write_all().returning(|_| Ok(()));
            file.expect_close().returning(|| Ok(()));
            Ok(Box::new(file))
        });
        mock.expect_rename()
            .withf(|_, to, overwrite| to == "/home/user/destination" && *overwrite)
            .times(1)
            .returning(|_, _, _| Err(SftpError::Unsupported("No POSIX rename".into())));
        mock.expect_remove_file()
            .withf(|path| path.contains("/.rsterm-"))
            .times(1)
            .returning(|_| Ok(()));
        assert!(client(mock)
            .upload(
                &source,
                "destination",
                true,
                &CancellationToken::new(),
                &|_, _| {}
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn download_no_clobber_preserves_destination_and_cleans_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("destination");
        tokio::fs::write(&destination, b"original").await.unwrap();
        let mut mock = MockSftpSessionOps::new();
        mock.expect_metadata().returning(|_| Ok(info(false, false)));
        mock.expect_open().returning(|_| {
            let mut file = MockSftpFileReadOps::new();
            file.expect_read().times(1).returning(|buf| {
                buf[..4].copy_from_slice(b"data");
                Ok(4)
            });
            file.expect_read().returning(|_| Ok(0));
            file.expect_close().returning(|| Ok(()));
            Ok(Box::new(file))
        });
        let result = client(mock)
            .download(
                "source",
                &destination,
                false,
                &CancellationToken::new(),
                &|_, _| {},
            )
            .await;
        assert!(matches!(result, Err(SftpError::AlreadyExists { .. })));
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"original");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn cancelled_download_preserves_destination_and_cleans_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("destination");
        tokio::fs::write(&destination, b"original").await.unwrap();
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        let mut mock = MockSftpSessionOps::new();
        mock.expect_metadata().returning(|_| Ok(info(false, false)));
        mock.expect_open().returning(move |_| {
            let mut file = MockSftpFileReadOps::new();
            let trigger = trigger.clone();
            file.expect_read().times(1).returning(move |buf| {
                buf[..4].copy_from_slice(b"data");
                trigger.cancel();
                Ok(4)
            });
            file.expect_close().times(1).returning(|| Ok(()));
            Ok(Box::new(file))
        });
        let result = client(mock)
            .download("source", &destination, true, &cancel, &|_, _| {})
            .await;
        assert!(matches!(result, Err(SftpError::Cancelled)));
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"original");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
