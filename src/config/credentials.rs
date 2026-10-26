use anyhow::{anyhow, bail, Result};

#[derive(Debug, Clone, Copy, Default)]
pub struct CredentialStore;

impl CredentialStore {
    pub fn get(&self, profile_id: &str) -> Result<Option<String>> {
        match Self::entry(profile_id)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(Self::error("read", error)),
        }
    }

    pub fn set(&self, profile_id: &str, secret: &str) -> Result<()> {
        Self::entry(profile_id)?
            .set_password(secret)
            .map_err(|error| Self::error("save", error))
    }

    pub fn delete(&self, profile_id: &str) -> Result<()> {
        match Self::entry(profile_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(Self::error("delete", error)),
        }
    }

    fn entry(profile_id: &str) -> Result<keyring::Entry> {
        if profile_id.trim().is_empty() {
            bail!("A saved profile ID is required for credential storage");
        }
        if !cfg!(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd",
            target_os = "openbsd"
        )) {
            bail!("System credential storage is not supported on this platform");
        }
        keyring::Entry::new("rsterm", profile_id).map_err(|error| Self::error("open", error))
    }

    fn error(operation: &str, error: keyring::Error) -> anyhow::Error {
        let reason = match error {
            keyring::Error::NoStorageAccess(_) => {
                "the credential store is locked or access was denied"
            }
            keyring::Error::PlatformFailure(_) => "the platform credential store failed",
            keyring::Error::NoEntry => "the credential does not exist",
            keyring::Error::BadEncoding(_) => "the stored credential is not valid UTF-8",
            keyring::Error::TooLong(_, _) => "a credential exceeds the platform size limit",
            keyring::Error::Invalid(_, _) => "the platform rejected the credential attributes",
            keyring::Error::Ambiguous(_) => "multiple matching credentials exist",
            _ => "an unexpected credential store error occurred",
        };
        anyhow!("Could not {operation} the system credential: {reason}")
    }
}
