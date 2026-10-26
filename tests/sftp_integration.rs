use std::time::Duration;

use rsterm::backend::{BackendEvent, SshAuthMethod, SshConnectionConfig, SshTransport};
use rsterm::sftp::SftpClient;
use rsterm::ssh::KnownHosts;
use tempfile::{tempdir, TempDir};
use tokio_util::sync::CancellationToken;

struct TestConnection {
    transport: SshTransport,
    sftp: SftpClient,
    _known_hosts: TempDir,
}

async fn connect_to_test_server() -> TestConnection {
    let known_hosts = tempdir().unwrap();
    let path = known_hosts.path().join("known_hosts");
    std::fs::write(&path, "").unwrap();
    let (events, mut event_rx) = tokio::sync::mpsc::channel(8);
    let responder = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            if let BackendEvent::HostKeyPrompt {
                host,
                port,
                respond_to,
                ..
            } = event
            {
                assert_eq!(host, "127.0.0.1");
                assert_eq!(port, 2222);
                respond_to.send(true).unwrap();
            }
        }
    });
    let config = SshConnectionConfig {
        host: "127.0.0.1".into(),
        port: 2222,
        username: "testuser".into(),
        auth: SshAuthMethod::Password("testpass".into()),
        known_hosts: KnownHosts::load_from(path).unwrap(),
        timeout: Duration::from_secs(10),
        keepalive_interval: Duration::from_secs(15),
        keepalive_max: 3,
    };
    let transport = SshTransport::connect(
        &config,
        "integration-test".into(),
        events,
        CancellationToken::new(),
    )
    .await
    .expect(
        "An isolated SFTP server must be running at 127.0.0.1:2222 when ignored tests are opted in",
    );
    let sftp = transport
        .sftp()
        .await
        .expect("Server must support SFTP independently of an interactive shell");
    responder.abort();
    TestConnection {
        transport,
        sftp,
        _known_hosts: known_hosts,
    }
}

#[tokio::test]
#[ignore = "requires isolated SFTP server on port 2222"]
async fn sftp_only_connection_lists_home_without_starting_shell() {
    let connection = connect_to_test_server().await;
    let entries = connection.sftp.list_dir("~").await.unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.name == "upload" && entry.is_dir));
    connection.transport.disconnect().await;
}

#[tokio::test]
#[ignore = "requires isolated SFTP server on port 2222"]
async fn sftp_mkdir_rename_chmod_and_empty_directory_delete() {
    let connection = connect_to_test_server().await;
    let first = format!("upload/rsterm-{}", uuid::Uuid::new_v4());
    let second = format!("{first}-renamed");
    connection.sftp.mkdir(&first).await.unwrap();
    connection.sftp.chmod(&first, 0o700).await.unwrap();
    connection.sftp.rename(&first, &second).await.unwrap();
    let entries = connection.sftp.list_dir("~/upload").await.unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.path.ends_with(&second) && entry.is_dir));
    connection.sftp.delete(&second).await.unwrap();
    connection.transport.disconnect().await;
}

#[tokio::test]
#[ignore = "requires isolated SFTP server on port 2222"]
async fn sftp_upload_download_and_no_clobber() {
    let connection = connect_to_test_server().await;
    let directory = tempdir().unwrap();
    let upload = directory.path().join("upload");
    let download = directory.path().join("download");
    let content = vec![0x5a; 256 * 1024 + 17];
    tokio::fs::write(&upload, &content).await.unwrap();
    let remote = format!("upload/rsterm-{}", uuid::Uuid::new_v4());
    let cancel = CancellationToken::new();
    connection
        .sftp
        .upload(&upload, &remote, false, &cancel, &|_, _| {})
        .await
        .unwrap();
    connection
        .sftp
        .download(&remote, &download, false, &cancel, &|_, _| {})
        .await
        .unwrap();
    assert_eq!(tokio::fs::read(&download).await.unwrap(), content);
    tokio::fs::write(&upload, b"replacement").await.unwrap();
    assert!(connection
        .sftp
        .upload(&upload, &remote, false, &cancel, &|_, _| {})
        .await
        .is_err());
    connection
        .sftp
        .download(&remote, &download, true, &cancel, &|_, _| {})
        .await
        .unwrap();
    assert_eq!(tokio::fs::read(&download).await.unwrap(), content);
    connection.sftp.delete(&remote).await.unwrap();
    connection.transport.disconnect().await;
}

#[tokio::test]
#[ignore = "requires isolated SFTP server on port 2222"]
async fn failed_download_preserves_existing_local_destination() {
    let connection = connect_to_test_server().await;
    let directory = tempdir().unwrap();
    let destination = directory.path().join("destination");
    tokio::fs::write(&destination, b"original").await.unwrap();
    let missing = format!("upload/missing-{}", uuid::Uuid::new_v4());
    assert!(connection
        .sftp
        .download(
            &missing,
            &destination,
            true,
            &CancellationToken::new(),
            &|_, _| {}
        )
        .await
        .is_err());
    assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"original");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    connection.transport.disconnect().await;
}

