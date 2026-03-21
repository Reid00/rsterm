// src/config/manager.rs
use std::fs;
use std::path::PathBuf;

use crate::config::{AppSettings, SessionConfig, Theme};
use crate::error::ConfigError;

pub struct ConfigManager {
    base_dir: PathBuf,
}

impl ConfigManager {
    pub fn new() -> Result<Self, ConfigError> {
        // Use dirs crate (not directories) - already in Cargo.toml
        let base = dirs::config_dir()
            .map(|d| d.join("rsterm"))
            .ok_or(ConfigError::NoPlatformConfigDir)?;

        if !base.exists() {
            Self::initialize_dirs(&base)?;
        }

        Ok(Self { base_dir: base })
    }

    fn initialize_dirs(base: &std::path::Path) -> Result<(), ConfigError> {
        fs::create_dir_all(base)?;
        fs::create_dir_all(base.join("themes"))?;

        // Write default config
        let settings = AppSettings::default();
        let toml = toml::to_string_pretty(&settings)?;
        fs::write(base.join("config.toml"), toml)?;

        // Sessions file is optional - load_sessions handles missing file

        Ok(())
    }

    pub fn load_settings(&self) -> Result<AppSettings, ConfigError> {
        let path = self.base_dir.join("config.toml");
        if !path.exists() {
            return Ok(AppSettings::default());
        }
        let content = fs::read_to_string(&path)?;
        Ok(toml::from_str(&content)?)
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), ConfigError> {
        let content = toml::to_string_pretty(settings)?;
        let tmp = self.base_dir.join("config.toml.tmp");
        fs::write(&tmp, &content)?;
        fs::rename(&tmp, self.base_dir.join("config.toml"))?;
        Ok(())
    }

    pub fn load_sessions(&self) -> Result<Vec<SessionConfig>, ConfigError> {
        let path = self.base_dir.join("sessions.toml");
        if !path.exists() {
            return Ok(vec![]);
        }
        let content = fs::read_to_string(&path)?;
        #[derive(serde::Deserialize)]
        struct SessionsFile {
            sessions: Vec<SessionConfig>,
        }
        let file: SessionsFile = toml::from_str(&content)?;
        Ok(file.sessions)
    }

    pub fn save_session(&self, session: &SessionConfig) -> Result<(), ConfigError> {
        let mut sessions = self.load_sessions()?;
        if let Some(existing) = sessions.iter_mut().find(|s| s.id == session.id) {
            *existing = session.clone();
        } else {
            sessions.push(session.clone());
        }
        self.write_sessions(&sessions)
    }

    pub fn delete_session(&self, id: &str) -> Result<(), ConfigError> {
        let mut sessions = self.load_sessions()?;
        let initial_len = sessions.len();
        sessions.retain(|s| s.id != id);
        if sessions.len() == initial_len {
            // Session not found - not an error, just no-op
            return Ok(());
        }
        self.write_sessions(&sessions)
    }

    fn write_sessions(&self, sessions: &[SessionConfig]) -> Result<(), ConfigError> {
        #[derive(serde::Serialize)]
        struct SessionsFile<'a> {
            sessions: &'a [SessionConfig],
        }
        let file = SessionsFile { sessions };
        let content = toml::to_string_pretty(&file)?;
        let tmp = self.base_dir.join("sessions.toml.tmp");
        fs::write(&tmp, &content)?;
        fs::rename(&tmp, self.base_dir.join("sessions.toml"))?;
        Ok(())
    }

    pub fn load_theme(&self, name: &str) -> Result<Theme, ConfigError> {
        let path = self.base_dir.join("themes").join(format!("{}.toml", name));
        if !path.exists() {
            return Err(ConfigError::ThemeNotFound(name.into()));
        }
        let content = fs::read_to_string(&path)?;
        Ok(toml::from_str(&content)?)
    }
}