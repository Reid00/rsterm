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