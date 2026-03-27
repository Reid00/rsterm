//! Integration tests for SFTP functionality
//!
//! These tests require a Docker SFTP server running on port 2222.
//! To start the test server:
//!   docker run -d --name sftp-test -p 2222:22 atmoz/sftp testuser:testpass:::upload

use std::net::TcpStream;
use std::time::Duration;

use rsterm::backend::{SshAuthMethod, SshConnectionConfig, SshPty};
use rsterm::backend::PtyBackend;
use tempfile::tempdir;

/// Check if the SFTP test server is available
fn sftp_available() -> bool {
    TcpStream::connect_timeout(
        &"127.0.0.1:2222".parse().unwrap(),
        Duration::from_secs(2),
    )
    .is_ok()
}

/// Create an SSH connection to the test SFTP server
async fn connect_to_test_server() -> Option<SshPty> {
    let (output_tx, _output_rx) = tokio::sync::mpsc::channel(64);

    let config = SshConnectionConfig {
        host: "127.0.0.1".to_string(),
        port: 2222,
        username: "testuser".to_string(),
        auth: SshAuthMethod::Password("testpass".to_string()),
        known_hosts: None, // Skip host key verification for tests
    };

    SshPty::new_async(&config, 80, 24, output_tx)
        .await
        .ok()
}

/// Test listing the home directory and verifying the upload folder exists
#[tokio::test]
#[ignore = "requires Docker SFTP server on port 2222"]
async fn test_sftp_list_home() {
    if !sftp_available() {
        eprintln!("Skipping: SFTP server not available");
        return;
    }

    let ssh = match connect_to_test_server().await {
        Some(ssh) => ssh,
        None => {
            eprintln!("Skipping: Failed to connect to SSH server");
            return;
        }
    };

    // Get SFTP client
    let sftp = match ssh.get_sftp().await {
        Some(sftp) => sftp,
        None => {
            eprintln!("Skipping: SFTP not available on connection");
            return;
        }
    };

    let mut sftp = sftp.lock().await;

    // List home directory
    let entries = match sftp.list_dir(".").await {
        Ok(entries) => entries,
        Err(e) => {
            panic!("Failed to list home directory: {:?}", e);
        }
    };

    // Verify upload folder exists (atmoz/sftp creates this by default)
    let upload_exists = entries.iter().any(|e| e.name == "upload" && e.is_dir);
    assert!(
        upload_exists,
        "Expected 'upload' directory to exist in home folder"
    );
}

/// Test creating a directory, verifying it exists, deleting it, and verifying it's gone
#[tokio::test]
#[ignore = "requires Docker SFTP server on port 2222"]
async fn test_sftp_mkdir_and_delete() {
    if !sftp_available() {
        eprintln!("Skipping: SFTP server not available");
        return;
    }

    let ssh = match connect_to_test_server().await {
        Some(ssh) => ssh,
        None => {
            eprintln!("Skipping: Failed to connect to SSH server");
            return;
        }
    };

    let sftp = match ssh.get_sftp().await {
        Some(sftp) => sftp,
        None => {
            eprintln!("Skipping: SFTP not available on connection");
            return;
        }
    };

    let mut sftp = sftp.lock().await;

    let test_dir = "test_mkdir_dir";

    // Create directory
    sftp.mkdir(test_dir)
        .await
        .expect("Failed to create directory");

    // Verify it exists
    let entries = sftp.list_dir(".").await.expect("Failed to list directory");
    let dir_exists = entries.iter().any(|e| e.name == test_dir && e.is_dir);
    assert!(dir_exists, "Directory should exist after creation");

    // Delete the directory
    sftp.delete(test_dir)
        .await
        .expect("Failed to delete directory");

    // Verify it's gone
    let entries = sftp.list_dir(".").await.expect("Failed to list directory");
    let dir_exists = entries.iter().any(|e| e.name == test_dir);
    assert!(!dir_exists, "Directory should not exist after deletion");
}

