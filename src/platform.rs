// src/platform.rs
//! Platform abstraction layer for cross-platform support

use std::path::PathBuf;

/// Get the default shell for the current platform
pub fn default_shell() -> String {
    #[cfg(target_os = "windows")]
    {
        if which::which("pwsh").is_ok() {
            "pwsh".into()
        } else {
            "powershell.exe".into()
        }
    }

    #[cfg(target_os = "macos")]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into())
    }

    #[cfg(target_os = "linux")]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
    }
}

/// Get common SSH key paths to search
pub fn ssh_key_search_paths() -> Vec<PathBuf> {
    let ssh_dir = dirs::home_dir()
        .map(|h| h.join(".ssh"))
        .unwrap_or_default();

    vec![
        ssh_dir.join("id_ed25519"),
        ssh_dir.join("id_rsa"),
        ssh_dir.join("id_ecdsa"),
    ]
}

/// Expand a path string, handling ~ and environment variables
pub fn expand_path(path: &str) -> PathBuf {
    if let Some(home) = dirs::home_dir() {
        if path == "~" {
            return home;
        }
        if let Some(rest) = path.strip_prefix("~/") {
            return home.join(rest);
        }
    }

    #[cfg(target_os = "windows")]
    {
        shellexpand::full(path)
            .map(|s| PathBuf::from(s.as_ref()))
            .unwrap_or_else(|_| PathBuf::from(path))
    }

    #[cfg(not(target_os = "windows"))]
    {
        PathBuf::from(path)
    }
}

/// Get the environment variables to pass to PTY processes
#[allow(unused_mut)]
pub fn pty_environment() -> Vec<(String, String)> {
    let mut env = vec![
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
    ];

    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(lang) = std::env::var("LANG") {
            env.push(("LANG".into(), lang));
        } else {
            env.push(("LANG".into(), "en_US.UTF-8".into()));
        }
    }

    env
}

/// Open a URL in the system's default browser
pub fn open_url(url: &str) -> Result<(), std::io::Error> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", url])
            .spawn()?;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()?;
    }

    Ok(())
}