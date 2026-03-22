// src/ssh/known_hosts.rs
//! Known hosts management for SSH connections
//!
//! Provides host key verification using the SSH known_hosts file.
//! Implements TOFU (Trust On First Use) for unknown hosts.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use russh_keys::key::PublicKey;
use tracing::{debug, info, warn};

/// Known hosts database for SSH host key verification
///
/// This struct wraps the russh-keys known hosts functionality and provides
/// a higher-level API for verifying, adding, and managing known host keys.
#[derive(Debug)]
pub struct KnownHosts {
    /// Path to the known_hosts file
    path: PathBuf,
    /// In-memory cache of recently added keys (for sessions where we accepted unknown hosts)
    pending_additions: Arc<RwLock<Vec<PendingEntry>>>,
}

/// A pending entry to be written to known_hosts
#[derive(Debug, Clone)]
struct PendingEntry {
    host: String,
    port: u16,
    key: PublicKey,
}

/// Result of host key verification
#[derive(Debug, Clone)]
pub enum HostKeyVerification {
    /// Key matches known host - connection is trusted
    Trusted,
    /// Key is different from known host - potential MITM attack
    Changed {
        /// The line number in known_hosts where the old key was found
        line: usize,
    },
    /// Host is unknown - first connection to this host
    Unknown,
}

/// Errors that can occur during known hosts operations
#[derive(Debug, thiserror::Error)]
pub enum KnownHostsError {
    /// The known_hosts file could not be found
    #[error("Known hosts file not found: {0}")]
    FileNotFound(PathBuf),

    /// Failed to parse the known_hosts file
    #[error("Failed to parse known hosts: {0}")]
    ParseError(String),

    /// Failed to write to the known_hosts file
    #[error("Failed to write known hosts: {0}")]
    WriteError(String),

    /// Home directory could not be determined
    #[error("Could not determine home directory")]
    NoHomeDir,

