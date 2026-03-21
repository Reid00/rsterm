// src/config/mod.rs
//! Configuration management

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Session configuration placeholder
/// Full implementation will be added in M5
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub startup_command: Option<String>,
    #[serde(default)]
    pub environment: HashMap<String, String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub last_connected: Option<String>,
}

fn default_port() -> u16 {
    22
}

/// Authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AuthConfig {
    Password {
        #[serde(default)]
        password: Option<String>,
    },
    Key {
        #[serde(default)]
        key_path: String,
        #[serde(default)]
        passphrase: Option<String>,
    },
}

impl Default for AuthConfig {
    fn default() -> Self {
        AuthConfig::Password { password: None }
    }
}

/// Configuration manager - stub for Task 1.4
/// Full implementation will be added in M1-T1.4
pub struct ConfigManager {
    _config_dir: std::path::PathBuf,
}

impl ConfigManager {
    pub fn new() -> Result<Self, crate::error::ConfigError> {
        let config_dir = dirs::config_dir()
            .ok_or(crate::error::ConfigError::NoPlatformConfigDir)?
            .join("rsterm");

        Ok(Self { _config_dir: config_dir })
    }

    pub fn load_settings(&self) -> Result<AppSettings, crate::error::ConfigError> {
        Ok(AppSettings::default())
    }

    pub fn load_theme(&self, _name: &str) -> Result<Theme, crate::error::ConfigError> {
        Ok(Theme::default_dark())
    }
}

/// Application settings - stub for Task 1.4
#[derive(Debug, Clone, Default)]
pub struct AppSettings {
    pub app: AppSettingsInner,
}

#[derive(Debug, Clone, Default)]
pub struct AppSettingsInner {
    pub theme: String,
}

/// Theme configuration - stub for Task 1.4
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
}

impl Theme {
    pub fn default_dark() -> Self {
        Self {
            name: "dark".to_string(),
        }
    }
}