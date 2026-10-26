use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::de::DeserializeOwned;

use crate::config::{
    AppSettings, AuthConfig, CredentialStore, HostLibrary, HostNodeId, SessionConfig, Theme,
};
use crate::error::ConfigError;

#[derive(Clone)]
pub struct ConfigManager {
    base_dir: PathBuf,
    credential_access: Arc<Mutex<()>>,
}

impl ConfigManager {
    pub fn new() -> Result<Self, ConfigError> {
        let base_dir = match std::env::var_os("RSTERM_CONFIG_DIR") {
            Some(path) => PathBuf::from(path),
            None => dirs::config_dir()
                .map(|path| path.join("rsterm"))
                .ok_or(ConfigError::NoPlatformConfigDir)?,
        };
        Self::from_dir(base_dir)
    }

    pub fn from_dir(base_dir: PathBuf) -> Result<Self, ConfigError> {
        Self::create_private_dir(&base_dir)?;
        Self::create_private_dir(&base_dir.join("themes"))?;
        let manager = Self {
            base_dir,
            credential_access: Arc::new(Mutex::new(())),
        };
        for name in ["config.toml", "sessions.toml"] {
            Self::protect_existing_file(&manager.base_dir.join(name))?;
        }
        let path = manager.base_dir.join("config.toml");
        if !path.try_exists()? {
            let content = toml::to_string_pretty(&AppSettings::default())?;
            manager.write_atomic(&path, &content, false)?;
        }
        Ok(manager)
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    pub fn keybindings_path(&self) -> PathBuf {
        self.base_dir
            .canonicalize()
            .unwrap_or_else(|_| self.base_dir.clone())
            .join("keybindings.toml")
    }

    pub fn create_keybindings_if_missing(&self) -> Result<(), ConfigError> {
        let path = self.keybindings_path();
        if !path.try_exists()? {
            self.write_atomic(&path, &super::default_keybindings_file(), false)?;
        }
        Ok(())
    }

    pub fn load_keybindings(&self) -> super::ShortcutConfig {
        super::ShortcutConfig::load(self)
    }

    pub fn load_settings(&self) -> Result<AppSettings, ConfigError> {
        Ok(Self::read_toml(&self.base_dir.join("config.toml"))?.unwrap_or_default())
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), ConfigError> {
        let content = toml::to_string_pretty(settings)?;
        self.write_atomic(&self.base_dir.join("config.toml"), &content, true)
    }

    pub fn load_library(&self) -> Result<HostLibrary, ConfigError> {
        let _access = self.credential_access.lock();
        self.read_library()
    }

    fn read_library(&self) -> Result<HostLibrary, ConfigError> {
        Ok(Self::read_toml(&self.base_dir.join("sessions.toml"))?.unwrap_or_default())
    }

    pub fn load_sessions(&self) -> Result<Vec<SessionConfig>, ConfigError> {
        Ok(self.load_library()?.sessions)
    }

    pub fn save_session(&self, session: &SessionConfig) -> Result<(), ConfigError> {
        let _access = self.credential_access.lock();
        self.save_session_locked(session)
    }

    fn save_session_locked(&self, session: &SessionConfig) -> Result<(), ConfigError> {
        let mut library = self.read_library()?;
        self.require_resolved_credentials(&library.sessions)?;
        let mut session = session.clone();
        Self::clear_secret(&mut session.auth);
        if let Some(existing) = library
            .sessions
            .iter_mut()
            .find(|saved| saved.id == session.id)
        {
            session.import_origin = existing.import_origin.clone();
            *existing = session;
        } else {
            library.sessions.push(session);
        }
        self.write_library(&library)
    }

    pub fn delete_session(&self, id: &str) -> Result<(), ConfigError> {
        let _access = self.credential_access.lock();
        self.delete_session_locked(id)
    }

    fn delete_session_locked(&self, id: &str) -> Result<(), ConfigError> {
        let mut library = self.read_library()?;
        self.require_resolved_credentials(&library.sessions)?;
        let initial_len = library.sessions.len();
        library.sessions.retain(|session| session.id != id);
        if library.sessions.len() == initial_len {
            return Ok(());
        }
        self.write_library(&library)
    }

    fn change_library(
        &self,
        change: impl FnOnce(&mut HostLibrary) -> Result<(), String>,
    ) -> Result<HostLibrary, ConfigError> {
        let _access = self.credential_access.lock();
        let mut library = self.read_library()?;
        self.require_resolved_credentials(&library.sessions)?;
        change(&mut library).map_err(|reason| self.session_error(reason))?;
        self.write_library(&library)?;
        Ok(library)
    }

    pub fn create_folder(
        &self,
        name: &str,
        parent_id: Option<&str>,
    ) -> Result<HostLibrary, ConfigError> {
        self.change_library(|library| {
            library.create_folder(name.into(), parent_id.map(str::to_owned))?;
            Ok(())
        })
    }

    pub fn rename_folder(&self, id: &str, name: &str) -> Result<HostLibrary, ConfigError> {
        self.change_library(|library| library.rename_folder(id, name.into()))
    }

    pub fn move_node(
        &self,
        node: &HostNodeId,
        target: Option<&str>,
    ) -> Result<HostLibrary, ConfigError> {
        self.change_library(|library| library.move_node(node, target.map(str::to_owned)))
    }

    pub fn delete_empty_folder(&self, id: &str) -> Result<HostLibrary, ConfigError> {
        self.change_library(|library| library.delete_empty_folder(id))
    }

    pub fn import_sessions(&self, sessions: &[SessionConfig]) -> Result<HostLibrary, ConfigError> {
        self.change_library(|library| {
            let mut origins: HashSet<_> = library
                .sessions
                .iter()
                .filter_map(|session| session.import_origin.clone())
                .collect();
            for session in sessions {
                let origin = session
                    .import_origin
                    .as_ref()
                    .ok_or_else(|| "Imported hosts require a source path and alias".to_owned())?;
                if origin.source_path.as_os_str().is_empty() || origin.alias.trim().is_empty() {
                    return Err("Imported hosts require a source path and alias".into());
                }
                if session.name.trim().is_empty()
                    || session.host.trim().is_empty()
                    || session.host.chars().any(char::is_whitespace)
                    || session.username.trim().is_empty()
                    || session.port == 0
                    || matches!(&session.auth, AuthConfig::Key { key_path, .. } if key_path.trim().is_empty())
                {
                    return Err("Imported host connection settings are incomplete".into());
                }
                if session.remember_credentials || Self::secret(&session.auth).is_some() {
                    return Err("Imported hosts cannot contain or remember credentials".into());
                }
                if origins.insert(origin.clone()) {
                    library.sessions.push(session.clone());
                }
            }
            library.validate()
        })
    }

    pub fn save_session_with_credentials(
        &self,
        session: &SessionConfig,
        secret: Option<&str>,
    ) -> Result<(), ConfigError> {
        let _access = self.credential_access.lock();
        let sessions = self.read_library()?.sessions;
        self.require_resolved_credentials(&sessions)?;
        let previous = sessions.iter().find(|saved| saved.id == session.id);
        if session.remember_credentials
            && secret.is_none()
            && previous.is_some_and(|saved| !Self::same_credential_target(saved, session))
        {
            return Err(self.session_error(
                "The SSH destination or authentication settings changed; enter a new credential or disable remembering credentials before saving",
            ));
        }
        let previously_remembered = previous.is_some_and(|saved| saved.remember_credentials);
        if (session.remember_credentials && secret.is_none())
            || (!session.remember_credentials && !previously_remembered)
        {
            return self.save_session_locked(session);
        }
        self.with_credential_change(
            &session.id,
            if session.remember_credentials {
                secret
            } else {
                None
            },
            |id| CredentialStore.get(id),
            |id, secret| match secret {
                Some(secret) => CredentialStore.set(id, secret),
                None => CredentialStore.delete(id),
            },
            || self.save_session_locked(session),
        )
    }

    pub fn delete_session_with_credentials(&self, id: &str) -> Result<(), ConfigError> {
        let _access = self.credential_access.lock();
        let sessions = self.read_library()?.sessions;
        self.require_resolved_credentials(&sessions)?;
        if !sessions
            .iter()
            .any(|session| session.id == id && session.remember_credentials)
        {
            return self.delete_session_locked(id);
        }
        self.with_credential_change(
            id,
            None,
            |id| CredentialStore.get(id),
            |id, secret| match secret {
                Some(secret) => CredentialStore.set(id, secret),
                None => CredentialStore.delete(id),
            },
            || self.delete_session_locked(id),
        )
    }

    pub fn credential_for_session(
        &self,
        session: &SessionConfig,
    ) -> Result<Option<String>, ConfigError> {
        let _access = self.credential_access.lock();
        if !session.remember_credentials {
            return Ok(None);
        }
        let sessions = self.read_library()?.sessions;
        self.require_resolved_credentials(&sessions)?;
        let saved = sessions
            .iter()
            .find(|saved| saved.id == session.id)
            .ok_or_else(|| {
                self.session_error(
                    "The saved host no longer exists; enter a credential before reconnecting",
                )
            })?;
        if !saved.remember_credentials || !Self::same_credential_target(saved, session) {
            return Err(self.session_error(
                "The saved host destination, authentication settings, or credential preference changed; enter a credential before reconnecting",
            ));
        }
        CredentialStore
            .get(&session.id)
            .map_err(|error| self.session_error(error.to_string()))
    }

    pub fn has_legacy_credentials(&self) -> Result<bool, ConfigError> {
        Ok(self
            .load_sessions()?
            .iter()
            .any(|session| Self::secret(&session.auth).is_some()))
    }

    pub fn resolve_legacy_credentials(&self, import: bool) -> Result<(), ConfigError> {
        let _access = self.credential_access.lock();
        let mut library = self.read_library()?;
        let sessions = &mut library.sessions;
        if !sessions
            .iter()
            .any(|session| Self::secret(&session.auth).is_some())
        {
            return Ok(());
        }
        if import {
            let mut ids = HashSet::new();
            for session in sessions.iter() {
                if !ids.insert(session.id.as_str())
                    || (Self::secret(&session.auth).is_some() && session.id.trim().is_empty())
                {
                    return Err(self.session_error(
                        "Legacy credentials cannot be imported until every saved host has a unique, nonempty ID",
                    ));
                }
            }
            for session in sessions.iter() {
                if let Some(secret) = Self::secret(&session.auth) {
                    CredentialStore
                        .set(&session.id, secret)
                        .map_err(|error| self.session_error(error.to_string()))?;
                }
            }
        }
        for session in sessions.iter_mut() {
            if Self::secret(&session.auth).is_some() {
                session.remember_credentials = import;
                Self::clear_secret(&mut session.auth);
            }
        }
        self.write_library(&library)
    }

    pub fn load_theme(&self, name: &str) -> Result<Theme, ConfigError> {
        if let Some(theme) = Theme::builtin(name) {
            return Ok(theme);
        }
        let path = self.base_dir.join("themes").join(format!("{name}.toml"));
        Self::read_toml(&path)?.ok_or_else(|| ConfigError::ThemeNotFound(name.into()))
    }

    pub(crate) fn same_credential_target(previous: &SessionConfig, next: &SessionConfig) -> bool {
        previous.host == next.host
            && previous.port == next.port
            && previous.username == next.username
            && match (&previous.auth, &next.auth) {
                (AuthConfig::Password { .. }, AuthConfig::Password { .. })
                | (AuthConfig::Agent, AuthConfig::Agent) => true,
                (
                    AuthConfig::Key {
                        key_path: previous, ..
                    },
                    AuthConfig::Key { key_path: next, .. },
                ) => previous == next,
                _ => false,
            }
    }

    fn with_credential_change(
        &self,
        id: &str,
        secret: Option<&str>,
        get: impl FnOnce(&str) -> anyhow::Result<Option<String>>,
        mut update: impl FnMut(&str, Option<&str>) -> anyhow::Result<()>,
        persist: impl FnOnce() -> Result<(), ConfigError>,
    ) -> Result<(), ConfigError> {
        let previous = get(id).map_err(|error| self.session_error(error.to_string()))?;
        if previous.as_deref() == secret {
            return persist();
        }
        update(id, secret).map_err(|error| self.session_error(error.to_string()))?;
        match persist() {
            Ok(()) => Ok(()),
            Err(error) => match update(id, previous.as_deref()) {
                Ok(()) => Err(error),
                Err(rollback) => Err(self.session_error(format!(
                    "{error}; the system credential could not be restored: {rollback}. The saved host and system credential may be inconsistent",
                ))),
            },
        }
    }

    fn require_resolved_credentials(&self, sessions: &[SessionConfig]) -> Result<(), ConfigError> {
        if sessions
            .iter()
            .any(|session| Self::secret(&session.auth).is_some())
        {
            return Err(self.session_error(
                "Saved hosts contain legacy plaintext credentials; explicitly import them into the system keyring or discard them before changing saved hosts",
            ));
        }
        Ok(())
    }

    fn secret(auth: &AuthConfig) -> Option<&str> {
        match auth {
            AuthConfig::Password { password } => password.as_deref(),
            AuthConfig::Key { passphrase, .. } => passphrase.as_deref(),
            AuthConfig::Agent => None,
        }
    }

    fn clear_secret(auth: &mut AuthConfig) {
        match auth {
            AuthConfig::Password { password } => *password = None,
            AuthConfig::Key { passphrase, .. } => *passphrase = None,
            AuthConfig::Agent => {}
        }
    }

    fn session_error(&self, reason: impl Into<String>) -> ConfigError {
        ConfigError::ParseError {
            path: self.base_dir.join("sessions.toml").display().to_string(),
            reason: reason.into(),
        }
    }

    fn write_library(&self, library: &HostLibrary) -> Result<(), ConfigError> {
        library
            .validate()
            .map_err(|reason| self.session_error(reason))?;
        let content = toml::to_string_pretty(library)?;
        self.write_atomic(&self.base_dir.join("sessions.toml"), &content, true)
    }

    fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, ConfigError> {
        Self::protect_existing_file(path)?;
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        toml::from_str(&content)
            .map(Some)
            .map_err(|error: toml::de::Error| {
                let reason = match error.span() {
                    Some(span) => {
                        let before = &content.as_bytes()[..span.start.min(content.len())];
                        let line = before.iter().filter(|byte| **byte == b'\n').count() + 1;
                        let column = before.len()
                            - before
                                .iter()
                                .rposition(|byte| *byte == b'\n')
                                .map(|position| position + 1)
                                .unwrap_or(0)
                            + 1;
                        format!(
                            "Invalid TOML or configuration value at line {line}, column {column}"
                        )
                    }
                    None => "Invalid TOML or configuration value".into(),
                };
                ConfigError::ParseError {
                    path: path.display().to_string(),
                    reason,
                }
            })
    }

    fn create_private_dir(path: &Path) -> Result<(), ConfigError> {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    fn protect_existing_file(path: &Path) -> Result<(), ConfigError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            match fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        #[cfg(not(unix))]
        let _ = path;
        Ok(())
    }

    fn write_atomic(&self, path: &Path, content: &str, replace: bool) -> Result<(), ConfigError> {
        let mut file = tempfile::NamedTempFile::new_in(&self.base_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(content.as_bytes())?;
        file.as_file().sync_all()?;
        let result = if replace {
            file.persist(path)
        } else {
            file.persist_noclobber(path)
        };
        match result {
            Ok(_) => {}
            Err(error) if !replace && error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                Self::protect_existing_file(path)?;
            }
            Err(error) => return Err(error.error.into()),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> (tempfile::TempDir, ConfigManager) {
        let directory = tempfile::tempdir().unwrap();
        let manager = ConfigManager::from_dir(directory.path().join("rsterm")).unwrap();
        (directory, manager)
    }

    fn session(id: &str) -> SessionConfig {
        SessionConfig {
            id: id.into(),
            name: "Saved server".into(),
            host: "server.example".into(),
            username: "admin".into(),
            ..SessionConfig::default()
        }
    }

    const LEGACY_SESSIONS: &str = r#"[[sessions]]
id = "password-profile"
name = "Password server"
host = "password.example"
username = "admin"
group = "Operations"
remember_credentials = true
[sessions.auth]
type = "password"
password = "legacy-password-value"

[[sessions]]
id = "key-profile"
name = "Key server"
host = "key.example"
username = "admin"
remember_credentials = true
[sessions.auth]
type = "key"
key_path = "~/.ssh/id_ed25519"
passphrase = "legacy-passphrase-value"
"#;

    #[test]
    fn missing_sessions_and_default_settings_load() {
        let (_directory, manager) = manager();
        assert!(manager.load_sessions().unwrap().is_empty());
        assert_eq!(manager.load_settings().unwrap().app.theme, "dark");
    }

    #[test]
    fn keybindings_first_create_preserves_existing_comments_and_settings_save() {
        let (_directory, manager) = manager();
        let first = manager.load_keybindings();
        assert!(first.warning.is_none());
        assert!(first.keymap.path.is_absolute());
        let path = manager.keybindings_path();
        let defaults = fs::read_to_string(&path).unwrap();
        assert_eq!(defaults, crate::config::default_keybindings_file());
        let custom = "# preserve my comments\nversion = 1\n[bindings]\nshow_history = []\n";
        fs::write(&path, custom).unwrap();
        manager.create_keybindings_if_missing().unwrap();
        let loaded = manager.load_keybindings();
        assert!(loaded.warning.is_none());
        assert!(loaded.keymap.bindings_for("show_history").is_empty());
        manager.save_settings(&AppSettings::default()).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), custom);
    }

    #[test]
    fn invalid_and_oversized_keybindings_fall_back_without_replacing_file() {
        let (_directory, manager) = manager();
        let path = manager.keybindings_path();
        for content in [
            "version = 1\n[bindings]\nnext_tab = [secret-invalid-input]\n".into(),
            "#".repeat(crate::config::MAX_KEYMAP_BYTES + 1),
        ] {
            fs::write(&path, &content).unwrap();
            let loaded = manager.load_keybindings();
            let warning = loaded.warning.unwrap();
            assert!(warning.contains("keybindings.toml"));
            assert!(!warning.contains("secret-invalid-input"));
            assert_eq!(fs::read_to_string(&path).unwrap(), content);
            assert_eq!(loaded.keymap.bindings_for("next_tab"), ["ctrl-tab"]);
        }
    }

    #[test]
    fn concurrent_keybindings_first_creation_never_clobbers_the_winning_file() {
        let (_directory, manager) = manager();
        let path = manager.keybindings_path();
        let workers = (0..8)
            .map(|_| {
                let manager = manager.clone();
                std::thread::spawn(move || manager.create_keybindings_if_missing().unwrap())
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            crate::config::default_keybindings_file()
        );
        manager
            .write_atomic(&path, "must not replace", false)
            .unwrap();
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            crate::config::default_keybindings_file()
        );
    }

    #[test]
    fn settings_round_trip() {
        let (_directory, manager) = manager();
        let mut settings = AppSettings::default();
        settings.app.theme = "light".into();
        settings.terminal.scroll_buffer_lines = 20000;
        settings.ui.session_panel_width = 240.0;
        manager.save_settings(&settings).unwrap();
        let loaded = manager.load_settings().unwrap();
        assert_eq!(loaded.app.theme, "light");
        assert_eq!(loaded.terminal.scroll_buffer_lines, 20000);
        assert_eq!(loaded.ui.session_panel_width, 240.0);
    }

    #[test]
    fn builtin_themes_load_without_files_and_custom_files_remain_supported() {
        let (_directory, manager) = manager();
        for preset in crate::config::BUILTIN_THEMES {
            assert_eq!(manager.load_theme(preset.id).unwrap().name, preset.id);
        }
        let mut custom = Theme::default_dark();
        custom.name = "custom".into();
        fs::write(
            manager.base_dir.join("themes/custom.toml"),
            toml::to_string(&custom).unwrap(),
        )
        .unwrap();
        assert_eq!(manager.load_theme("custom").unwrap().name, "custom");
        assert!(matches!(
            manager.load_theme("missing"),
            Err(ConfigError::ThemeNotFound(_))
        ));
    }

    #[test]
    fn saved_hosts_support_create_update_and_delete() {
        let (_directory, manager) = manager();
        let library = manager.create_folder("Development", None).unwrap();
        let development = library.folders[0].id.clone();
        let library = manager.create_folder("Production", None).unwrap();
        let production = library.folders[1].id.clone();
        manager.save_session(&session("one")).unwrap();
        let mut second = session("two");
        second.folder_id = Some(development);
        second.auth = AuthConfig::Agent;
        manager.save_session(&second).unwrap();
        let mut first = session("one");
        first.name = "Renamed server".into();
        first.folder_id = Some(production.clone());
        first.remember_credentials = true;
        first.auth = AuthConfig::Password {
            password: Some("transient-password-value".into()),
        };
        manager.save_session(&first).unwrap();
        let loaded = manager.load_sessions().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "Renamed server");
        assert_eq!(loaded[0].folder_id.as_deref(), Some(production.as_str()));
        assert_eq!(manager.load_library().unwrap().folders.len(), 2);
        assert_eq!(loaded[0].port, 22);
        assert!(loaded[0].remember_credentials);
        assert!(matches!(
            loaded[0].auth,
            AuthConfig::Password { password: None }
        ));
        assert!(matches!(loaded[1].auth, AuthConfig::Agent));
        let content = fs::read_to_string(manager.base_dir.join("sessions.toml")).unwrap();
        assert!(!content.contains("transient-password-value"));
        manager.delete_session("one").unwrap();
        manager.delete_session("missing").unwrap();
        let loaded = manager.load_sessions().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "two");
        manager.delete_session("two").unwrap();
        assert!(manager.load_sessions().unwrap().is_empty());
    }

    #[test]
    fn credential_aware_save_without_a_new_secret_only_updates_metadata() {
        let (_directory, manager) = manager();
        let mut profile = session("one");
        profile.remember_credentials = true;
        manager
            .save_session_with_credentials(&profile, None)
            .unwrap();
        let loaded = manager.load_sessions().unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].remember_credentials);
    }

    #[test]
    fn changed_credential_targets_cannot_reuse_the_saved_secret() {
        let (_directory, manager) = manager();
        let mut original = session("one");
        original.remember_credentials = true;
        manager.save_session(&original).unwrap();
        let path = manager.base_dir.join("sessions.toml");
        let before = fs::read_to_string(&path).unwrap();
        let mut changed_host = original.clone();
        changed_host.host = "different.example".into();
        let mut changed_port = original.clone();
        changed_port.port = 2222;
        let mut changed_username = original.clone();
        changed_username.username = "different-user".into();
        let mut changed_auth = original.clone();
        changed_auth.auth = AuthConfig::Agent;
        let mut changed_key = original.clone();
        changed_key.auth = AuthConfig::Key {
            key_path: "~/.ssh/id_ed25519".into(),
            passphrase: None,
        };
        for changed in [
            changed_host,
            changed_port,
            changed_username,
            changed_auth,
            changed_key,
        ] {
            let error = manager
                .save_session_with_credentials(&changed, None)
                .unwrap_err();
            assert!(error.to_string().contains("enter a new credential"));
            assert_eq!(fs::read_to_string(&path).unwrap(), before);
        }
        original.auth = AuthConfig::Key {
            key_path: "~/.ssh/id_ed25519".into(),
            passphrase: None,
        };
        manager.save_session(&original).unwrap();
        let before = fs::read_to_string(&path).unwrap();
        let mut changed = original.clone();
        changed.auth = AuthConfig::Key {
            key_path: "~/.ssh/another_key".into(),
            passphrase: None,
        };
        assert!(manager
            .save_session_with_credentials(&changed, None)
            .is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), before);
    }

    #[test]
    fn stale_session_cannot_read_a_changed_targets_credential() {
        let (_directory, manager) = manager();
        let mut original = session("one");
        original.remember_credentials = true;
        manager.save_session(&original).unwrap();
        let mut changed = original.clone();
        changed.host = "another.example".into();
        manager.save_session(&changed).unwrap();
        let error = manager.credential_for_session(&original).unwrap_err();
        assert!(error
            .to_string()
            .contains("enter a credential before reconnecting"));
    }

    #[test]
    fn removed_or_opted_out_saved_session_cannot_read_credential() {
        let (_directory, manager) = manager();
        let mut original = session("one");
        original.remember_credentials = true;
        assert!(manager.credential_for_session(&original).is_err());
        let mut saved = original.clone();
        saved.remember_credentials = false;
        manager.save_session(&saved).unwrap();
        assert!(manager.credential_for_session(&original).is_err());
        assert!(manager.credential_for_session(&saved).unwrap().is_none());
    }

    #[test]
    fn cloned_managers_share_credential_access_lock() {
        let (_directory, manager) = manager();
        let cloned = manager.clone();
        assert!(Arc::ptr_eq(
            &manager.credential_access,
            &cloned.credential_access
        ));
        let access = manager.credential_access.lock();
        assert!(cloned.credential_access.try_lock().is_none());
        drop(access);
        assert!(cloned.credential_access.try_lock().is_some());
    }

    #[test]
    fn saved_credential_survives_name_and_folder_changes_without_keyring_access() {
        let (_directory, manager) = manager();
        let library = manager.create_folder("Another folder", None).unwrap();
        let folder = library.folders[0].id.clone();
        let mut profile = session("one");
        profile.remember_credentials = true;
        manager.save_session(&profile).unwrap();
        profile.name = "Renamed".into();
        profile.folder_id = Some(folder.clone());
        manager
            .save_session_with_credentials(&profile, None)
            .unwrap();
        let loaded = manager.load_sessions().unwrap();
        assert_eq!(loaded[0].name, "Renamed");
        assert_eq!(loaded[0].folder_id.as_deref(), Some(folder.as_str()));
        assert!(loaded[0].remember_credentials);
        assert_eq!(manager.load_library().unwrap().folders.len(), 1);
    }

    #[test]
    fn changed_opt_out_target_saves_without_keyring_access() {
        let (_directory, manager) = manager();
        let mut profile = session("one");
        manager.save_session(&profile).unwrap();
        profile.host = "different.example".into();
        profile.username = "different-user".into();
        profile.port = 2222;
        manager
            .save_session_with_credentials(&profile, None)
            .unwrap();
        let loaded = manager.load_sessions().unwrap();
        assert_eq!(loaded[0].host, "different.example");
        assert_eq!(loaded[0].username, "different-user");
        assert_eq!(loaded[0].port, 2222);
        assert!(!loaded[0].remember_credentials);
    }

    #[test]
    fn credential_aware_opt_out_save_and_delete_do_not_access_keyring() {
        let (_directory, manager) = manager();
        let profile = session("one");
        manager
            .save_session_with_credentials(&profile, Some("transient-secret"))
            .unwrap();
        let loaded = manager.load_sessions().unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(!loaded[0].remember_credentials);
        manager.delete_session_with_credentials("one").unwrap();
        manager.delete_session_with_credentials("missing").unwrap();
        assert!(manager.load_sessions().unwrap().is_empty());
    }

    #[test]
    fn credential_aware_mutations_refuse_legacy_before_accessing_keyring() {
        let (_directory, manager) = manager();
        let path = manager.base_dir.join("sessions.toml");
        fs::write(&path, LEGACY_SESSIONS).unwrap();
        assert!(manager
            .save_session_with_credentials(&session("new"), Some("not-saved"))
            .is_err());
        assert!(manager
            .delete_session_with_credentials("password-profile")
            .is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), LEGACY_SESSIONS);
    }

    #[test]
    fn credential_change_commits_secret_before_metadata() {
        use std::cell::RefCell;

        let (_directory, manager) = manager();
        let stored = RefCell::new(Some("old-secret".to_owned()));
        let mut profile = session("one");
        profile.remember_credentials = true;
        manager
            .with_credential_change(
                &profile.id,
                Some("new-secret"),
                |_| Ok(stored.borrow().clone()),
                |_, secret| {
                    *stored.borrow_mut() = secret.map(str::to_owned);
                    Ok(())
                },
                || {
                    assert_eq!(stored.borrow().as_deref(), Some("new-secret"));
                    manager.save_session(&profile)
                },
            )
            .unwrap();
        assert_eq!(stored.borrow().as_deref(), Some("new-secret"));
        assert!(manager.load_sessions().unwrap()[0].remember_credentials);
    }

    #[test]
    fn failed_metadata_save_restores_changed_or_removed_secret() {
        use std::cell::{Cell, RefCell};

        let (_directory, manager) = manager();
        for (previous, next) in [
            (Some("old-secret"), Some("new-secret")),
            (None, Some("new-secret")),
            (Some("old-secret"), None),
        ] {
            let stored = RefCell::new(previous.map(str::to_owned));
            let updates = Cell::new(0);
            let error = manager
                .with_credential_change(
                    "one",
                    next,
                    |_| Ok(stored.borrow().clone()),
                    |_, secret| {
                        updates.set(updates.get() + 1);
                        *stored.borrow_mut() = secret.map(str::to_owned);
                        Ok(())
                    },
                    || Err(manager.session_error("metadata-save-failed")),
                )
                .unwrap_err();
            assert!(error.to_string().contains("metadata-save-failed"));
            assert_eq!(stored.borrow().as_deref(), previous);
            assert_eq!(updates.get(), 2);
        }
    }

    #[test]
    fn credential_read_or_write_failure_leaves_metadata_unchanged() {
        let (_directory, manager) = manager();
        assert!(manager
            .with_credential_change(
                "one",
                Some("new-secret"),
                |_| Err(anyhow::anyhow!("credential-read-failed")),
                |_, _| panic!("must not update after failed read"),
                || panic!("must not persist after failed read"),
            )
            .unwrap_err()
            .to_string()
            .contains("credential-read-failed"));
        assert!(manager
            .with_credential_change(
                "one",
                Some("new-secret"),
                |_| Ok(Some("old-secret".into())),
                |_, _| Err(anyhow::anyhow!("credential-write-failed")),
                || panic!("must not persist after failed credential write"),
            )
            .unwrap_err()
            .to_string()
            .contains("credential-write-failed"));
        assert!(manager.load_sessions().unwrap().is_empty());
    }

    #[test]
    fn rollback_failure_reports_both_errors_without_secret_values() {
        use std::cell::Cell;

        let (_directory, manager) = manager();
        let updates = Cell::new(0);
        let error = manager
            .with_credential_change(
                "one",
                Some("new-secret"),
                |_| Ok(Some("old-secret".into())),
                |_, _| {
                    updates.set(updates.get() + 1);
                    if updates.get() == 1 {
                        Ok(())
                    } else {
                        Err(anyhow::anyhow!("credential-rollback-failed"))
                    }
                },
                || Err(manager.session_error("metadata-save-failed")),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("metadata-save-failed"));
        assert!(error.contains("credential-rollback-failed"));
        assert!(error.contains("may be inconsistent"));
        assert!(!error.contains("old-secret"));
        assert!(!error.contains("new-secret"));
    }

    #[test]
    fn unchanged_credentials_do_not_access_the_store_for_writes() {
        let (_directory, manager) = manager();
        for secret in [None, Some("existing-secret")] {
            manager
                .with_credential_change(
                    "one",
                    secret,
                    |_| Ok(secret.map(str::to_owned)),
                    |_, _| panic!("must not rewrite an unchanged credential"),
                    || manager.save_session(&session("one")),
                )
                .unwrap();
        }
        assert_eq!(manager.load_sessions().unwrap().len(), 1);
    }

    #[test]
    fn credential_aware_delete_commits_removal_before_metadata() {
        use std::cell::RefCell;

        let (_directory, manager) = manager();
        manager.save_session(&session("one")).unwrap();
        let stored = RefCell::new(Some("old-secret".to_owned()));
        manager
            .with_credential_change(
                "one",
                None,
                |_| Ok(stored.borrow().clone()),
                |_, secret| {
                    *stored.borrow_mut() = secret.map(str::to_owned);
                    Ok(())
                },
                || {
                    assert!(stored.borrow().is_none());
                    manager.delete_session("one")
                },
            )
            .unwrap();
        assert!(stored.borrow().is_none());
        assert!(manager.load_sessions().unwrap().is_empty());
    }

    #[test]
    fn new_key_passphrases_are_not_written() {
        let (_directory, manager) = manager();
        let mut profile = session("key-profile");
        profile.auth = AuthConfig::Key {
            key_path: "~/.ssh/id_ed25519".into(),
            passphrase: Some("transient-passphrase-value".into()),
        };
        manager.save_session(&profile).unwrap();
        let content = fs::read_to_string(manager.base_dir.join("sessions.toml")).unwrap();
        assert!(!content.contains("transient-passphrase-value"));
        assert!(!content.contains("passphrase"));
        assert!(matches!(
            &manager.load_sessions().unwrap()[0].auth,
            AuthConfig::Key { key_path, passphrase: None }
                if key_path == "~/.ssh/id_ed25519"
        ));
    }

    #[test]
    fn legacy_credentials_block_all_session_mutations_without_changing_file() {
        let (_directory, manager) = manager();
        let path = manager.base_dir.join("sessions.toml");
        fs::write(&path, LEGACY_SESSIONS).unwrap();
        assert!(manager.has_legacy_credentials().unwrap());
        assert!(manager.save_session(&session("new")).is_err());
        assert!(manager.delete_session("password-profile").is_err());
        assert!(manager.delete_session("missing").is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), LEGACY_SESSIONS);
    }

    #[test]
    fn explicit_legacy_discard_clears_secrets_and_preserves_metadata() {
        let (_directory, manager) = manager();
        let path = manager.base_dir.join("sessions.toml");
        fs::write(&path, LEGACY_SESSIONS).unwrap();
        manager.resolve_legacy_credentials(false).unwrap();
        assert!(!manager.has_legacy_credentials().unwrap());
        let content = fs::read_to_string(&path).unwrap();
        assert!(!content.contains("legacy-password-value"));
        assert!(!content.contains("legacy-passphrase-value"));
        let loaded = manager.load_sessions().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "password-profile");
        assert_eq!(loaded[0].name, "Password server");
        let library = manager.load_library().unwrap();
        assert_eq!(library.folders.len(), 1);
        assert_eq!(library.folders[0].name, "Operations");
        assert_eq!(
            loaded[0].folder_id.as_deref(),
            Some(library.folders[0].id.as_str())
        );
        assert!(!loaded[0].remember_credentials);
        assert!(!loaded[1].remember_credentials);
        assert!(matches!(
            loaded[0].auth,
            AuthConfig::Password { password: None }
        ));
        assert!(matches!(
            &loaded[1].auth,
            AuthConfig::Key { key_path, passphrase: None }
                if key_path == "~/.ssh/id_ed25519"
        ));
        manager.save_session(&session("new")).unwrap();
        manager.resolve_legacy_credentials(false).unwrap();
        let names: Vec<_> = fs::read_dir(&manager.base_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"config.toml".into()));
        assert!(names.contains(&"sessions.toml".into()));
        assert!(names.contains(&"themes".into()));
    }

    #[test]
    fn invalid_legacy_profile_ids_fail_before_accessing_keyring() {
        for legacy in [
            LEGACY_SESSIONS.replace("password-profile", ""),
            LEGACY_SESSIONS.replace("key-profile", "password-profile"),
        ] {
            let (_directory, manager) = manager();
            let path = manager.base_dir.join("sessions.toml");
            fs::write(&path, &legacy).unwrap();
            assert!(manager.resolve_legacy_credentials(true).is_err());
            assert_eq!(fs::read_to_string(path).unwrap(), legacy);
        }
    }

    #[test]
    fn parse_errors_are_visible_and_do_not_echo_secret_source() {
        let (_directory, manager) = manager();
        let path = manager.base_dir.join("sessions.toml");
        let invalid = "[[sessions]]\n[sessions.auth]\ntype = \"password\"\npassword = secret-value-not-for-errors\n";
        fs::write(&path, invalid).unwrap();
        let error = manager.load_sessions().unwrap_err();
        let display = error.to_string();
        assert!(display.contains("sessions.toml"));
        assert!(display.contains("line 4"));
        assert!(!display.contains("secret-value-not-for-errors"));
        assert!(!format!("{error:?}").contains("secret-value-not-for-errors"));
        assert!(manager.has_legacy_credentials().is_err());
        assert!(manager.save_session(&session("new")).is_err());
        assert!(manager.resolve_legacy_credentials(false).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), invalid);
        fs::write(manager.base_dir.join("config.toml"), "[app\n").unwrap();
        assert!(manager.load_settings().is_err());
    }

    fn imported_session(id: &str, alias: &str) -> SessionConfig {
        SessionConfig {
            id: id.into(),
            name: format!("Imported {alias}"),
            host: "import.example".into(),
            username: "fixture".into(),
            auth: AuthConfig::Agent,
            import_origin: Some(crate::config::ImportOrigin {
                source_path: "/fixture/ssh-config".into(),
                alias: alias.into(),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn legacy_group_migration_only_writes_on_explicit_mutation() {
        let (_directory, manager) = manager();
        let path = manager.base_dir.join("sessions.toml");
        let legacy = "[[sessions]]\nid = 'old'\ngroup = ' 运维/Prod '\n";
        fs::write(&path, legacy).unwrap();
        let first = manager.load_library().unwrap();
        let second = manager.load_library().unwrap();
        assert_eq!(first.folders, second.folders);
        assert_eq!(first.folders[0].name, " 运维/Prod ");
        assert_eq!(fs::read_to_string(&path).unwrap(), legacy);
        manager.save_session(&session("new")).unwrap();
        let reloaded = manager.load_library().unwrap();
        assert_eq!(reloaded.folders, first.folders);
        assert_eq!(reloaded.sessions[0].folder_id, first.sessions[0].folder_id);
        let content: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(content["sessions"][0].get("group").is_none());
        assert_eq!(fs::read_dir(&manager.base_dir).unwrap().count(), 3);
    }

    #[test]
    fn nested_and_empty_folders_persist_through_all_metadata_mutations() {
        let (_directory, manager) = manager();
        let root = manager.create_folder("Root", None).unwrap().folders[0]
            .id
            .clone();
        let child = manager.create_folder("Child", Some(&root)).unwrap().folders[1]
            .id
            .clone();
        let empty = manager.create_folder("Empty", None).unwrap().folders[2]
            .id
            .clone();
        let mut imported = imported_session("imported", "server");
        imported.folder_id = Some(child.clone());
        manager.import_sessions(&[imported.clone()]).unwrap();
        manager.save_session(&session("ordinary")).unwrap();
        let folders = manager.load_library().unwrap().folders;
        let mut edited = imported.clone();
        edited.name = "Renamed imported host".into();
        edited.import_origin = None;
        manager
            .save_session_with_credentials(&edited, None)
            .unwrap();
        let moved = manager
            .move_node(&HostNodeId::Host(imported.id.clone()), None)
            .unwrap();
        let saved = moved
            .sessions
            .iter()
            .find(|session| session.id == imported.id)
            .unwrap();
        assert!(saved.folder_id.is_none());
        assert_eq!(saved.import_origin, imported.import_origin);
        assert!(ConfigManager::same_credential_target(saved, &imported));
        manager.delete_session_with_credentials("ordinary").unwrap();
        assert_eq!(manager.load_library().unwrap().folders, folders);
        manager.rename_folder(&root, "Renamed root").unwrap();
        manager
            .move_node(&HostNodeId::Folder(child.clone()), None)
            .unwrap();
        manager.delete_empty_folder(&empty).unwrap();
        manager.delete_empty_folder(&root).unwrap();
        let reopened = ConfigManager::from_dir(manager.base_dir.clone())
            .unwrap()
            .load_library()
            .unwrap();
        assert_eq!(reopened.folders.len(), 1);
        assert_eq!(reopened.folders[0].id, child);
        assert!(reopened.folders[0].parent_id.is_none());
        assert_eq!(reopened.sessions[0].import_origin, imported.import_origin);
    }

    #[test]
    fn legacy_discard_preserves_existing_folders_and_import_origins() {
        let (_directory, manager) = manager();
        let path = manager.base_dir.join("sessions.toml");
        let content = format!(
            "[[folders]]\nid = 'empty'\nname = 'Empty'\n{LEGACY_SESSIONS}\n[sessions.import_origin]\nsource_path = '/fixture/config'\nalias = 'key'\n"
        );
        fs::write(&path, content).unwrap();
        let before = manager.load_library().unwrap();
        manager.resolve_legacy_credentials(false).unwrap();
        let after = manager.load_library().unwrap();
        assert_eq!(after.folders, before.folders);
        assert_eq!(
            after.sessions[1].import_origin,
            before.sessions[1].import_origin
        );
        let content = fs::read_to_string(path).unwrap();
        assert!(!content.contains("legacy-password-value"));
        assert!(!content.contains("legacy-passphrase-value"));
        assert_eq!(fs::read_dir(&manager.base_dir).unwrap().count(), 3);
    }

    #[test]
    fn folder_and_import_mutations_refuse_unresolved_plaintext_credentials() {
        let (_directory, manager) = manager();
        let path = manager.base_dir.join("sessions.toml");
        fs::write(&path, LEGACY_SESSIONS).unwrap();
        let folder = manager.load_library().unwrap().folders[0].id.clone();
        assert!(manager.create_folder("New", None).is_err());
        assert!(manager.rename_folder(&folder, "Renamed").is_err());
        assert!(manager
            .move_node(&HostNodeId::Host("password-profile".into()), None)
            .is_err());
        assert!(manager.delete_empty_folder(&folder).is_err());
        assert!(manager
            .import_sessions(&[imported_session("new", "new")])
            .is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), LEGACY_SESSIONS);
        assert_eq!(fs::read_dir(&manager.base_dir).unwrap().count(), 3);
    }

    #[test]
    fn repeat_import_skips_existing_origins_after_rename_move_or_edit() {
        let (_directory, manager) = manager();
        let original = imported_session("first", "server");
        manager
            .import_sessions(std::slice::from_ref(&original))
            .unwrap();
        let folder = manager.create_folder("Moved", None).unwrap().folders[0]
            .id
            .clone();
        manager
            .move_node(&HostNodeId::Host(original.id.clone()), Some(&folder))
            .unwrap();
        let mut edited = manager.load_sessions().unwrap().remove(0);
        edited.name = "Local name".into();
        edited.host = "locally-edited.example".into();
        edited.import_origin = None;
        manager.save_session(&edited).unwrap();
        let duplicate = imported_session("second", "server");
        let loaded = manager
            .import_sessions(&[duplicate.clone(), duplicate])
            .unwrap();
        assert_eq!(loaded.sessions.len(), 1);
        assert_eq!(loaded.sessions[0].id, original.id);
        assert_eq!(loaded.sessions[0].name, edited.name);
        assert_eq!(loaded.sessions[0].host, edited.host);
        assert_eq!(
            loaded.sessions[0].folder_id.as_deref(),
            Some(folder.as_str())
        );
        assert_eq!(loaded.sessions[0].import_origin, original.import_origin);
        let mut another_source = imported_session("other-source", "server");
        another_source.import_origin.as_mut().unwrap().source_path =
            "/fixture/another-config".into();
        assert_eq!(
            manager
                .import_sessions(&[another_source])
                .unwrap()
                .sessions
                .len(),
            2
        );
        manager.delete_session("first").unwrap();
        let reimport = imported_session("fresh", "server");
        assert_eq!(
            manager.import_sessions(&[reimport]).unwrap().sessions.len(),
            2
        );
    }

    #[test]
    fn invalid_import_batches_leave_the_previous_file_unchanged() {
        let (_directory, manager) = manager();
        manager.save_session(&session("existing")).unwrap();
        let path = manager.base_dir.join("sessions.toml");
        let before = fs::read_to_string(&path).unwrap();
        let valid = imported_session("valid", "valid");
        let profile = imported_session("invalid", "invalid");
        let mut invalid = Vec::new();
        let mut changed = profile.clone();
        changed.id.clear();
        invalid.push(changed);
        let mut changed = profile.clone();
        changed.id = "existing".into();
        invalid.push(changed);
        let mut changed = profile.clone();
        changed.name.clear();
        invalid.push(changed);
        let mut changed = profile.clone();
        changed.host = "invalid host".into();
        invalid.push(changed);
        let mut changed = profile.clone();
        changed.username.clear();
        invalid.push(changed);
        let mut changed = profile.clone();
        changed.port = 0;
        invalid.push(changed);
        let mut changed = profile.clone();
        changed.auth = AuthConfig::Key {
            key_path: String::new(),
            passphrase: None,
        };
        invalid.push(changed);
        let mut changed = profile.clone();
        changed.auth = AuthConfig::Password {
            password: Some("must-not-save".into()),
        };
        invalid.push(changed);
        let mut changed = profile.clone();
        changed.auth = AuthConfig::Key {
            key_path: "/fixture/key".into(),
            passphrase: Some("must-not-save".into()),
        };
        invalid.push(changed);
        let mut changed = profile.clone();
        changed.remember_credentials = true;
        invalid.push(changed);
        let mut changed = profile.clone();
        changed.import_origin = None;
        invalid.push(changed);
        let mut changed = profile.clone();
        changed.import_origin.as_mut().unwrap().alias.clear();
        invalid.push(changed);
        let mut changed = profile.clone();
        changed.import_origin.as_mut().unwrap().source_path = PathBuf::new();
        invalid.push(changed);
        let mut changed = profile;
        changed.folder_id = Some("missing".into());
        invalid.push(changed);
        for invalid in invalid {
            assert!(manager.import_sessions(&[valid.clone(), invalid]).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), before);
            assert_eq!(fs::read_dir(&manager.base_dir).unwrap().count(), 3);
        }
    }

    #[test]
    fn concurrent_clones_do_not_lose_folder_or_batch_import_updates() {
        let (_directory, manager) = manager();
        let mut workers = Vec::new();
        for index in 0..12 {
            let manager = manager.clone();
            workers.push(std::thread::spawn(move || {
                manager
                    .create_folder(&format!("Folder {index}"), None)
                    .unwrap();
                let profile = imported_session(&format!("profile-{index}"), "same-alias");
                manager.import_sessions(&[profile]).unwrap();
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        let library = manager.load_library().unwrap();
        assert_eq!(library.folders.len(), 12);
        assert_eq!(library.sessions.len(), 1);
    }

    #[test]
    fn invalid_metadata_mutations_preserve_the_existing_document() {
        let (_directory, manager) = manager();
        let root = manager.create_folder("Root", None).unwrap().folders[0]
            .id
            .clone();
        let child = manager.create_folder("Child", Some(&root)).unwrap().folders[1]
            .id
            .clone();
        let path = manager.base_dir.join("sessions.toml");
        let before = fs::read_to_string(&path).unwrap();
        assert!(manager
            .move_node(&HostNodeId::Folder(root.clone()), Some(&child))
            .is_err());
        assert!(manager.delete_empty_folder(&root).is_err());
        assert!(manager.rename_folder(&child, "").is_err());
        assert!(manager.create_folder("Root", None).is_err());
        let mut profile = session("host");
        profile.folder_id = Some("missing".into());
        assert!(manager.save_session(&profile).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn failed_batch_publication_leaves_existing_destination_intact() {
        let (_directory, manager) = manager();
        let target = manager.base_dir.join("sessions.toml");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("existing"), "original").unwrap();
        let library = HostLibrary {
            folders: Vec::new(),
            sessions: vec![
                imported_session("one", "one"),
                imported_session("two", "two"),
            ],
        };
        assert!(manager.write_library(&library).is_err());
        assert!(target.is_dir());
        assert_eq!(
            fs::read_to_string(target.join("existing")).unwrap(),
            "original"
        );
        assert_eq!(fs::read_dir(&manager.base_dir).unwrap().count(), 3);
    }

    #[test]
    fn failed_atomic_replacement_preserves_the_previous_target_and_cleans_tempfile() {
        let (_directory, manager) = manager();
        let target = manager.base_dir.join("occupied");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("existing"), "original").unwrap();
        assert!(manager.write_atomic(&target, "replacement", true).is_err());
        assert!(target.is_dir());
        assert_eq!(
            fs::read_to_string(target.join("existing")).unwrap(),
            "original"
        );
        assert_eq!(fs::read_dir(&manager.base_dir).unwrap().count(), 3);
    }

    #[test]
    fn atomic_writes_do_not_use_fixed_temporary_names() {
        let (_directory, manager) = manager();
        let path = manager.base_dir.join("sessions.toml.tmp");
        fs::write(&path, "unrelated existing file").unwrap();
        manager.save_session(&session("one")).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "unrelated existing file");
        let path = manager.base_dir.join("config.toml.tmp");
        fs::write(&path, "another unrelated file").unwrap();
        manager.save_settings(&AppSettings::default()).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "another unrelated file");
    }

    #[cfg(unix)]
    #[test]
    fn directories_and_files_have_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let (_directory, manager) = manager();
        manager.save_session(&session("one")).unwrap();
        for name in ["", "themes"] {
            assert_eq!(
                fs::metadata(manager.base_dir.join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        for name in ["config.toml", "sessions.toml"] {
            let path = manager.base_dir.join(name);
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            fs::set_permissions(path, fs::Permissions::from_mode(0o666)).unwrap();
        }
        fs::set_permissions(&manager.base_dir, fs::Permissions::from_mode(0o777)).unwrap();
        let reopened = ConfigManager::from_dir(manager.base_dir.clone()).unwrap();
        assert_eq!(
            fs::metadata(&reopened.base_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for name in ["config.toml", "sessions.toml"] {
            assert_eq!(
                fs::metadata(reopened.base_dir.join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
