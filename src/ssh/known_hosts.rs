use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};

use parking_lot::Mutex;
use russh_keys::key::PublicKey;

#[derive(Clone, Debug)]
pub struct KnownHosts {
    path: PathBuf,
    access: Arc<Mutex<()>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostKeyVerification {
    Trusted,
    Changed { line: usize },
    Unknown,
}

#[derive(Debug, thiserror::Error)]
pub enum KnownHostsError {
    #[error("Known hosts file not found: {0}")]
    FileNotFound(PathBuf),
    #[error("Failed to parse known hosts: {0}")]
    ParseError(String),
    #[error("Could not determine home directory")]
    NoHomeDir,
    #[error("Host key changed at line {0}")]
    KeyChanged(usize),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("SSH key error: {0}")]
    RusshKeys(#[from] russh_keys::Error),
}

impl KnownHosts {
    pub fn load_default() -> Result<Self, KnownHostsError> {
        let home = dirs::home_dir().ok_or(KnownHostsError::NoHomeDir)?;
        let ssh_dir = home.join(".ssh");
        if !ssh_dir.exists() {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&ssh_dir)?;
        }
        let path = ssh_dir.join("known_hosts");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        Self::load_from(path)
    }

    pub fn load_from(path: PathBuf) -> Result<Self, KnownHostsError> {
        let path = path.canonicalize().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                KnownHostsError::FileNotFound(path)
            } else {
                KnownHostsError::Io(error)
            }
        })?;
        let access = file_access(&path);
        {
            let _guard = access.lock();
            validate_file(&path)?;
        }
        Ok(Self { path, access })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn verify(
        &self,
        host: &str,
        port: u16,
        key: &PublicKey,
    ) -> Result<HostKeyVerification, KnownHostsError> {
        let _guard = self.access.lock();
        self.verify_inner(host, port, key)
    }

    fn verify_inner(
        &self,
        host: &str,
        port: u16,
        key: &PublicKey,
    ) -> Result<HostKeyVerification, KnownHostsError> {
        validate_file(&self.path)?;
        let keys = russh_keys::known_host_keys_path(host, port, &self.path)?;
        if keys.iter().any(|(_, recorded)| recorded == key) {
            return Ok(HostKeyVerification::Trusted);
        }
        Ok(match keys.first() {
            Some((line, _)) => HostKeyVerification::Changed { line: *line },
            None => HostKeyVerification::Unknown,
        })
    }

    pub fn add(&self, host: &str, port: u16, key: &PublicKey) -> Result<(), KnownHostsError> {
        if host.chars().any(char::is_whitespace) || host.contains([',', '\n', '\r']) {
            return Err(KnownHostsError::ParseError("Invalid host name".into()));
        }
        let _guard = self.access.lock();
        match self.verify_inner(host, port, key)? {
            HostKeyVerification::Trusted => return Ok(()),
            HostKeyVerification::Changed { line } => return Err(KnownHostsError::KeyChanged(line)),
            HostKeyVerification::Unknown => {}
        }
        let mut record = Vec::new();
        if port == 22 {
            write!(record, "\n{host} ")?;
        } else {
            write!(record, "\n[{host}]:{port} ")?;
        }
        russh_keys::write_public_key_base64(&mut record, key)?;
        record.push(b'\n');
        let mut file = std::fs::OpenOptions::new().append(true).open(&self.path)?;
        file.write_all(&record)?;
        file.flush()?;
        file.sync_data()?;
        Ok(())
    }
}

fn file_access(path: &Path) -> Arc<Mutex<()>> {
    static FILE_ACCESS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    let mut files = FILE_ACCESS.get_or_init(Mutex::default).lock();
    files.retain(|_, access| access.strong_count() > 0);
    if let Some(access) = files.get(path).and_then(Weak::upgrade) {
        return access;
    }
    let access = Arc::new(Mutex::new(()));
    files.insert(path.to_path_buf(), Arc::downgrade(&access));
    access
}

