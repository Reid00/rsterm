// src/config/settings.rs
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

use super::ImportOrigin;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppSettings {
    #[serde(default)]
    pub app: AppConfig,
    #[serde(default)]
    pub terminal: TerminalConfig,
    #[serde(default)]
    pub ssh: SshConfig,
    #[serde(default)]
    pub sftp: SftpConfig,
    #[serde(default)]
    pub ui: UiStateConfig,
}

/// UI state that should persist between sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiStateConfig {
    #[serde(default = "default_session_panel_width")]
    pub session_panel_width: f32,
    #[serde(default = "default_sftp_panel_width")]
    pub sftp_panel_width: f32,
    #[serde(default = "default_inspector_panel_width")]
    pub inspector_panel_width: f32,
    #[serde(default = "default_true")]
    pub local_group_open: bool,
    #[serde(default = "default_true")]
    pub ssh_group_open: bool,
}

fn default_session_panel_width() -> f32 {
    200.0
}
fn default_sftp_panel_width() -> f32 {
    200.0
}
fn default_inspector_panel_width() -> f32 {
    320.0
}

impl Default for UiStateConfig {
    fn default() -> Self {
        Self {
            session_panel_width: default_session_panel_width(),
            sftp_panel_width: default_sftp_panel_width(),
            inspector_panel_width: default_inspector_panel_width(),
            local_group_open: true,
            ssh_group_open: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_font")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
}

fn default_theme() -> String {
    "dark".into()
}
fn default_font() -> String {
    "Menlo".into()
}
fn default_font_size() -> f32 {
    14.0
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            font_family: default_font(),
            font_size: default_font_size(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalConfig {
    #[serde(default = "default_scroll_buffer")]
    pub scroll_buffer_lines: usize,
    #[serde(default)]
    pub copy_on_select: bool,
    #[serde(default = "default_true")]
    pub right_click_paste: bool,
    #[serde(default = "default_cursor_style")]
    pub cursor_style: String,
    #[serde(default = "default_true")]
    pub cursor_blink: bool,
}

fn default_scroll_buffer() -> usize {
    10000
}
fn default_cursor_style() -> String {
    "block".into()
}
fn default_true() -> bool {
    true
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            scroll_buffer_lines: default_scroll_buffer(),
            copy_on_select: false,
            right_click_paste: true,
            cursor_style: default_cursor_style(),
            cursor_blink: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConfig {
    #[serde(default = "default_timeout")]
    pub connection_timeout_secs: u64,
    #[serde(default = "default_keepalive")]
    pub keepalive_interval_secs: u64,
    #[serde(default = "default_keepalive_max")]
    pub keepalive_max_count: u32,
    #[serde(default = "default_reconnect_max")]
    pub reconnect_max_attempts: u32,
}

fn default_timeout() -> u64 {
    30
}
fn default_keepalive() -> u64 {
    30
}
fn default_keepalive_max() -> u32 {
    3
}
fn default_reconnect_max() -> u32 {
    3
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            connection_timeout_secs: default_timeout(),
            keepalive_interval_secs: default_keepalive(),
            keepalive_max_count: default_keepalive_max(),
            reconnect_max_attempts: default_reconnect_max(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SftpConfig {
    #[serde(default)]
    pub show_hidden_files: bool,
    #[serde(default = "default_true")]
    pub confirm_overwrite: bool,
}

impl Default for SftpConfig {
    fn default() -> Self {
        Self {
            show_hidden_files: false,
            confirm_overwrite: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub folder_id: Option<String>,
    #[serde(default)]
    pub import_origin: Option<ImportOrigin>,
    #[serde(default)]
    pub remember_credentials: bool,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub startup_command: Option<String>,
    #[serde(default)]
    pub environment: HashMap<String, String>,
    /// Shell to use for local PTY sessions (None uses platform default)
    #[serde(default)]
    pub shell: Option<String>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            host: String::new(),
            port: default_port(),
            username: String::new(),
            folder_id: None,
            import_origin: None,
            remember_credentials: false,
            auth: AuthConfig::default(),
            startup_command: None,
            environment: HashMap::new(),
            shell: None,
        }
    }
}

fn default_port() -> u16 {
    22
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AuthConfig {
    Password {
        #[serde(default, skip_serializing)]
        password: Option<String>,
    },
    Key {
        #[serde(default)]
        key_path: String,
        #[serde(default, skip_serializing)]
        passphrase: Option<String>,
    },
    Agent,
}

impl fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Password { .. } => f
                .debug_struct("Password")
                .field("password", &"[REDACTED]")
                .finish(),
            Self::Key { key_path, .. } => f
                .debug_struct("Key")
                .field("key_path", key_path)
                .field("passphrase", &"[REDACTED]")
                .finish(),
            Self::Agent => f.write_str("Agent"),
        }
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        AuthConfig::Password { password: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_defaults_use_menlo_and_preserve_configured_font_and_theme() {
        assert_eq!(AppConfig::default().font_family, "Menlo");
        let defaults: AppConfig = toml::from_str("").unwrap();
        assert_eq!(defaults.font_family, "Menlo");
        assert_eq!(defaults.theme, "dark");
        let configured: AppConfig = toml::from_str(
            r#"font_family = "JetBrains Mono"
theme = "light"
"#,
        )
        .unwrap();
        assert_eq!(configured.font_family, "JetBrains Mono");
        assert_eq!(configured.theme, "light");
    }

    #[test]
    fn session_defaults_use_ssh_port_and_do_not_remember_credentials() {
        let defaults = SessionConfig::default();
        let decoded: SessionConfig = toml::from_str("").unwrap();
        for session in [defaults, decoded] {
            assert_eq!(session.port, 22);
            assert!(session.folder_id.is_none());
            assert!(session.import_origin.is_none());
            assert!(!session.remember_credentials);
        }
    }

    #[test]
    fn password_deserializes_but_is_never_serialized_or_debugged() {
        let auth: AuthConfig = toml::from_str(
            r#"type = "password"
password = "legacy-password-value"
"#,
        )
        .unwrap();
        assert!(matches!(
            &auth,
            AuthConfig::Password { password: Some(password) }
                if password == "legacy-password-value"
        ));
        let encoded = toml::to_string(&auth).unwrap();
        let value: toml::Value = toml::from_str(&encoded).unwrap();
        assert!(value.get("password").is_none());
        assert!(!format!("{auth:?}").contains("legacy-password-value"));
    }

    #[test]
    fn key_passphrase_deserializes_but_only_path_is_serialized() {
        let auth: AuthConfig = toml::from_str(
            r#"type = "key"
key_path = "~/.ssh/id_ed25519"
passphrase = "legacy-passphrase-value"
"#,
        )
        .unwrap();
        assert!(matches!(
            &auth,
            AuthConfig::Key { passphrase: Some(passphrase), .. }
                if passphrase == "legacy-passphrase-value"
        ));
        let encoded = toml::to_string(&auth).unwrap();
        let value: toml::Value = toml::from_str(&encoded).unwrap();
        assert_eq!(value["key_path"].as_str(), Some("~/.ssh/id_ed25519"));
        assert!(value.get("passphrase").is_none());
        assert!(!format!("{auth:?}").contains("legacy-passphrase-value"));
    }

    #[test]
    fn agent_authentication_round_trips() {
        let encoded = toml::to_string(&AuthConfig::Agent).unwrap();
        assert!(matches!(
            toml::from_str::<AuthConfig>(&encoded).unwrap(),
            AuthConfig::Agent
        ));
    }

    #[test]
    fn saved_host_metadata_round_trips_without_secret() {
        let session = SessionConfig {
            id: "profile-1".into(),
            name: "Production".into(),
            host: "server.example".into(),
            port: 2222,
            username: "admin".into(),
            folder_id: Some("operations".into()),
            import_origin: Some(ImportOrigin {
                source_path: "/fixture/ssh-config".into(),
                alias: "production".into(),
            }),
            remember_credentials: true,
            auth: AuthConfig::Password {
                password: Some("transient-password-value".into()),
            },
            startup_command: Some("pwd".into()),
            environment: HashMap::from([("LANG".into(), "en_US.UTF-8".into())]),
            shell: Some("/bin/sh".into()),
        };
        let encoded = toml::to_string(&session).unwrap();
        assert!(!encoded.contains("transient-password-value"));
        assert!(!format!("{session:?}").contains("transient-password-value"));
        let decoded: SessionConfig = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.id, session.id);
        assert_eq!(decoded.name, session.name);
        assert_eq!(decoded.host, session.host);
        assert_eq!(decoded.port, session.port);
        assert_eq!(decoded.username, session.username);
        assert_eq!(decoded.folder_id, session.folder_id);
        assert_eq!(decoded.import_origin, session.import_origin);
        assert!(decoded.remember_credentials);
        assert_eq!(decoded.startup_command, session.startup_command);
        assert_eq!(decoded.environment, session.environment);
        assert_eq!(decoded.shell, session.shell);
        assert!(matches!(
            decoded.auth,
            AuthConfig::Password { password: None }
        ));
    }
}
