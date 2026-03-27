// src/app.rs
//! Main application structure

use std::collections::{HashMap, VecDeque};
use std::fs;

use eframe::egui;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::backend::{AppCommand, BackendEvent, SshAuthMethod, SshConnectionConfig};
use crate::config::{AppSettings, ConfigManager, Theme};
use crate::state::{AuthMethodUi, ConnectionState, FileEntryInfo, ModalDialog, SftpPanelState, SftpTransfer, Tab, TabKind, TransferDirection};
use crate::terminal_v2::{CellFlags, Terminal};

pub type SessionId = String;

/// A terminal session with its associated terminal state
pub struct TerminalSession {
    pub session_id: SessionId,
    pub terminal: Terminal,
}

impl TerminalSession {
    pub fn new(session_id: SessionId, cols: usize, rows: usize) -> Self {
        Self {
            session_id,
            terminal: Terminal::new(cols, rows),
        }
    }
}

pub struct App {
    // Channels
    command_tx: mpsc::Sender<AppCommand>,
    event_rx: mpsc::Receiver<BackendEvent>,
    _runtime: tokio::runtime::Runtime,

    // Sessions
    sessions: HashMap<SessionId, TerminalSession>,
    pending_sessions: HashMap<SessionId, mpsc::Receiver<Result<String, String>>>,

    // Tabs
    tabs: Vec<Tab>,
    active_tab_idx: Option<usize>,
    tab_id_counter: u64,

    // Config
    config: ConfigManager,
    settings: AppSettings,
    theme: Theme,

    // UI State
    modal: ModalDialog,
    notifications: VecDeque<String>,
    session_panel_width: f32,
    sftp_panel_width: f32,

    // Session panel state
    renaming_tab: Option<u64>,
    rename_buffer: String,
    local_group_open: bool,
    ssh_group_open: bool,

    // Focus state
    /// If set, request focus on this session's terminal on the next frame
    focus_session_on_next_frame: Option<String>,

    // SFTP state
    sftp_transfers: Vec<SftpTransfer>,
    _sftp_transfer_id_counter: u64,
}

impl App {
    pub fn new(
        command_tx: mpsc::Sender<AppCommand>,
        event_rx: mpsc::Receiver<BackendEvent>,
        runtime: tokio::runtime::Runtime,
    ) -> Self {
        let config = ConfigManager::new().expect("Failed to initialize config");
        let settings = config.load_settings().unwrap_or_default();
        let theme = config.load_theme(&settings.app.theme)
            .unwrap_or_else(|_| Theme::default_dark());

        // Load UI state from settings
        let session_panel_width = settings.ui.session_panel_width;
        let sftp_panel_width = settings.ui.sftp_panel_width;
        let local_group_open = settings.ui.local_group_open;
        let ssh_group_open = settings.ui.ssh_group_open;

        Self {
            command_tx,
            event_rx,
            _runtime: runtime,
            sessions: HashMap::new(),
            pending_sessions: HashMap::new(),
            tabs: vec![],
            active_tab_idx: None,
            tab_id_counter: 0,
            config,
            settings,
            theme,
            modal: ModalDialog::None,
            notifications: VecDeque::new(),
            session_panel_width,
            sftp_panel_width,
            renaming_tab: None,
            rename_buffer: String::new(),
            local_group_open,
            ssh_group_open,
            focus_session_on_next_frame: None,
            sftp_transfers: Vec::new(),
            _sftp_transfer_id_counter: 0,
        }
    }

    /// Save UI settings to config
    fn save_ui_settings(&self) {
        let mut settings = self.settings.clone();
        settings.ui.session_panel_width = self.session_panel_width;
        settings.ui.sftp_panel_width = self.sftp_panel_width;
        settings.ui.local_group_open = self.local_group_open;
        settings.ui.ssh_group_open = self.ssh_group_open;
        if let Err(e) = self.config.save_settings(&settings) {
            tracing::error!("Failed to save settings: {}", e);
        }
    }

    /// Create a new local terminal session
    fn create_local_session(&mut self, shell: Option<&str>) {
        use crate::config::SessionConfig;
        use tokio::sync::mpsc;

        let session_id = uuid::Uuid::new_v4().to_string();
        let (respond_tx, respond_rx) = mpsc::channel::<Result<String, String>>(1);

        let config = SessionConfig {
            shell: shell.map(String::from),
            ..Default::default()
        };

        let cmd = AppCommand::CreateSession {
            session_id: session_id.clone(),
            config,
            respond_to: respond_tx,
        };

        // Send the command
        match self.command_tx.try_send(cmd) {
            Ok(()) => {
                info!("Sent CreateSession command for {}", session_id);
                // Create terminal session with default size
                let terminal_session = TerminalSession::new(session_id.clone(), 80, 24);
                self.sessions.insert(session_id.clone(), terminal_session);

                // Store the response receiver to check later
                self.pending_sessions.insert(session_id.clone(), respond_rx);

                // Create a tab for this session
                let tab = Tab {
                    id: self.tab_id_counter,
                    title: format!("Terminal {}", self.tab_id_counter + 1),
                    kind: TabKind::Local {
                        shell: shell.unwrap_or("default").to_string(),
                    },
                    session_id: session_id.clone(),
                    status: ConnectionState::Connecting,
                    sftp_panel: SftpPanelState::default(),
                };
                self.tabs.push(tab);
                self.active_tab_idx = Some(self.tabs.len() - 1);
                self.tab_id_counter += 1;
                // Request focus on the new terminal
                self.focus_session_on_next_frame = Some(session_id);
            }
            Err(e) => {
                error!("Failed to send CreateSession command: {}", e);
                self.notifications.push_back(format!("Failed to create session: {}", e));
            }
        }
    }