#[tokio::test]
#[ignore = "requires isolated SFTP server on port 2222"]
async fn zero_byte_files_report_completion_and_round_trip() {
    let connection = connect_to_test_server().await;
    let directory = tempdir().unwrap();
    let source = directory.path().join("empty");
    let destination = directory.path().join("download");
    tokio::fs::write(&source, b"").await.unwrap();
    let remote = format!("upload/empty-{}", uuid::Uuid::new_v4());
    let progress = std::sync::Mutex::new(Vec::new());
    connection
        .sftp
        .upload(
            &source,
            &remote,
            false,
            &CancellationToken::new(),
            &|done, total| {
                progress.lock().unwrap().push((done, total));
            },
        )
        .await
        .unwrap();
    assert_eq!(*progress.lock().unwrap(), vec![(0, 0), (0, 0)]);
    progress.lock().unwrap().clear();
    connection
        .sftp
        .download(
            &remote,
            &destination,
            false,
            &CancellationToken::new(),
            &|done, total| {
                progress.lock().unwrap().push((done, total));
            },
        )
        .await
        .unwrap();
    assert_eq!(*progress.lock().unwrap(), vec![(0, 0), (0, 0)]);
    assert!(tokio::fs::read(&destination).await.unwrap().is_empty());
    connection.sftp.delete(&remote).await.unwrap();
    connection.transport.disconnect().await;
}

#[tokio::test]
#[ignore = "requires isolated SFTP server on port 2222"]
async fn cancelled_transfers_preserve_destinations_and_remove_staging_files() {
    let connection = connect_to_test_server().await;
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let destination = directory.path().join("destination");
    let remote_directory = format!("upload/cancel-{}", uuid::Uuid::new_v4());
    let remote = format!("{remote_directory}/destination");
    connection.sftp.mkdir(&remote_directory).await.unwrap();
    tokio::fs::write(&source, b"remote-original").await.unwrap();
    connection
        .sftp
        .upload(
            &source,
            &remote,
            false,
            &CancellationToken::new(),
            &|_, _| {},
        )
        .await
        .unwrap();
    tokio::fs::write(&source, vec![0x51; 512 * 1024])
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let result = connection
        .sftp
        .upload(&source, &remote, true, &cancel, &|_, _| cancel.cancel())
        .await;
    assert!(matches!(result, Err(rsterm::error::SftpError::Cancelled)));
    let entries = connection.sftp.list_dir(&remote_directory).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "destination");
    connection
        .sftp
        .download(
            &remote,
            &destination,
            false,
            &CancellationToken::new(),
            &|_, _| {},
        )
        .await
        .unwrap();
    assert_eq!(
        tokio::fs::read(&destination).await.unwrap(),
        b"remote-original"
    );
    tokio::fs::write(&destination, b"local-original")
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let result = connection
        .sftp
        .download(&remote, &destination, true, &cancel, &|_, _| {
            cancel.cancel()
        })
        .await;
    assert!(matches!(result, Err(rsterm::error::SftpError::Cancelled)));
    assert_eq!(
        tokio::fs::read(&destination).await.unwrap(),
        b"local-original"
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    connection.sftp.delete(&remote).await.unwrap();
    connection.sftp.delete(&remote_directory).await.unwrap();
    connection.transport.disconnect().await;
}

#[tokio::test]
#[ignore = "requires isolated SFTP server with posix-rename on port 2222"]
async fn upload_overwrite_publishes_complete_content_without_staging_files() {
    let connection = connect_to_test_server().await;
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    let destination = directory.path().join("destination");
    let remote_directory = format!("upload/overwrite-{}", uuid::Uuid::new_v4());
    let remote = format!("{remote_directory}/destination");
    connection.sftp.mkdir(&remote_directory).await.unwrap();
    tokio::fs::write(&source, b"original").await.unwrap();
    connection
        .sftp
        .upload(
            &source,
            &remote,
            false,
            &CancellationToken::new(),
            &|_, _| {},
        )
        .await
        .unwrap();
    let replacement = vec![0x6b; 1024 * 1024 + 31];
    tokio::fs::write(&source, &replacement).await.unwrap();
    connection
        .sftp
        .upload(
            &source,
            &remote,
            true,
            &CancellationToken::new(),
            &|_, _| {},
        )
        .await
        .unwrap();
    let entries = connection.sftp.list_dir(&remote_directory).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "destination");
    assert_eq!(entries[0].size, replacement.len() as u64);
    connection
        .sftp
        .download(
            &remote,
            &destination,
            false,
            &CancellationToken::new(),
            &|_, _| {},
        )
        .await
        .unwrap();
    assert_eq!(tokio::fs::read(&destination).await.unwrap(), replacement);
    connection.sftp.delete(&remote).await.unwrap();
    connection.sftp.delete(&remote_directory).await.unwrap();
    connection.transport.disconnect().await;
}