/// Test uploading a file, downloading it, and comparing content
#[tokio::test]
#[ignore = "requires Docker SFTP server on port 2222"]
async fn test_sftp_upload_and_download() {
    if !sftp_available() {
        eprintln!("Skipping: SFTP server not available");
        return;
    }

    let ssh = match connect_to_test_server().await {
        Some(ssh) => ssh,
        None => {
            eprintln!("Skipping: Failed to connect to SSH server");
            return;
        }
    };

    let sftp = match ssh.get_sftp().await {
        Some(sftp) => sftp,
        None => {
            eprintln!("Skipping: SFTP not available on connection");
            return;
        }
    };

    let mut sftp = sftp.lock().await;

    // Create temp directory for test files
    let temp_dir = tempdir().expect("Failed to create temp directory");

    let test_content = b"This is test content for SFTP upload/download test.\nLine 2 of test content.\n";

    // Create local file to upload
    let local_upload_path = temp_dir.path().join("upload_test.txt");
    tokio::fs::write(&local_upload_path, test_content)
        .await
        .expect("Failed to write local test file");

    // Upload to upload directory (writable in atmoz/sftp)
    let remote_path = "upload/test_upload_file.txt";

    sftp.upload(local_upload_path.to_str().unwrap(), remote_path)
        .await
        .expect("Failed to upload file");

    // Download to a different local path
    let local_download_path = temp_dir.path().join("download_test.txt");
    sftp.download(remote_path, local_download_path.to_str().unwrap())
        .await
        .expect("Failed to download file");

    // Compare content
    let downloaded_content = tokio::fs::read(&local_download_path)
        .await
        .expect("Failed to read downloaded file");

    assert_eq!(
        test_content.as_slice(),
        downloaded_content.as_slice(),
        "Downloaded content should match uploaded content"
    );

    // Cleanup remote file
    sftp.delete(remote_path)
        .await
        .expect("Failed to cleanup remote file");
}

/// Test navigating to the upload directory and listing contents
#[tokio::test]
#[ignore = "requires Docker SFTP server on port 2222"]
async fn test_sftp_navigate_directories() {
    if !sftp_available() {
        eprintln!("Skipping: SFTP server not available");
        return;
    }

    let ssh = match connect_to_test_server().await {
        Some(ssh) => ssh,
        None => {
            eprintln!("Skipping: Failed to connect to SSH server");
            return;
        }
    };

    let sftp = match ssh.get_sftp().await {
        Some(sftp) => sftp,
        None => {
            eprintln!("Skipping: SFTP not available on connection");
            return;
        }
    };

    let mut sftp = sftp.lock().await;

    // Change to upload directory
    let new_path = sftp.cd("upload").await.expect("Failed to change to upload directory");
    assert!(
        new_path.ends_with("upload"),
        "New path should end with 'upload', got: {}",
        new_path
    );

    // Verify pwd reflects the change
    let pwd = sftp.pwd();
    assert!(
        pwd.ends_with("upload"),
        "pwd() should return path ending with 'upload', got: {}",
        pwd
    );

    // List contents (may be empty or have files from other tests)
    let entries = sftp.list_dir(".").await.expect("Failed to list upload directory");
    // Just verify we can list - content depends on server state
    println!("Upload directory has {} entries", entries.len());

    // Change back to home
    let home_path = sftp.cd("..").await.expect("Failed to change to parent directory");
    assert!(
        !home_path.ends_with("upload"),
        "Should be back in home directory, got: {}",
        home_path
    );
}

/// Test attempting to download a non-existent file returns an error
#[tokio::test]
#[ignore = "requires Docker SFTP server on port 2222"]
async fn test_sftp_file_not_found_error() {
    if !sftp_available() {
        eprintln!("Skipping: SFTP server not available");
        return;
    }

    let ssh = match connect_to_test_server().await {
        Some(ssh) => ssh,
        None => {
            eprintln!("Skipping: Failed to connect to SSH server");
            return;
        }
    };

    let sftp = match ssh.get_sftp().await {
        Some(sftp) => sftp,
        None => {
            eprintln!("Skipping: SFTP not available on connection");
            return;
        }
    };

    let mut sftp = sftp.lock().await;

    // Create temp directory for download target
    let temp_dir = tempdir().expect("Failed to create temp directory");
    let local_path = temp_dir.path().join("nonexistent_download.txt");

    // Try to download a non-existent file
    let result = sftp.download("nonexistent_file_12345.txt", local_path.to_str().unwrap()).await;

    assert!(
        result.is_err(),
        "Downloading non-existent file should return an error"
    );

    let error = result.unwrap_err();
    assert!(
        matches!(error, rsterm::error::SftpError::FileNotFound { .. }),
        "Error should be FileNotFound, got: {:?}",
        error
    );
}