    /// Handle a backend event
    fn handle_backend_event(&mut self, event: BackendEvent) {
        match event {
            BackendEvent::SessionCreated { session_id } => {
                info!("Session created: {}", session_id);
                // Update tab status if we have one
                for tab in &mut self.tabs {
                    if tab.session_id == session_id {
                        tab.status = ConnectionState::Connected {
                            connected_at: std::time::Instant::now(),
                            server_banner: None,
                        };
                    }
                }
            }
            BackendEvent::SessionOutput { session_id, data } => {
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    session.terminal.process_output(&data);
                }
            }
            BackendEvent::SessionClosed { session_id } => {
                info!("Session closed: {}", session_id);
                // Remove session
                self.sessions.remove(&session_id);
                // Update tab status
                for tab in &mut self.tabs {
                    if tab.session_id == session_id {
                        tab.status = ConnectionState::Disconnected;
                    }
                }
            }
            BackendEvent::SftpReady { session_id, cwd } => {
                info!("SFTP ready for session: {} with cwd: {}", session_id, cwd);
                // Update the SFTP panel with the current working directory and list it
                if let Some(idx) = self.active_tab_idx {
                    if let Some(tab) = self.tabs.get_mut(idx) {
                        if tab.session_id == session_id {
                            // Update the path to the actual working directory
                            tab.sftp_panel.current_path = cwd.clone();
                            tab.sftp_panel.loading = true;
                            tab.sftp_panel.error = None;

                            // Request directory listing
                            let _ = self.command_tx.try_send(AppCommand::SftpListDir {
                                session_id: session_id.clone(),
                                path: cwd,
                            });
                        }
                    }
                }
            }
            BackendEvent::SftpError { session_id, error } => {
                error!("SFTP error for session {}: {}", session_id, error);
                // Update SFTP panel error state for the active tab
                if let Some(idx) = self.active_tab_idx {
                    if let Some(tab) = self.tabs.get_mut(idx) {
                        if tab.session_id == session_id {
                            tab.sftp_panel.error = Some(error);
                            tab.sftp_panel.loading = false;
                        }
                    }
                }
            }
            BackendEvent::SftpListResult { session_id, path, entries } => {
                info!("SFTP list result for {} in session {}: {} entries", path, session_id, entries.len());
                // Update the SFTP panel with the directory listing
                if let Some(idx) = self.active_tab_idx {
                    if let Some(tab) = self.tabs.get_mut(idx) {
                        if tab.session_id == session_id {
                            tab.sftp_panel.current_path = path;
                            tab.sftp_panel.entries = entries;
                            tab.sftp_panel.loading = false;
                            tab.sftp_panel.error = None;
                        }
                    }
                }
            }
            BackendEvent::SftpTransferProgress { session_id: _, transfer_id, bytes_transferred, total_bytes: _ } => {
                // Update transfer progress
                for transfer in &mut self.sftp_transfers {
                    if transfer.id == transfer_id {
                        transfer.update_progress(bytes_transferred);
                    }
                }
            }
            BackendEvent::SftpTransferComplete { session_id: _, transfer_id, success, error } => {
                // Handle transfer completion
                if success {
                    info!("Transfer {} completed successfully", transfer_id);
                } else {
                    error!("Transfer {} failed: {:?}", transfer_id, error);
                }
                // Remove from active transfers
                self.sftp_transfers.retain(|t| t.id != transfer_id);
            }
        }
    }

    /// Handle keyboard input for the focused terminal
    fn handle_key_input(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        // Find the active session
        if let Some(tab_idx) = self.active_tab_idx {
            if let Some(tab) = self.tabs.get(tab_idx) {
                let session_id = tab.session_id.clone();
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    if let Some(data) = session.terminal.handle_key(key, modifiers) {
                        tracing::debug!("Key {:?} -> {} bytes: {:?}", key, data.len(), String::from_utf8_lossy(&data));
                        let _ = self.command_tx.try_send(AppCommand::WriteInput {
                            session_id,
                            data,
                        });
                    }
                }
            }
        }
    }

    /// Handle text input for the focused terminal
    fn handle_text_input(&mut self, text: &str) {
        // Find the active session
        if let Some(tab_idx) = self.active_tab_idx {
            if let Some(tab) = self.tabs.get(tab_idx) {
                let session_id = tab.session_id.clone();
                if let Some(session) = self.sessions.get(&session_id) {
                    let data = session.terminal.handle_text(text);
                    let _ = self.command_tx.try_send(AppCommand::WriteInput {
                        session_id,
                        data,
                    });
                }
            }
        }
    }

    /// Show the new session dialog
    fn show_new_session_dialog(&mut self) {
        self.modal = ModalDialog::NewSession {
            name: String::new(),
            host: String::new(),
            port: 22,
            username: String::new(),
            auth_method: AuthMethodUi::default(),
            password: String::new(),
            key_path: String::new(),
            key_passphrase: String::new(),
            error: None,
        };
    }

    /// Render the connection dialog
    fn render_connection_dialog(&mut self, ctx: &egui::Context) {
        // Check if we have a NewSession dialog
        match &self.modal {
            ModalDialog::NewSession { .. } => {}
            _ => return,
        }

        // No overlay - the Window will be modal by virtue of being the only interactable element
        // when shown at Foreground order

        // Since we need mutable access to self, we'll work with a local copy
        // and update at the end
        let mut should_close = false;
        let mut should_connect = false;

        // Get current values for rendering
        let (name, host, port, username, auth_method, password, key_path, key_passphrase, error) =
            match &mut self.modal {
                ModalDialog::NewSession {
                    name,
                    host,
                    port,
                    username,
                    auth_method,
                    password,
                    key_path,
                    key_passphrase,
                    error,
                } => (
                    name.clone(),
                    host.clone(),
                    *port,
                    username.clone(),
                    auth_method.clone(),
                    password.clone(),
                    key_path.clone(),
                    key_passphrase.clone(),
                    error.clone(),
                ),
                _ => return,
            };

        egui::Window::new("New SSH Session")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_min_width(400.0);

                // Session name
                ui.label("Session Name:");
                let mut name_mut = name.clone();
                ui.text_edit_singleline(&mut name_mut);
                if name_mut != name {
                    if let ModalDialog::NewSession { name, .. } = &mut self.modal {
                        *name = name_mut;
                    }
                }

                ui.add_space(8.0);

                // Host
                ui.label("Host:");
                let mut host_mut = host.clone();
                ui.text_edit_singleline(&mut host_mut);
                if host_mut != host {
                    if let ModalDialog::NewSession { host, .. } = &mut self.modal {
                        *host = host_mut;
                    }
                }

                // Port
                ui.label("Port:");
                let mut port_str = port.to_string();
                if ui.text_edit_singleline(&mut port_str).changed() {
                    if let Ok(new_port) = port_str.parse::<u16>() {
                        if let ModalDialog::NewSession { port, .. } = &mut self.modal {
                            *port = new_port;
                        }
                    }
                }

                // Username
                ui.label("Username:");
                let mut username_mut = username.clone();
                ui.text_edit_singleline(&mut username_mut);
                if username_mut != username {
                    if let ModalDialog::NewSession { username, .. } = &mut self.modal {
                        *username = username_mut;
                    }
                }

                ui.add_space(8.0);

                // Auth method selector
                ui.label("Authentication Method:");
                ui.horizontal(|ui| {
                    let methods = [
                        (AuthMethodUi::Password, "Password"),
                        (AuthMethodUi::KeyFile, "Key File"),
                        (AuthMethodUi::Agent, "SSH Agent"),
                    ];

                    for (method, label) in methods {
                        let is_selected = auth_method == method;
                        if ui.radio(is_selected, label).clicked() {
                            if let ModalDialog::NewSession { auth_method, .. } = &mut self.modal {
                                *auth_method = method;
                            }
                        }
                    }
                });

                ui.add_space(8.0);

                // Auth-specific fields
                match auth_method {
                    AuthMethodUi::Password => {
                        ui.label("Password:");
                        let mut password_mut = password.clone();
                        ui.add(egui::TextEdit::singleline(&mut password_mut).password(true));
                        if password_mut != password {
                            if let ModalDialog::NewSession { password, .. } = &mut self.modal {
                                *password = password_mut;
                            }
                        }
                    }
                    AuthMethodUi::KeyFile => {
                        ui.label("Key Path:");
                        ui.horizontal(|ui| {
                            let mut key_path_mut = key_path.clone();
                            ui.text_edit_singleline(&mut key_path_mut);
                            if key_path_mut != key_path {
                                if let ModalDialog::NewSession { key_path, .. } = &mut self.modal {
                                    *key_path = key_path_mut;
                                }
                            }
                            if ui.button("Browse...").clicked() {
                                if let Some(path) = rfd::FileDialog::new()
                                    .add_filter("Private Key", &["pem", "key", "ppk"])
                                    .pick_file()
                                {
                                    if let ModalDialog::NewSession { key_path, .. } = &mut self.modal {
                                        *key_path = path.to_string_lossy().to_string();
                                    }
                                }
                            }
                        });

                        ui.label("Passphrase (optional):");
                        let mut key_passphrase_mut = key_passphrase.clone();
                        ui.add(egui::TextEdit::singleline(&mut key_passphrase_mut).password(true));
                        if key_passphrase_mut != key_passphrase {
                            if let ModalDialog::NewSession { key_passphrase, .. } = &mut self.modal {
                                *key_passphrase = key_passphrase_mut;
                            }
                        }
                    }
                    AuthMethodUi::Agent => {
                        ui.label("Will use SSH agent for authentication");
                    }
                }

                ui.add_space(8.0);

                // Error display
                if let Some(err) = &error {
                    ui.colored_label(egui::Color32::RED, err);
                    ui.add_space(8.0);
                }

                ui.separator();

                // Buttons
                ui.horizontal(|ui| {
                    if ui.button("Connect").clicked() {
                        // Validate fields
                        let (host, username, auth_method, password, key_path) =
                            match &self.modal {
                                ModalDialog::NewSession {
                                    host, username, auth_method, password, key_path, ..
                                } => (
                                    host.clone(),
                                    username.clone(),
                                    auth_method.clone(),
                                    password.clone(),
                                    key_path.clone(),
                                ),
                                _ => return,
                            };

                        if host.is_empty() {
                            if let ModalDialog::NewSession { error, .. } = &mut self.modal {
                                *error = Some("Host is required".to_string());
                            }
                            return;
                        }

                        if username.is_empty() {
                            if let ModalDialog::NewSession { error, .. } = &mut self.modal {
                                *error = Some("Username is required".to_string());
                            }
                            return;
                        }

                        // Validate auth-specific fields
                        match auth_method {
                            AuthMethodUi::Password => {
                                if password.is_empty() {
                                    if let ModalDialog::NewSession { error, .. } = &mut self.modal {
                                        *error = Some("Password is required".to_string());
                                    }
                                    return;
                                }
                            }
                            AuthMethodUi::KeyFile => {
                                if key_path.is_empty() {
                                    if let ModalDialog::NewSession { error, .. } = &mut self.modal {
                                        *error = Some("Key path is required".to_string());
                                    }
                                    return;
                                }
                            }
                            AuthMethodUi::Agent => {}
                        }

                        should_connect = true;
                    }

                    if ui.button("Cancel").clicked() {
                        should_close = true;
                    }
                });
            });

        // Handle connect
        if should_connect {
            info!("Connecting to SSH from dialog...");
            self.create_ssh_session_from_dialog();
            should_close = true;
            info!("SSH session creation initiated, closing modal");
        }

        // Handle close
        if should_close {
            info!("Closing modal, setting to None");
            self.modal = ModalDialog::None;
        }
    }

    /// Create an SSH session from the dialog values
    fn create_ssh_session_from_dialog(&mut self) {
        info!("create_ssh_session_from_dialog called");
        let (host, port, username, auth_method, password, key_path, key_passphrase) =
            match &self.modal {
                ModalDialog::NewSession {
                    host,
                    port,
                    username,
                    auth_method,
                    password,
                    key_path,
                    key_passphrase,
                    ..
                } => (
                    host.clone(),
                    *port,
                    username.clone(),
                    auth_method.clone(),
                    password.clone(),
                    key_path.clone(),
                    key_passphrase.clone(),
                ),
                _ => return,
            };

        // Build the SSH auth method
        let ssh_auth = match auth_method {
            AuthMethodUi::Password => SshAuthMethod::Password(password),
            AuthMethodUi::KeyFile => {
                // Read the key file
                match fs::read(&key_path) {
                    Ok(private_key) => {
                        let passphrase = if key_passphrase.is_empty() {
                            None
                        } else {
                            Some(key_passphrase)
                        };
                        SshAuthMethod::Key {
                            private_key,
                            passphrase,
                        }
                    }
                    Err(e) => {
                        self.notifications
                            .push_back(format!("Failed to read key file: {}", e));
                        return;
                    }
                }
            }
            AuthMethodUi::Agent => SshAuthMethod::Agent,
        };

        let session_id = uuid::Uuid::new_v4().to_string();
        let (respond_tx, respond_rx) = mpsc::channel::<Result<String, String>>(1);

        let ssh_config = SshConnectionConfig {
            host,
            port,
            username,
            auth: ssh_auth,
            known_hosts: None, // TODO: Load from config
        };

        let cmd = AppCommand::CreateSshSession {
            session_id: session_id.clone(),
            ssh_config,
            respond_to: respond_tx,
        };

        match self.command_tx.try_send(cmd) {
            Ok(()) => {
                info!("Sent CreateSshSession command for {}", session_id);
                let terminal_session = TerminalSession::new(session_id.clone(), 80, 24);
                self.sessions.insert(session_id.clone(), terminal_session);
                self.pending_sessions.insert(session_id.clone(), respond_rx);

                let tab = Tab {
                    id: self.tab_id_counter,
                    title: format!("SSH {}", self.tab_id_counter + 1),
                    kind: TabKind::Ssh {
                        session_config_id: String::new(),
                    },
                    session_id: session_id.clone(),
                    status: ConnectionState::Connecting,
                    sftp_panel: SftpPanelState::default(),
                };
                self.tabs.push(tab);
                self.active_tab_idx = Some(self.tabs.len() - 1);
                self.tab_id_counter += 1;
                // Request focus on the new terminal after modal closes
                self.focus_session_on_next_frame = Some(session_id);
            }
            Err(e) => {
                error!("Failed to send CreateSshSession command: {}", e);
                self.notifications
                    .push_back(format!("Failed to create SSH session: {}", e));
            }
        }
    }

    /// Check if SFTP panel should be visible for the current tab
    fn is_sftp_available(&self) -> bool {
        if let Some(idx) = self.active_tab_idx {
            if let Some(tab) = self.tabs.get(idx) {
                return matches!(tab.kind, TabKind::Ssh { .. })
                    && matches!(tab.status, ConnectionState::Connected { .. });
            }
        }
        false
    }

    /// Toggle SFTP panel visibility
    fn toggle_sftp_panel(&mut self) {
        if let Some(idx) = self.active_tab_idx {
            if let Some(tab) = self.tabs.get_mut(idx) {
                if matches!(tab.kind, TabKind::Ssh { .. }) {
                    tab.sftp_panel.visible = !tab.sftp_panel.visible;
                    // Initialize SFTP and path if opening panel for the first time
                    if tab.sftp_panel.visible {
                        // Set loading state and default path
                        if tab.sftp_panel.current_path.is_empty() {
                            tab.sftp_panel.current_path = "/".to_string();
                        }
                        tab.sftp_panel.loading = true;
                        tab.sftp_panel.error = None;

                        // Send SFTP init command - this will trigger list after ready
                        let session_id = tab.session_id.clone();
                        let _ = self.command_tx.try_send(AppCommand::SftpInit { session_id });
                    }
                }
            }
        }
    }

    /// Navigate to a directory in SFTP panel
    fn sftp_navigate_to(&mut self, path: String) {
        if let Some(idx) = self.active_tab_idx {
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.sftp_panel.current_path = path.clone();
                tab.sftp_panel.loading = true;
                tab.sftp_panel.selected = None;
                tab.sftp_panel.error = None;
                // Send SFTP list command
                let _ = self.command_tx.try_send(AppCommand::SftpListDir {
                    session_id: tab.session_id.clone(),
                    path,
                });
            }
        }
    }

    /// Navigate up one directory in SFTP panel
    fn sftp_navigate_up(&mut self) {
        if let Some(idx) = self.active_tab_idx {
            // Get parent path first
            let parent_path = self.tabs.get(idx).and_then(|tab| {
                let current = &tab.sftp_panel.current_path;
                if current != "/" && !current.is_empty() {
                    current.rfind('/').map(|i| {
                        if i == 0 { "/".to_string() } else { current[..i].to_string() }
                    })
                } else {
                    None
                }
            });

            if let Some(path) = parent_path {
                self.sftp_navigate_to(path);
            }
        }
    }

    /// Render the SFTP panel
    fn render_sftp_panel(&mut self, ctx: &egui::Context) {
        let active_idx = self.active_tab_idx;
        let is_ssh_session = active_idx.is_some_and(|idx| {
            self.tabs.get(idx).is_some_and(|t| matches!(t.kind, TabKind::Ssh { .. }))
        });

        if !is_ssh_session {
            return;
        }

        // Get panel visibility
        let visible = active_idx.is_some_and(|idx| {
            self.tabs.get(idx).is_some_and(|t| t.sftp_panel.visible)
        });

        if !visible {
            return;
        }

        // Use session panel width as default if sftp_panel_width is 0 or default
        let panel_width = if self.sftp_panel_width <= 0.0 || self.sftp_panel_width == 200.0 {
            self.session_panel_width
        } else {
            self.sftp_panel_width
        };

        let mut panel_width = panel_width;

        egui::SidePanel::right("sftp_panel")
            .default_width(panel_width)
            .min_width(200.0)
            .max_width(600.0)
            .resizable(true)
            .show(ctx, |ui| {
                // Track actual width for persistence
                panel_width = ui.min_rect().width();
                // Header with close button
                ui.horizontal(|ui| {
                    ui.heading("SFTP");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("X").clicked() {
                            self.toggle_sftp_panel();
                        }
                    });
                });

                ui.separator();

                // Address bar
                self.render_sftp_address_bar(ui);

                ui.separator();

                // Toolbar
                self.render_sftp_toolbar(ui);

                ui.separator();

                // File list
                self.render_sftp_file_list(ui);

                ui.separator();

                // Transfer progress
                self.render_sftp_transfers(ui);
            });

        // Save panel width if changed
        if panel_width > 0.0 && panel_width != self.sftp_panel_width {
            self.sftp_panel_width = panel_width;
        }
    }

    /// Render SFTP address bar
    fn render_sftp_address_bar(&mut self, ui: &mut egui::Ui) {
        let active_idx = self.active_tab_idx;

        ui.horizontal(|ui| {
            // Up button
            if ui.small_button("⬆").on_hover_text("Go to parent directory").clicked() {
                self.sftp_navigate_up();
            }

            // Path display/edit
            if let Some(idx) = active_idx {
                if let Some(tab) = self.tabs.get_mut(idx) {
                    let mut path = tab.sftp_panel.current_path.clone();
                    ui.add(egui::TextEdit::singleline(&mut path).desired_width(f32::INFINITY));
                    if path != tab.sftp_panel.current_path {
                        tab.sftp_panel.current_path = path;
                    }
                }
            }

            // Refresh button
            if ui.small_button("🔄").on_hover_text("Refresh directory").clicked() {
                if let Some(idx) = active_idx {
                    if let Some(tab) = self.tabs.get_mut(idx) {
                        tab.sftp_panel.loading = true;
                        // Send SFTP list command for current path
                        let _ = self.command_tx.try_send(AppCommand::SftpListDir {
                            session_id: tab.session_id.clone(),
                            path: tab.sftp_panel.current_path.clone(),
                        });
                    }
                }
            }
        });
    }

    /// Render SFTP toolbar
    fn render_sftp_toolbar(&mut self, ui: &mut egui::Ui) {
        let active_idx = self.active_tab_idx;

        // Get current path and selected item info
        let (current_path, selected_path) = active_idx.map_or((None, None), |idx| {
            self.tabs.get(idx).map(|tab| {
                let selected = tab.sftp_panel.selected.and_then(|s| tab.sftp_panel.entries.get(s));
                (
                    Some(tab.sftp_panel.current_path.clone()),
                    selected.map(|e| e.path.clone()),
                )
            }).unwrap_or((None, None))
        });

        ui.horizontal(|ui| {
            if ui.small_button("📤 Upload").clicked() {
                // Open file picker for upload
                if let Some(session_id) = active_idx.and_then(|idx| self.tabs.get(idx).map(|t| t.session_id.clone())) {
                    if let Some(path) = current_path.clone() {
                        // Use rfd for file dialog (spawn in background)
                        let tx = self.command_tx.clone();
                        let remote_path = path.clone();
                        std::thread::spawn(move || {
                            if let Some(file_path) = rfd::FileDialog::new().pick_file() {
                                let local_path = file_path.to_string_lossy().to_string();
                                let filename = file_path.file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| "upload".to_string());
                                let remote = if remote_path.ends_with('/') {
                                    format!("{}{}", remote_path, filename)
                                } else {
                                    format!("{}/{}", remote_path, filename)
                                };
                                let _ = tx.try_send(AppCommand::SftpUpload {
                                    session_id,
                                    local_path,
                                    remote_path: remote,
                                });
                            }
                        });
                    }
                }
            }
            if ui.small_button("📁 New Folder").clicked() {
                // Show new folder dialog
                if let Some(path) = current_path.clone() {
                    self.modal = ModalDialog::SftpNewFolder {
                        folder_name: String::new(),
                        current_path: path,
                    };
                }
            }
            if ui.small_button("🗑 Delete").clicked() {
                // Delete selected item
                if let Some(path) = selected_path.clone() {
                    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
                    self.modal = ModalDialog::SftpDeleteConfirm {
                        item_name: name,
                        item_path: path,
                    };
                } else {
                    self.notifications.push_back("No item selected".to_string());
                }
            }
        });
    }

    /// Render SFTP file list
    fn render_sftp_file_list(&mut self, ui: &mut egui::Ui) {
        let active_idx = self.active_tab_idx;

        // Get current state info
        let (loading, _has_error, error_msg) = active_idx.map_or((false, false, None), |idx| {
            self.tabs.get(idx).map_or((false, false, None), |t| {
                (t.sftp_panel.loading, t.sftp_panel.error.is_some(), t.sftp_panel.error.clone())
            })
        });

        if loading {
            ui.vertical_centered(|ui| {
                ui.add_space(50.0);
                ui.spinner();
                ui.label("Loading...");
            });
            return;
        }

        if let Some(err) = error_msg {
            ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                ui.colored_label(egui::Color32::RED, &err);
                if ui.button("Retry").clicked() {
                    if let Some(idx) = active_idx {
                        if let Some(tab) = self.tabs.get_mut(idx) {
                            tab.sftp_panel.error = None;
                            tab.sftp_panel.loading = true;
                            // Send SFTP list command
                            let _ = self.command_tx.try_send(AppCommand::SftpListDir {
                                session_id: tab.session_id.clone(),
                                path: tab.sftp_panel.current_path.clone(),
                            });
                        }
                    }
                }
            });
            return;
        }

        // Get entries and selection
        let entries: Vec<FileEntryInfo> = active_idx.map_or(Vec::new(), |idx| {
            self.tabs.get(idx).map_or(Vec::new(), |t| t.sftp_panel.entries.clone())
        });

        egui::ScrollArea::vertical().show(ui, |ui| {
            // Header row
            ui.horizontal(|ui| {
                ui.set_min_width(ui.available_width());
                ui.label(egui::RichText::new("Name").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    ui.label(egui::RichText::new("Size").strong());
                });
            });
            ui.separator();

            if entries.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(30.0);
                    ui.label("Empty directory");
                    ui.label("Use Upload to add files");
                });
                return;
            }

            // File entries
            for (i, entry) in entries.iter().enumerate() {
                let is_selected = active_idx.is_some_and(|idx| {
                    self.tabs.get(idx).is_some_and(|t| t.sftp_panel.selected == Some(i))
                });

                let response = ui.selectable_label(is_selected, "");
                let rect = response.rect;

                // Custom painting
                let painter = ui.painter_at(rect);

                if is_selected {
                    painter.rect_filled(
                        rect,
                        0.0,
                        self.theme.selection_bg(),
                    );
                }

                // Icon and name
                let icon = if entry.is_dir { "📁" } else { "📄" };
                let mut x_offset = rect.min.x + 4.0;
                let y_center = rect.center().y;

                painter.text(
                    egui::pos2(x_offset, y_center),
                    egui::Align2::LEFT_CENTER,
                    icon,
                    egui::FontId::proportional(14.0),
                    egui::Color32::WHITE,
                );
                x_offset += 20.0;

                painter.text(
                    egui::pos2(x_offset, y_center),
                    egui::Align2::LEFT_CENTER,
                    &entry.name,
                    egui::FontId::proportional(12.0),
                    ui.style().visuals.text_color(),
                );

                // Size on right
                let size_text = if entry.is_dir {
                    "<DIR>".to_string()
                } else {
                    format_size(entry.size)
                };
                painter.text(
                    egui::pos2(rect.max.x - 4.0, y_center),
                    egui::Align2::RIGHT_CENTER,
                    size_text,
                    egui::FontId::proportional(11.0),
                    egui::Color32::GRAY,
                );

                // Handle interaction
                if response.clicked() {
                    if let Some(idx) = active_idx {
                        if let Some(tab) = self.tabs.get_mut(idx) {
                            tab.sftp_panel.selected = Some(i);
                        }
                    }
                }

                // Double-click to enter directory
                if response.double_clicked() && entry.is_dir {
                    self.sftp_navigate_to(entry.path.clone());
                }

                // Context menu on right-click
                response.context_menu(|ui| {
                    if ui.button("Download").clicked() {
                        if !entry.is_dir {
                            // Open save dialog for download
                            let remote_path = entry.path.clone();
                            let filename = entry.name.clone();
                            let session_id = active_idx.and_then(|idx| self.tabs.get(idx).map(|t| t.session_id.clone()));
                            if let Some(session_id) = session_id {
                                let tx = self.command_tx.clone();
                                std::thread::spawn(move || {
                                    if let Some(local_path) = rfd::FileDialog::new()
                                        .set_file_name(&filename)
                                        .save_file()
                                    {
                                        let local = local_path.to_string_lossy().to_string();
                                        let _ = tx.try_send(AppCommand::SftpDownload {
                                            session_id,
                                            remote_path,
                                            local_path: local,
                                        });
                                    }
                                });
                            }
                        }
                        ui.close_menu();
                    }
                    if ui.button("Delete").clicked() {
                        let name = entry.name.clone();
                        let path = entry.path.clone();
                        self.modal = ModalDialog::SftpDeleteConfirm {
                            item_name: name,
                            item_path: path,
                        };
                        ui.close_menu();
                    }
                    if ui.button("Rename").clicked() {
                        // TODO: Rename dialog
                        self.notifications.push_back(format!("Rename: {}", entry.name));
                        ui.close_menu();
                    }
                });
            }
        });
    }

    /// Render active SFTP transfers
    fn render_sftp_transfers(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Transfers").strong());

        if self.sftp_transfers.is_empty() {
            ui.label("No active transfers");
            return;
        }

        let mut completed = Vec::new();

        for transfer in &self.sftp_transfers {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    let icon = match transfer.direction {
                        TransferDirection::Upload => "📤",
                        TransferDirection::Download => "📥",
                    };
                    ui.label(icon);
                    ui.label(&transfer.filename);
                });

                // Progress bar
                let progress = transfer.progress.min(1.0);
                let progress_text = format!("{:.0}%", progress * 100.0);
                ui.add(
                    egui::ProgressBar::new(progress)
                        .text(progress_text)
                );

                // Bytes transferred
                let transferred = format_size(transfer.bytes_transferred);
                let total = format_size(transfer.total_bytes);
                ui.label(format!("{} / {}", transferred, total));

                // Mark completed
                if progress >= 1.0 {
                    completed.push(transfer.id);
                }
            });
        }

        // Remove completed transfers after a short delay (for now, immediately)
        for id in completed {
            self.sftp_transfers.retain(|t| t.id != id);
        }
    }
}