    /// I/O error
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// russh-keys error
    #[error("SSH key error: {0}")]
    RusshKeys(#[from] russh_keys::Error),
}

impl KnownHosts {
    /// Load known hosts from the default location (~/.ssh/known_hosts)
    ///
    /// Creates the ~/.ssh directory if it doesn't exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the home directory cannot be determined or
    /// if there's an I/O error accessing the known_hosts file.
    pub fn load_default() -> Result<Self, KnownHostsError> {
        let home = dirs::home_dir().ok_or(KnownHostsError::NoHomeDir)?;
        let ssh_dir = home.join(".ssh");
        let path = ssh_dir.join("known_hosts");

        // Ensure ~/.ssh directory exists
        if !ssh_dir.exists() {
            std::fs::create_dir_all(&ssh_dir)?;
            debug!("Created .ssh directory at {:?}", ssh_dir);
        }

        // Create empty known_hosts file if it doesn't exist
        if !path.exists() {
            std::fs::File::create(&path)?;
            debug!("Created empty known_hosts file at {:?}", path);
        }

        Ok(Self {
            path,
            pending_additions: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Load known hosts from a specific file path
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the known_hosts file
    ///
    /// # Errors
    ///
    /// Returns an error if the file doesn't exist or cannot be read.
    pub fn load_from(path: PathBuf) -> Result<Self, KnownHostsError> {
        if !path.exists() {
            return Err(KnownHostsError::FileNotFound(path));
        }

        Ok(Self {
            path,
            pending_additions: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Get the path to the known_hosts file
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Verify a host key against the known hosts database
    ///
    /// # Arguments
    ///
    /// * `host` - The hostname or IP address
    /// * `port` - The port number (typically 22 for SSH)
    /// * `key` - The public key presented by the server
    ///
    /// # Returns
    ///
    /// - `HostKeyVerification::Trusted` if the key matches
    /// - `HostKeyVerification::Changed` if the key differs from the known key
    /// - `HostKeyVerification::Unknown` if the host is not in known_hosts
    pub fn verify(&self, host: &str, port: u16, key: &PublicKey) -> HostKeyVerification {
        debug!("Verifying host key for {}:{}", host, port);

        // First check pending additions (keys we've accepted this session)
        if let Ok(pending) = self.pending_additions.read() {
            for entry in pending.iter() {
                if entry.host == host && entry.port == port {
                    // We already accepted this key in this session
                    if &entry.key == key {
                        debug!("Key matched pending addition for {}:{}", host, port);
                        return HostKeyVerification::Trusted;
                    } else {
                        // Key changed within the same session - shouldn't happen normally
                        warn!("Key changed for {}:{} within the same session", host, port);
                        return HostKeyVerification::Changed { line: 0 };
                    }
                }
            }
        }

        // Check against the known_hosts file
        match russh_keys::check_known_hosts_path(host, port, key, &self.path) {
            Ok(true) => {
                info!("Host key verified for {}:{}", host, port);
                HostKeyVerification::Trusted
            }
            Ok(false) => {
                // Host not in known_hosts or key doesn't match
                // Check if the host exists with a different key
                match self.get_known_key(host, port) {
                    Some((line, _known_key)) => {
                        warn!("Host key changed for {}:{} at line {}", host, port, line);
                        HostKeyVerification::Changed { line }
                    }
                    None => {
                        info!("Unknown host {}:{}", host, port);
                        HostKeyVerification::Unknown
                    }
                }
            }
            Err(russh_keys::Error::KeyChanged { line }) => {
                warn!("Host key changed for {}:{} at line {}", host, port, line);
                HostKeyVerification::Changed { line }
            }
            Err(e) => {
                // For other errors (file not found, etc.), treat as unknown
                debug!("Error checking known hosts for {}:{}: {}", host, port, e);
                HostKeyVerification::Unknown
            }
        }
    }

    /// Get the known public key for a host
    ///
    /// # Arguments
    ///
    /// * `host` - The hostname or IP address
    /// * `port` - The port number
    ///
    /// # Returns
    ///
    /// The first matching public key (with its line number), or None if the host is not known.
    fn get_known_key(&self, host: &str, port: u16) -> Option<(usize, PublicKey)> {
        russh_keys::known_host_keys_path(host, port, &self.path)
            .ok()
            .and_then(|keys| keys.into_iter().next())
    }

    /// Add a new host key to the in-memory pending list
    ///
    /// The key will be added to the known_hosts file when `save()` is called.
    /// This allows accepting unknown hosts during a session without immediately
    /// writing to disk.
    ///
    /// # Arguments
    ///
    /// * `host` - The hostname or IP address
    /// * `port` - The port number
    /// * `key` - The public key to add
    pub fn add_pending(&self, host: &str, port: u16, key: PublicKey) {
        if let Ok(mut pending) = self.pending_additions.write() {
            // Check if we already have this entry
            let exists = pending.iter().any(|e| e.host == host && e.port == port);
            if !exists {
                pending.push(PendingEntry {
                    host: host.to_string(),
                    port,
                    key,
                });
                debug!("Added pending known_hosts entry for {}:{}", host, port);
            }
        }
    }

    /// Add a new host key directly to the known_hosts file
    ///
    /// # Arguments
    ///
    /// * `host` - The hostname or IP address
    /// * `port` - The port number
    /// * `key` - The public key to add
    ///
    /// # Errors
    ///
    /// Returns an error if the key cannot be written to the file.
    pub fn add(&self, host: &str, port: u16, key: &PublicKey) -> Result<(), KnownHostsError> {
        info!("Adding host key for {}:{}", host, port);

        russh_keys::learn_known_hosts_path(host, port, key, &self.path)?;

        // Also remove from pending if it was there
        if let Ok(mut pending) = self.pending_additions.write() {
            pending.retain(|e| !(e.host == host && e.port == port));
        }

        Ok(())
    }

    /// Save all pending additions to the known_hosts file
    ///
    /// This writes any keys added via `add_pending()` to the known_hosts file.
    ///
    /// # Errors
    ///
    /// Returns an error if any key cannot be written to the file.
    pub fn save(&self) -> Result<(), KnownHostsError> {
        let pending: Vec<PendingEntry> = if let Ok(p) = self.pending_additions.read() {
            p.clone()
        } else {
            return Ok(());
        };

        for entry in pending {
            self.add(&entry.host, entry.port, &entry.key)?;
        }

        Ok(())
    }

    /// Check if there are any pending additions
    pub fn has_pending(&self) -> bool {
        self.pending_additions
            .read()
            .map(|p| !p.is_empty())
            .unwrap_or(false)
    }
}

impl Clone for KnownHosts {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            pending_additions: self.pending_additions.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_from_nonexistent_file() {
        let result = KnownHosts::load_from(PathBuf::from("/nonexistent/known_hosts"));
        assert!(matches!(result, Err(KnownHostsError::FileNotFound(_))));
    }

    #[test]
    fn test_host_key_verification_debug() {
        let trusted = HostKeyVerification::Trusted;
        assert!(format!("{:?}", trusted).contains("Trusted"));

        let unknown = HostKeyVerification::Unknown;
        assert!(format!("{:?}", unknown).contains("Unknown"));

        let changed = HostKeyVerification::Changed { line: 42 };
        assert!(format!("{:?}", changed).contains("Changed"));
    }

    #[test]
    fn test_known_hosts_error_display() {
        let err = KnownHostsError::FileNotFound(PathBuf::from("/test/path"));
        assert!(err.to_string().contains("not found"));

        let err = KnownHostsError::ParseError("test error".to_string());
        assert!(err.to_string().contains("test error"));

        let err = KnownHostsError::NoHomeDir;
        assert!(err.to_string().contains("home directory"));
    }

    #[test]
    fn test_pending_additions_empty() {
        // Create a temporary file for testing
        let temp_dir = std::env::temp_dir();
        let test_path = temp_dir.join("rsterm_test_known_hosts_empty");

        // Ensure the file exists
        let _ = std::fs::File::create(&test_path);

        let known_hosts = KnownHosts::load_from(test_path.clone()).unwrap();

        // Should have no pending additions initially
        assert!(!known_hosts.has_pending());
    }
}