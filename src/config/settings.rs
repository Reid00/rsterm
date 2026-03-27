// src/config/settings.rs
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    #[serde(default = "default_true")]
    pub local_group_open: bool,
    #[serde(default = "default_true")]
    pub ssh_group_open: bool,
}

fn default_session_panel_width() -> f32 { 200.0 }
fn default_sftp_panel_width() -> f32 { 200.0 }

impl Default for UiStateConfig {
    fn default() -> Self {
        Self {
            session_panel_width: default_session_panel_width(),
            sftp_panel_width: default_sftp_panel_width(),
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

fn default_theme() -> String { "dark".into() }
fn default_font() -> String { "JetBrains Mono".into() }
fn default_font_size() -> f32 { 14.0 }

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

fn default_scroll_buffer() -> usize { 10000 }
fn default_cursor_style() -> String { "block".into() }
fn default_true() -> bool { true }

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

fn default_timeout() -> u64 { 30 }
fn default_keepalive() -> u64 { 30 }
fn default_keepalive_max() -> u32 { 3 }
fn default_reconnect_max() -> u32 { 3 }

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

// Session configuration
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
    /// Shell to use for local PTY sessions (None uses platform default)
    #[serde(default)]
    pub shell: Option<String>,
}

fn default_port() -> u16 { 22 }

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