/// Format file size in human-readable format
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Pull events from backend and request repaint if there's output
        let mut needs_repaint = false;
        while let Ok(event) = self.event_rx.try_recv() {
            if matches!(event, BackendEvent::SessionOutput { .. }) {
                needs_repaint = true;
            }
            self.handle_backend_event(event);
        }

        // Request continuous repaint while terminal is active
        if needs_repaint {
            ctx.request_repaint();
        }

        // Always request repaint when we have active sessions to ensure responsive terminal
        if !self.sessions.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }

        // Check pending session responses
        let mut failed_sessions = Vec::new();
        for (session_id, respond_rx) in &mut self.pending_sessions {
            if let Ok(Err(e)) = respond_rx.try_recv() {
                error!("Failed to create session {}: {}", session_id, e);
                self.notifications.push_back(format!("Failed to create session: {}", e));
                failed_sessions.push(session_id.clone());
            }
        }
        for session_id in &failed_sessions {
            self.pending_sessions.remove(session_id);
            self.sessions.remove(session_id);
            self.tabs.retain(|t| &t.session_id != session_id);
        }

        // Handle keyboard shortcuts
        ctx.input(|i| {
            // Ctrl+Shift+T: New local terminal
            if i.key_pressed(egui::Key::T)
                && i.modifiers.ctrl
                && i.modifiers.shift
            {
                self.create_local_session(None);
            }

            // Ctrl+W: Close current tab
            if i.key_pressed(egui::Key::W) && i.modifiers.ctrl && !i.modifiers.shift {
                if let Some(idx) = self.active_tab_idx {
                    if let Some(tab) = self.tabs.get(idx).cloned() {
                        let session_id = tab.session_id;
                        let _ = self.command_tx.try_send(AppCommand::CloseSession {
                            session_id: session_id.clone(),
                        });
                        self.sessions.remove(&session_id);
                        self.tabs.remove(idx);
                        if self.tabs.is_empty() {
                            self.active_tab_idx = None;
                        } else if self.active_tab_idx.is_some_and(|a| a >= self.tabs.len()) {
                            self.active_tab_idx = Some(self.tabs.len() - 1);
                        }
                    }
                }
            }

            // Ctrl+Tab: Next tab
            if i.key_pressed(egui::Key::Tab) && i.modifiers.ctrl && !i.modifiers.shift
                && !self.tabs.is_empty() {
                let current = self.active_tab_idx.unwrap_or(0);
                let next = (current + 1) % self.tabs.len();
                self.active_tab_idx = Some(next);
            }

            // Ctrl+Shift+Tab: Previous tab
            if i.key_pressed(egui::Key::Tab) && i.modifiers.ctrl && i.modifiers.shift
                && !self.tabs.is_empty() {
                let current = self.active_tab_idx.unwrap_or(0);
                let prev = if current == 0 { self.tabs.len() - 1 } else { current - 1 };
                self.active_tab_idx = Some(prev);
            }

            // Ctrl+1 through Ctrl+9: Jump to specific tab
            for (idx, key) in [
                egui::Key::Num1, egui::Key::Num2, egui::Key::Num3,
                egui::Key::Num4, egui::Key::Num5, egui::Key::Num6,
                egui::Key::Num7, egui::Key::Num8, egui::Key::Num9,
            ].iter().enumerate() {
                if i.key_pressed(*key) && i.modifiers.ctrl && idx < self.tabs.len() {
                    self.active_tab_idx = Some(idx);
                }
            }

            // Ctrl+Shift+Left: Move current tab left
            if i.key_pressed(egui::Key::ArrowLeft) && i.modifiers.ctrl && i.modifiers.shift {
                if let Some(idx) = self.active_tab_idx {
                    if idx > 0 {
                        self.tabs.swap(idx, idx - 1);
                        self.active_tab_idx = Some(idx - 1);
                    }
                }
            }

            // Ctrl+Shift+Right: Move current tab right
            if i.key_pressed(egui::Key::ArrowRight) && i.modifiers.ctrl && i.modifiers.shift {
                if let Some(idx) = self.active_tab_idx {
                    if idx < self.tabs.len() - 1 {
                        self.tabs.swap(idx, idx + 1);
                        self.active_tab_idx = Some(idx + 1);
                    }
                }
            }
        });

        // Render UI
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("RSTerm");
                if ui.button("New Terminal").clicked() {
                    self.create_local_session(None);
                }
                if ui.button("New SSH Session").clicked() {
                    self.show_new_session_dialog();
                }

                // SFTP toggle button (only visible for SSH sessions)
                if self.is_sftp_available() {
                    let sftp_visible = self.active_tab_idx.is_some_and(|idx| {
                        self.tabs.get(idx).is_some_and(|t| t.sftp_panel.visible)
                    });
                    let btn_text = if sftp_visible { "Hide SFTP" } else { "Show SFTP" };
                    if ui.button(btn_text).clicked() {
                        self.toggle_sftp_panel();
                    }
                }
            });
        });

        // Render SFTP panel BEFORE CentralPanel so it can account for the width
        self.render_sftp_panel(ctx);

        egui::SidePanel::left("sessions")
            .default_width(self.session_panel_width)
            .show(ctx, |ui| {
                // Collect tab info first to avoid borrowing issues
                let tab_info: Vec<(usize, u64, String, TabKind, ConnectionState, String)> = self.tabs
                    .iter()
                    .enumerate()
                    .map(|(i, t)| (i, t.id, t.title.clone(), t.kind.clone(), t.status.clone(), t.session_id.clone()))
                    .collect();

                // Partition tabs by type
                let local_tabs: Vec<_> = tab_info.iter()
                    .filter(|(_, _, _, kind, _, _)| matches!(kind, TabKind::Local { .. }))
                    .collect();
                let ssh_tabs: Vec<_> = tab_info.iter()
                    .filter(|(_, _, _, kind, _, _)| matches!(kind, TabKind::Ssh { .. }))
                    .collect();

                // Deferred actions
                let mut tab_to_close: Option<(usize, String)> = None;
                let mut tab_to_activate: Option<usize> = None;
                let mut tab_to_rename: Option<u64> = None;
                let mut create_local = false;
                let mut create_ssh = false;

                // --- Terminals group ---
                let arrow = if self.local_group_open { "▼" } else { "▶" };
                ui.horizontal(|ui| {
                    if ui.small_button(arrow).clicked() {
                        self.local_group_open = !self.local_group_open;
                        self.save_ui_settings();
                    }
                    ui.strong(format!("Terminals ({})", local_tabs.len()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("+").on_hover_text("New Terminal").clicked() {
                            create_local = true;
                        }
                    });
                });

                if self.local_group_open {
                    if local_tabs.is_empty() {
                        ui.label("  No terminals");
                    } else {
                        for (i, tab_id, title, _kind, status, session_id) in &local_tabs {
                            let is_active = self.active_tab_idx == Some(*i);

                            // Status indicator
                            let status_char = match status {
                                ConnectionState::Connecting => "○",
                                ConnectionState::Connected { .. } => "●",
                                ConnectionState::Disconnected => "○",
                                ConnectionState::Reconnecting { .. } => "◐",
                                ConnectionState::Error { .. } => "✕",
                            };
                            let status_color = match status {
                                ConnectionState::Connecting => egui::Color32::from_rgb(255, 200, 0),  // Yellow/amber for connecting
                                ConnectionState::Connected { .. } => egui::Color32::from_rgb(0, 200, 0),  // Green for connected
                                ConnectionState::Disconnected => egui::Color32::from_rgb(128, 128, 128),  // Gray for disconnected
                                ConnectionState::Reconnecting { .. } => egui::Color32::from_rgb(255, 150, 0),  // Orange for reconnecting
                                ConnectionState::Error { .. } => egui::Color32::from_rgb(
                                    self.theme.ui.error[0],
                                    self.theme.ui.error[1],
                                    self.theme.ui.error[2],
                                ),  // Use theme error color
                            };

                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(status_char).color(status_color).size(14.0));

                                if self.renaming_tab == Some(*tab_id) {
                                    let response = ui.text_edit_singleline(&mut self.rename_buffer);
                                    if response.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == *tab_id) {
                                            tab.title = self.rename_buffer.clone();
                                        }
                                        self.renaming_tab = None;
                                    }
                                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                        self.renaming_tab = None;
                                    }
                                } else {
                                    let title_text = if is_active {
                                        egui::RichText::new(title).strong()
                                    } else {
                                        egui::RichText::new(title)
                                    };
                                    let label_response = ui.selectable_label(is_active, title_text);

                                    if label_response.clicked() {
                                        tab_to_activate = Some(*i);
                                    }

                                    label_response.context_menu(|ui| {
                                        if ui.button("Rename").clicked() {
                                            tab_to_rename = Some(*tab_id);
                                            ui.close_menu();
                                        }
                                        if ui.button("Duplicate").clicked() {
                                            create_local = true;
                                            ui.close_menu();
                                        }
                                        ui.separator();
                                        if ui.button("Close").clicked() {
                                            tab_to_close = Some((*i, session_id.to_string()));
                                            ui.close_menu();
                                        }
                                    });
                                }

                                if ui.small_button(egui::RichText::new("x").monospace()).on_hover_text("Close session").clicked() {
                                    tab_to_close = Some((*i, session_id.to_string()));
                                }
                            });
                        }
                    }
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // --- SSH Sessions group ---
                let arrow = if self.ssh_group_open { "▼" } else { "▶" };
                ui.horizontal(|ui| {
                    if ui.small_button(arrow).clicked() {
                        self.ssh_group_open = !self.ssh_group_open;
                        self.save_ui_settings();
                    }
                    ui.strong(format!("SSH Sessions ({})", ssh_tabs.len()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("+").on_hover_text("New SSH Session").clicked() {
                            create_ssh = true;
                        }
                    });
                });

                if self.ssh_group_open {
                    if ssh_tabs.is_empty() {
                        ui.label("  No SSH sessions");
                    } else {
                        for (i, tab_id, title, _kind, status, session_id) in &ssh_tabs {
                            let is_active = self.active_tab_idx == Some(*i);

                            // Status indicator
                            let status_char = match status {
                                ConnectionState::Connecting => "○",
                                ConnectionState::Connected { .. } => "●",
                                ConnectionState::Disconnected => "○",
                                ConnectionState::Reconnecting { .. } => "◐",
                                ConnectionState::Error { .. } => "✕",
                            };
                            let status_color = match status {
                                ConnectionState::Connecting => egui::Color32::from_rgb(255, 200, 0),  // Yellow/amber for connecting
                                ConnectionState::Connected { .. } => egui::Color32::from_rgb(0, 200, 0),  // Green for connected
                                ConnectionState::Disconnected => egui::Color32::from_rgb(128, 128, 128),  // Gray for disconnected
                                ConnectionState::Reconnecting { .. } => egui::Color32::from_rgb(255, 150, 0),  // Orange for reconnecting
                                ConnectionState::Error { .. } => egui::Color32::from_rgb(
                                    self.theme.ui.error[0],
                                    self.theme.ui.error[1],
                                    self.theme.ui.error[2],
                                ),  // Use theme error color
                            };

                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(status_char).color(status_color).size(14.0));

                                if self.renaming_tab == Some(*tab_id) {
                                    let response = ui.text_edit_singleline(&mut self.rename_buffer);
                                    if response.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == *tab_id) {
                                            tab.title = self.rename_buffer.clone();
                                        }
                                        self.renaming_tab = None;
                                    }
                                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                        self.renaming_tab = None;
                                    }
                                } else {
                                    let title_text = if is_active {
                                        egui::RichText::new(title).strong()
                                    } else {
                                        egui::RichText::new(title)
                                    };
                                    let label_response = ui.selectable_label(is_active, title_text);

                                    if label_response.clicked() {
                                        tab_to_activate = Some(*i);
                                    }

                                    label_response.context_menu(|ui| {
                                        if ui.button("Rename").clicked() {
                                            tab_to_rename = Some(*tab_id);
                                            ui.close_menu();
                                        }
                                        if ui.button("Duplicate").clicked() {
                                            create_ssh = true;
                                            ui.close_menu();
                                        }
                                        if ui.button("Reconnect").clicked() {
                                            tab_to_close = Some((*i, session_id.to_string()));
                                            create_ssh = true;
                                            ui.close_menu();
                                        }
                                        ui.separator();
                                        if ui.button("Close").clicked() {
                                            tab_to_close = Some((*i, session_id.to_string()));
                                            ui.close_menu();
                                        }
                                    });
                                }

                                if ui.small_button(egui::RichText::new("x").monospace()).on_hover_text("Close session").clicked() {
                                    tab_to_close = Some((*i, session_id.to_string()));
                                }
                            });
                        }
                    }
                }

                // Handle deferred actions
                if let Some(idx) = tab_to_activate {
                    self.active_tab_idx = Some(idx);
                }

                if let Some((idx, session_id)) = tab_to_close {
                    let _ = self.command_tx.try_send(AppCommand::CloseSession {
                        session_id: session_id.clone(),
                    });
                    self.sessions.remove(&session_id);
                    self.tabs.remove(idx);
                    if self.tabs.is_empty() {
                        self.active_tab_idx = None;
                    } else if self.active_tab_idx.is_some_and(|a| a >= self.tabs.len()) {
                        self.active_tab_idx = Some(self.tabs.len() - 1);
                    }
                }

                if let Some(tab_id) = tab_to_rename {
                    self.renaming_tab = Some(tab_id);
                    if let Some(tab) = self.tabs.iter().find(|t| t.id == tab_id) {
                        self.rename_buffer = tab.title.clone();
                    }
                }

                if create_local {
                    self.create_local_session(None);
                }
                if create_ssh {
                    self.show_new_session_dialog();
                }

                // Save panel width after the panel has been shown (on next frame)
                let panel_width = ui.min_rect().width();
                if (panel_width - self.session_panel_width).abs() > 5.0 {
                    // Width changed significantly, save after a delay
                }
            });

        // Check if session panel width changed and save (debounced)
        if let Some(rect) = ctx.memory(|m| m.area_rect(egui::Id::new("sessions"))) {
            let new_width = rect.width();
            if (new_width - self.session_panel_width).abs() > 5.0 {
                self.session_panel_width = new_width;
                self.save_ui_settings();
            }
        }

        // Check if SFTP panel width changed and save (debounced)
        if let Some(rect) = ctx.memory(|m| m.area_rect(egui::Id::new("sftp_panel"))) {
            let new_width = rect.width();
            if (new_width - self.sftp_panel_width).abs() > 5.0 {
                self.sftp_panel_width = new_width;
                self.save_ui_settings();
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.tabs.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(100.0);
                    ui.heading("Welcome to RSTerm");
                    ui.label("Press Ctrl+Shift+T to open a local terminal");
                    ui.add_space(10.0);
                    if ui.button("New SSH Session").clicked() {
                        self.show_new_session_dialog();
                    }
                });
            } else if let Some(active_idx) = self.active_tab_idx {
                // Get session ID from the active tab
                let session_id_opt = self.tabs.get(active_idx).map(|t| t.session_id.clone());

                if let Some(session_id) = session_id_opt {
                    // Get terminal data from session (we need to extract what we need to avoid borrowing issues)
                    let terminal_data = self.sessions.get(&session_id).map(|session| {
                        let grid = session.terminal.grid();
                        let cursor = session.terminal.cursor();
                        let rows = grid.row_count();
                        let cols = grid.col_count();
                        (rows, cols, cursor.row, cursor.col)
                    });

                    if let Some((rows, cols, cursor_row, cursor_col)) = terminal_data {
                        let font_id = egui::FontId::monospace(self.settings.app.font_size);
                        let row_height = self.settings.app.font_size * 1.2;
                        let col_width = self.settings.app.font_size * 0.6;

                        // Reserve space for scrollbar on the right
                        let scrollbar_width = 12.0;
                        let full_rect = ui.available_rect_before_wrap();
                        let terminal_rect = egui::Rect::from_min_size(
                            full_rect.min,
                            egui::vec2(full_rect.width() - scrollbar_width, full_rect.height())
                        );
                        let scrollbar_rect = egui::Rect::from_min_size(
                            egui::pos2(full_rect.max.x - scrollbar_width, full_rect.min.y),
                            egui::vec2(scrollbar_width, full_rect.height())
                        );

                        // Calculate terminal size from UI rect and resize if needed
                        let new_cols = (terminal_rect.width() / col_width).floor() as usize;
                        let new_rows = (terminal_rect.height() / row_height).floor() as usize;
                        let new_cols = new_cols.max(2);
                        let new_rows = new_rows.max(2);

                        if new_cols != cols || new_rows != rows {
                            if let Some(session) = self.sessions.get_mut(&session_id) {
                                session.terminal.resize(new_cols, new_rows);
                                // Notify PTY backend of size change
                                let _ = self.command_tx.try_send(AppCommand::ResizeSession {
                                    session_id: session_id.clone(),
                                    cols: new_cols as u16,
                                    rows: new_rows as u16,
                                });
                            }
                        }

                        // Use actual terminal size (may have just been resized)
                        let (rows, cols) = self.sessions.get(&session_id)
                            .map(|s| { let g = s.terminal.grid(); (g.row_count(), g.col_count()) })
                            .unwrap_or((rows, cols));

                        // Use click and key focus for terminal area
                        let terminal_focus_id = egui::Id::new("terminal_focus").with(&session_id);
                        let response = ui.allocate_rect(terminal_rect, egui::Sense::click());

                        // Request focus on click
                        if response.clicked() {
                            ui.memory_mut(|m| m.request_focus(terminal_focus_id));
                        }

                        // Check if we need to auto-focus this terminal (after modal closes)
                        if self.focus_session_on_next_frame.as_deref() == Some(&session_id) {
                            info!("Auto-focusing terminal for session: {}", session_id);
                            ui.memory_mut(|m| m.request_focus(terminal_focus_id));
                            self.focus_session_on_next_frame = None;
                        }

                        // Check if this terminal has focus
                        let has_focus = ui.memory(|m| m.has_focus(terminal_focus_id));

                        // Debug: log focus state
                        if has_focus {
                            tracing::trace!("Terminal {} has focus", session_id);
                        }

                        // Always check for keyboard input when we have an active tab
                        // (Focus might not work correctly in some cases)
                        response.on_hover_cursor(egui::CursorIcon::Text);

                        // Handle keyboard input - process if we have focus OR if no modal is open
                        // This ensures terminal input works even if focus is lost unexpectedly
                        let no_modal_open = matches!(self.modal, ModalDialog::None);
                        let should_process_input = has_focus || no_modal_open;

                        if should_process_input {
                            ui.input(|i| {
                                // Process all key events
                                for event in &i.raw.events {
                                    match event {
                                        egui::Event::Key { key, modifiers, pressed: true, .. } => {
                                            // Handle special keys and Ctrl/Alt combinations
                                            // For terminal, we handle ALL keys when Ctrl is pressed to ensure
                                            // Ctrl+C, Ctrl+D, etc. are sent to the terminal, not consumed by egui
                                            if matches!(key,
                                                egui::Key::ArrowUp | egui::Key::ArrowDown |
                                                egui::Key::ArrowLeft | egui::Key::ArrowRight |
                                                egui::Key::Home | egui::Key::End |
                                                egui::Key::Insert | egui::Key::Delete |
                                                egui::Key::PageUp | egui::Key::PageDown |
                                                egui::Key::Tab | egui::Key::Enter |
                                                egui::Key::Backspace | egui::Key::Escape |
                                                egui::Key::F1 | egui::Key::F2 | egui::Key::F3 |
                                                egui::Key::F4 | egui::Key::F5 | egui::Key::F6 |
                                                egui::Key::F7 | egui::Key::F8 | egui::Key::F9 |
                                                egui::Key::F10 | egui::Key::F11 | egui::Key::F12
                                            ) || modifiers.ctrl || modifiers.alt {
                                                self.handle_key_input(*key, *modifiers);
                                            }
                                        }
                                        egui::Event::Text(text) => {
                                            // Text input (including IME)
                                            self.handle_text_input(text);
                                        }
                                        egui::Event::Ime(ime_event) => {
                                            // Handle IME events for Chinese/Japanese/Korean input
                                            match ime_event {
                                                egui::ImeEvent::Commit(text) => {
                                                    // IME composition finished, send the committed text
                                                    if !text.is_empty() {
                                                        self.handle_text_input(text);
                                                    }
                                                }
                                                // Preedit text is shown by the IME system, we don't need to handle it
                                                // for terminal input as the terminal doesn't support inline composition
                                                egui::ImeEvent::Preedit(_) |
                                                egui::ImeEvent::Enabled |
                                                egui::ImeEvent::Disabled => {}
                                            }
                                        }
                                        // egui converts Ctrl+C to Event::Copy, so we need to handle it
                                        // For terminal, this should send Ctrl+C (ETX, byte 3)
                                        egui::Event::Copy => {
                                            // Send Ctrl+C to terminal
                                            self.handle_key_input(egui::Key::C, egui::Modifiers::CTRL);
                                        }
                                        // egui converts Ctrl+X to Event::Cut
                                        egui::Event::Cut => {
                                            // Send Ctrl+X to terminal
                                            self.handle_key_input(egui::Key::X, egui::Modifiers::CTRL);
                                        }
                                        // Handle paste event - for terminal, we should paste the text directly
                                        egui::Event::Paste(text) => {
                                            self.handle_text_input(text);
                                        }
                                        _ => {}
                                    }
                                }
                            });
                        }

                        // Handle scroll wheel for terminal history
                        let hover_pos = ui.input(|i| i.pointer.hover_pos());
                        let should_scroll = hover_pos.is_some_and(|pos| terminal_rect.contains(pos));

                        if should_scroll {
                            // Use smooth_scroll_delta which egui normalizes across platforms
                            let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
                            if scroll_y != 0.0 {
                                // Convert pixel delta to lines (positive = scroll content up = view history)
                                let scroll_lines = (scroll_y / row_height).round() as i32;
                                if scroll_lines != 0 {
                                    if let Some(session) = self.sessions.get_mut(&session_id) {
                                        session.terminal.scroll(scroll_lines);
                                    }
                                }
                            }
                        }

                        // Draw terminal content - get fresh data after scroll processing
                        let scroll_offset = self.sessions.get(&session_id).map_or(0, |s| s.terminal.scroll_offset());
                        let history_size = self.sessions.get(&session_id).map_or(0, |s| s.terminal.history_size());

                        // Draw terminal content
                        let painter = ui.painter_at(terminal_rect);
                        let origin = terminal_rect.min;

                        // Fill terminal background with theme color
                        painter.rect_filled(
                            terminal_rect,
                            0.0,
                            self.theme.terminal_bg(),
                        );

                        // Get session again for drawing
                        if let Some(session) = self.sessions.get(&session_id) {
                            let grid = session.terminal.grid();

                            for row in 0..rows {
                                let mut col = 0;
                                while col < cols {
                                    let cell = grid.cell(row, col);
                                    let x = col as f32 * col_width;
                                    let y = row as f32 * row_height;

                                    // Skip wide char spacer cells (second cell of double-width char)
                                    if cell.is_wide_spacer {
                                        col += 1;
                                        continue;
                                    }

                                    // Determine cell width (1 or 2 for wide chars)
                                    let cell_width = cell.width as usize;
                                    let render_width = col_width * cell_width as f32;

                                    // Draw cell background if not default
                                    if !cell.bg.is_default() {
                                        let bg_color = cell.bg.to_egui_bg_with_theme(&self.theme);
                                        painter.rect_filled(
                                            egui::Rect::from_min_size(
                                                origin + egui::vec2(x, y),
                                                egui::vec2(render_width, row_height),
                                            ),
                                            0.0,
                                            bg_color,
                                        );
                                    }

                                    // Draw character
                                    let ch = cell.char;
                                    if ch != '\0' && ch != ' ' {
                                        // Get foreground color using theme
                                        let fg_color = if cell.flags.contains(CellFlags::REVERSE) {
                                            self.theme.terminal_bg()
                                        } else {
                                            cell.fg.to_egui_with_theme(&self.theme)
                                        };
                                        painter.text(
                                            origin + egui::vec2(x, y),
                                            egui::Align2::LEFT_TOP,
                                            ch.to_string(),
                                            font_id.clone(),
                                            fg_color,
                                        );
                                    }

                                    col += cell_width;
                                }
                            }

                            // Draw cursor (only when at bottom of scrollback)
                            if scroll_offset == 0 {
                                let cursor_x = cursor_col as f32 * col_width;
                                let cursor_y = cursor_row as f32 * row_height;
                                let cursor_rect = egui::Rect::from_min_size(
                                    origin + egui::vec2(cursor_x, cursor_y),
                                    egui::vec2(col_width, row_height),
                                );
                                painter.rect_filled(
                                    cursor_rect,
                                    0.0,
                                    egui::Color32::from_rgba_unmultiplied(
                                        self.theme.terminal.cursor[0],
                                        self.theme.terminal.cursor[1],
                                        self.theme.terminal.cursor[2],
                                        180,
                                    ),
                                );

                                // Set IME output for proper Chinese/Japanese/Korean input
                                // This tells the OS where to show the IME candidate window
                                let transform = ui
                                    .memory(|m| m.layer_transforms.get(&ui.layer_id()).copied())
                                    .unwrap_or_default();
                                ctx.output_mut(|o| {
                                    o.ime = Some(egui::output::IMEOutput {
                                        rect: transform * terminal_rect,
                                        cursor_rect: transform * cursor_rect,
                                    });
                                });
                            }
                        }

                        // Draw and handle scrollbar
                        if history_size > 0 {
                            let handle_height = (rows as f32 / (history_size + rows) as f32 * scrollbar_rect.height()).max(20.0);
                            let max_handle_offset = scrollbar_rect.height() - handle_height;

                            // scroll_offset = 0 means at bottom, so handle should be at bottom (max offset)
                            // scroll_offset = history_size means at top, so handle should be at top (0 offset)
                            let handle_offset = if history_size > 0 {
                                (1.0 - scroll_offset as f32 / history_size as f32) * max_handle_offset
                            } else {
                                max_handle_offset
                            };

                            let handle_rect = egui::Rect::from_min_size(
                                egui::pos2(scrollbar_rect.min.x, scrollbar_rect.min.y + handle_offset),
                                egui::vec2(scrollbar_width, handle_height)
                            );

                            // Scrollbar background
                            let scrollbar_painter = ui.painter_at(scrollbar_rect);
                            scrollbar_painter.rect_filled(
                                scrollbar_rect,
                                0.0,
                                egui::Color32::from_rgb(self.theme.ui.panel_bg[0], self.theme.ui.panel_bg[1], self.theme.ui.panel_bg[2]),
                            );

                            // Scrollbar handle
                            let scroll_response = ui.allocate_rect(scrollbar_rect, egui::Sense::click_and_drag());
                            let handle_color = if scroll_response.hovered() || scroll_response.dragged() {
                                egui::Color32::from_rgb(self.theme.ui.accent[0], self.theme.ui.accent[1], self.theme.ui.accent[2])
                            } else {
                                egui::Color32::from_rgb(self.theme.ui.tab_inactive_bg[0], self.theme.ui.tab_inactive_bg[1], self.theme.ui.tab_inactive_bg[2])
                            };
                            scrollbar_painter.rect_filled(
                                handle_rect,
                                2.0,
                                handle_color,
                            );

                            // Handle scrollbar interaction
                            if scroll_response.dragged() {
                                if let Some(pointer_pos) = ui.input(|i| i.pointer.interact_pos()) {
                                    // Convert pointer position to scroll offset
                                    // pointer at top (y=0) -> scroll to top (offset = history_size)
                                    // pointer at bottom (y=max) -> scroll to bottom (offset = 0)
                                    let relative_y = (pointer_pos.y - scrollbar_rect.min.y - handle_height / 2.0)
                                        .max(0.0)
                                        .min(max_handle_offset);
                                    let scroll_ratio = 1.0 - relative_y / max_handle_offset;
                                    let new_offset = (scroll_ratio * history_size as f32) as usize;
                                    if let Some(session) = self.sessions.get_mut(&session_id) {
                                        session.terminal.set_scroll_offset(new_offset);
                                    }
                                }
                            }

                            // Click on scrollbar track to jump
                            if scroll_response.clicked() {
                                if let Some(pointer_pos) = ui.input(|i| i.pointer.interact_pos()) {
                                    let relative_y = pointer_pos.y - scrollbar_rect.min.y;
                                    let scroll_ratio = 1.0 - relative_y / scrollbar_rect.height();
                                    let new_offset = (scroll_ratio * history_size as f32) as usize;
                                    if let Some(session) = self.sessions.get_mut(&session_id) {
                                        session.terminal.set_scroll_offset(new_offset);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        // Render modal dialogs
        match &self.modal {
            ModalDialog::NewSession { .. } => {
                self.render_connection_dialog(ctx);
            }
            ModalDialog::SftpNewFolder { .. } => {
                self.render_sftp_new_folder_dialog(ctx);
            }
            ModalDialog::SftpDeleteConfirm { .. } => {
                self.render_sftp_delete_confirm_dialog(ctx);
            }
            _ => {}
        }
    }
}

impl App {
    /// Render the SFTP new folder dialog
    fn render_sftp_new_folder_dialog(&mut self, ctx: &egui::Context) {
        let mut should_close = false;
        let mut create_folder = false;

        // Get mutable access to the modal data
        let (folder_name, current_path) = match &mut self.modal {
            ModalDialog::SftpNewFolder { folder_name, current_path } => (folder_name, current_path),
            _ => return,
        };

        egui::Window::new("New Folder")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Folder name:");
                let response = ui.text_edit_singleline(folder_name);
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    create_folder = true;
                }

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Create").clicked() {
                        create_folder = true;
                    }
                    if ui.button("Cancel").clicked() {
                        should_close = true;
                    }
                });
            });

        if create_folder && !folder_name.is_empty() {
            // Get session ID
            if let Some(idx) = self.active_tab_idx {
                if let Some(tab) = self.tabs.get(idx) {
                    let session_id = tab.session_id.clone();
                    let full_path = if current_path.ends_with('/') {
                        format!("{}{}", current_path, folder_name)
                    } else {
                        format!("{}/{}", current_path, folder_name)
                    };
                    info!("Creating directory: {}", full_path);
                    let _ = self.command_tx.try_send(AppCommand::SftpMkdir {
                        session_id: session_id.clone(),
                        path: full_path,
                    });
                    // Refresh the directory after creating
                    let _ = self.command_tx.try_send(AppCommand::SftpListDir {
                        session_id,
                        path: current_path.clone(),
                    });
                    if let Some(tab) = self.tabs.get_mut(idx) {
                        tab.sftp_panel.loading = true;
                    }
                }
            }
            should_close = true;
        }

        if should_close {
            self.modal = ModalDialog::None;
        }
    }

    /// Render the SFTP delete confirmation dialog
    fn render_sftp_delete_confirm_dialog(&mut self, ctx: &egui::Context) {
        let mut should_close = false;
        let mut do_delete = false;
        let item_name: String;
        let item_path: String;

        if let ModalDialog::SftpDeleteConfirm { item_name: name, item_path: path } = &self.modal {
            item_name = name.clone();
            item_path = path.clone();
        } else {
            return;
        }

        egui::Window::new("Confirm Delete")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("Delete '{}'?", item_name));
                ui.label("This action cannot be undone.");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        do_delete = true;
                    }
                    if ui.button("Cancel").clicked() {
                        should_close = true;
                    }
                });
            });

        if do_delete {
            // Get session ID and current path
            if let Some(idx) = self.active_tab_idx {
                if let Some(tab) = self.tabs.get(idx) {
                    let session_id = tab.session_id.clone();
                    let current_path = tab.sftp_panel.current_path.clone();

                    let _ = self.command_tx.try_send(AppCommand::SftpDelete {
                        session_id: session_id.clone(),
                        path: item_path.clone(),
                    });

                    // Refresh the directory after deleting
                    let _ = self.command_tx.try_send(AppCommand::SftpListDir {
                        session_id,
                        path: current_path.clone(),
                    });
                    if let Some(tab) = self.tabs.get_mut(idx) {
                        tab.sftp_panel.loading = true;
                        tab.sftp_panel.selected = None;
                    }
                }
            }
            should_close = true;
        }

        if should_close {
            self.modal = ModalDialog::None;
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        info!("App dropping, sending shutdown command");
        // Send shutdown command to backend
        let _ = self.command_tx.try_send(AppCommand::Shutdown);
    }
}