fn validate_file(path: &Path) -> Result<(), KnownHostsError> {
    let contents = std::fs::read_to_string(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            KnownHostsError::FileNotFound(path.to_path_buf())
        } else {
            KnownHostsError::Io(error)
        }
    })?;
    for (index, line) in contents.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split(' ').filter(|field| !field.is_empty()).collect();
        let mut columns = line.split(' ');
        let canonical_columns = [columns.next(), columns.next(), columns.next()]
            .into_iter()
            .all(|column| column.is_some_and(|value| !value.is_empty()));
        if fields.len() < 3
            || !canonical_columns
            || line.contains('\t')
            || fields[0].starts_with('@')
            || fields[0].contains(['*', '?', '!'])
        {
            return Err(KnownHostsError::ParseError(format!(
                "Unsupported or malformed record at line {}",
                index + 1
            )));
        }
        let key = russh_keys::parse_public_key_base64(fields[2])
            .map_err(|error| KnownHostsError::ParseError(format!("Line {}: {error}", index + 1)))?;
        let rsa_algorithm = matches!(key, PublicKey::RSA { .. })
            && matches!(fields[1], "ssh-rsa" | "rsa-sha2-256" | "rsa-sha2-512");
        if fields[1] != key.name() && !rsa_algorithm {
            return Err(KnownHostsError::ParseError(format!(
                "Key algorithm mismatch at line {}",
                index + 1
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> PublicKey {
        russh_keys::key::KeyPair::generate_ed25519()
            .unwrap()
            .clone_public_key()
            .unwrap()
    }

    #[test]
    fn missing_or_corrupt_database_is_not_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        assert!(matches!(
            KnownHosts::load_from(path.clone()),
            Err(KnownHostsError::FileNotFound(_))
        ));
        std::fs::write(&path, "malformed\n").unwrap();
        assert!(matches!(
            KnownHosts::load_from(path.clone()),
            Err(KnownHostsError::ParseError(_))
        ));
        std::fs::write(&path, "").unwrap();
        let known = KnownHosts::load_from(path.clone()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(known.verify("host", 22, &key()).is_err());
    }

    #[test]
    fn independently_loaded_databases_serialize_conflicting_acceptance() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let first = KnownHosts::load_from(file.path().to_path_buf()).unwrap();
        let second = KnownHosts::load_from(file.path().to_path_buf()).unwrap();
        assert!(Arc::ptr_eq(&first.access, &second.access));
        let first_key = key();
        let second_key = key();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let accepting = |known: KnownHosts, key: PublicKey| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                assert_eq!(
                    known.verify("host", 2222, &key).unwrap(),
                    HostKeyVerification::Unknown
                );
                barrier.wait();
                known.add("host", 2222, &key)
            })
        };
        let first_acceptance = accepting(first.clone(), first_key.clone());
        let second_acceptance = accepting(second, second_key.clone());
        let first_result = first_acceptance.join().unwrap();
        let second_result = second_acceptance.join().unwrap();
        assert_ne!(first_result.is_ok(), second_result.is_ok());
        let (accepted, rejected, rejection) = if first_result.is_ok() {
            (first_key, second_key, second_result)
        } else {
            (second_key, first_key, first_result)
        };
        assert!(matches!(rejection, Err(KnownHostsError::KeyChanged(_))));
        assert_eq!(
            first.verify("host", 2222, &accepted).unwrap(),
            HostKeyVerification::Trusted
        );
        assert!(matches!(
            first.verify("host", 2222, &rejected).unwrap(),
            HostKeyVerification::Changed { .. }
        ));
    }

    #[test]
    fn explicit_acceptance_persists_and_mismatch_is_rejected() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let known = KnownHosts::load_from(file.path().to_path_buf()).unwrap();
        let first = key();
        assert_eq!(
            known.verify("host", 2222, &first).unwrap(),
            HostKeyVerification::Unknown
        );
        known.add("host", 2222, &first).unwrap();
        assert_eq!(
            known.verify("host", 2222, &first).unwrap(),
            HostKeyVerification::Trusted
        );
        assert!(matches!(
            known.verify("host", 2222, &key()).unwrap(),
            HostKeyVerification::Changed { .. }
        ));
        assert!(known.add("host", 2222, &key()).is_err());
    }
}
