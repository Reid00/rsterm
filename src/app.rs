// src/app.rs
//! Main application structure

use std::collections::{HashMap, VecDeque};
use std::fs;

use eframe::egui;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::backend::{AppCommand, BackendEvent, SshAuthMethod, SshConnectionConfig};
use crate::config::{AppSettings, ConfigManager, Theme};
use crate::state::{AuthMethodUi, ConnectionState, ModalDialog, Tab, TabKind};
use crate::terminal::Terminal;

pub type SessionId = String;

/// Actions for the context menu
enum ContextMenuAction {
    Disconnect(String),
    Reconnect(usize, TabKind),
    CloseTab(usize, String),
}

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

    // Context menu state
    context_menu_tab_id: Option<u64>,
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
            session_panel_width: 200.0,
            context_menu_tab_id: None,
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
                    session_id,
                    status: ConnectionState::Connecting,
                };
                self.tabs.push(tab);
                self.active_tab_idx = Some(self.tabs.len() - 1);
                self.tab_id_counter += 1;
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
            self.create_ssh_session_from_dialog();
            should_close = true;
        }

        // Handle close
        if should_close {
            self.modal = ModalDialog::None;
        }
    }

    /// Create an SSH session from the dialog values
    fn create_ssh_session_from_dialog(&mut self) {
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
                    session_id,
                    status: ConnectionState::Connecting,
                };
                self.tabs.push(tab);
                self.active_tab_idx = Some(self.tabs.len() - 1);
                self.tab_id_counter += 1;
            }
            Err(e) => {
                error!("Failed to send CreateSshSession command: {}", e);
                self.notifications
                    .push_back(format!("Failed to create SSH session: {}", e));
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Pull events from backend
        while let Ok(event) = self.event_rx.try_recv() {
            self.handle_backend_event(event);
        }

        // Check pending session responses
        let mut failed_sessions = Vec::new();
        for (session_id, respond_rx) in &mut self.pending_sessions {
            if let Ok(result) = respond_rx.try_recv() {
                if let Err(e) = result {
                    error!("Failed to create session {}: {}", session_id, e);
                    self.notifications.push_back(format!("Failed to create session: {}", e));
                    failed_sessions.push(session_id.clone());
                }
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
            });
        });

        egui::SidePanel::left("sessions")
            .default_width(self.session_panel_width)
            .show(ctx, |ui| {
                ui.heading("Sessions");
                ui.separator();

                if self.tabs.is_empty() {
                    ui.label("No sessions");
                    ui.label("Press Ctrl+Shift+T");
                } else {
                    // Collect tab info first to avoid borrowing issues
                    let tab_info: Vec<(usize, u64, String, TabKind, ConnectionState)> = self.tabs
                        .iter()
                        .enumerate()
                        .map(|(i, t)| (i, t.id, t.title.clone(), t.kind.clone(), t.status.clone()))
                        .collect();

                    for (i, tab_id, title, kind, status) in tab_info {
                        let is_active = self.active_tab_idx == Some(i);

                        // Create a unique ID for this tab item
                        let _tab_id_str = format!("tab_{}", tab_id);

                        // Build the tab label with status indicator and type badge
                        let (status_indicator, status_color) = match &status {
                            ConnectionState::Connecting => ("*".to_string(), egui::Color32::from_rgb(255, 200, 0)),
                            ConnectionState::Connected { .. } => ("*".to_string(), egui::Color32::from_rgb(0, 200, 0)),
                            ConnectionState::Disconnected => ("*".to_string(), egui::Color32::from_rgb(200, 50, 50)),
                            ConnectionState::Reconnecting { attempt, .. } => {
                                (format!("R{}", attempt), egui::Color32::from_rgb(255, 150, 0))
                            }
                            ConnectionState::Error { .. } => ("!".to_string(), egui::Color32::from_rgb(255, 50, 50)),
                        };

                        // Type badge
                        let (type_badge, type_color) = match &kind {
                            TabKind::Local { .. } => ("L", egui::Color32::from_rgb(100, 150, 200)),
                            TabKind::Ssh { .. } => ("S", egui::Color32::from_rgb(150, 100, 200)),
                        };

                        // Draw the tab item with interactive area
                        let response = ui.selectable_label(is_active, "");
                        let rect = response.rect;

                        // Custom painting for the tab item
                        if is_active {
                            ui.painter().rect_filled(
                                rect,
                                0.0,
                                egui::Color32::from_rgba_unmultiplied(60, 120, 200, 50),
                            );
                        }

                        let painter = ui.painter_at(rect);

                        // Layout: [status] [type] [title]
                        let mut x_offset = rect.min.x + 4.0;
                        let y_center = rect.center().y;

                        // Status indicator (animated for connecting)
                        let indicator_text = match &status {
                            ConnectionState::Connecting => {
                                // Animate dots based on time
                                let time = ui.input(|i| i.time);
                                let dots = ((time * 2.0) as i32 % 4) as usize;
                                let dot_str = ".".repeat(dots);
                                format!("{}{}", status_indicator, dot_str)
                            }
                            _ => status_indicator.clone(),
                        };

                        painter.text(
                            egui::pos2(x_offset, y_center),
                            egui::Align2::LEFT_CENTER,
                            indicator_text,
                            egui::FontId::proportional(12.0),
                            status_color,
                        );
                        x_offset += 12.0;

                        // Type badge
                        let badge_rect = egui::Rect::from_min_size(
                            egui::pos2(x_offset, rect.min.y + 2.0),
                            egui::vec2(14.0, rect.height() - 4.0),
                        );
                        painter.rect_filled(badge_rect, 2.0, type_color);
                        painter.text(
                            badge_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            type_badge,
                            egui::FontId::proportional(10.0),
                            egui::Color32::WHITE,
                        );
                        x_offset += 18.0;

                        // Title text
                        let title_text = if is_active {
                            egui::RichText::new(&title).strong()
                        } else {
                            egui::RichText::new(&title)
                        };
                        painter.text(
                            egui::pos2(x_offset, y_center),
                            egui::Align2::LEFT_CENTER,
                            title_text.text(),
                            egui::FontId::proportional(12.0),
                            ui.style().visuals.text_color(),
                        );

                        // Handle click
                        if response.clicked() {
                            self.active_tab_idx = Some(i);
                        }

                        // Handle right-click for context menu
                        if response.secondary_clicked() {
                            self.context_menu_tab_id = Some(tab_id);
                        }

                        // Handle double-click to rename (optional - placeholder for MVP)
                        if response.double_clicked() {
                            // TODO: Implement rename functionality
                            tracing::info!("Double-clicked tab {} for rename", tab_id);
                        }
                    }
                }

                // Render context menu if open
                if let Some(menu_tab_id) = self.context_menu_tab_id {
                    let mut close_menu = false;
                    let mut action: Option<ContextMenuAction> = None;

                    // Find the tab info
                    if let Some((idx, tab)) = self.tabs.iter().enumerate().find(|(_, t)| t.id == menu_tab_id) {
                        let tab_status = tab.status.clone();
                        let tab_session_id = tab.session_id.clone();

                        egui::Window::new("Context Menu")
                            .title_bar(false)
                            .resizable(false)
                            .collapsible(false)
                            .anchor(egui::Align2::LEFT_TOP, egui::vec2(0.0, 0.0))
                            .show(ctx, |ui| {
                                ui.set_min_width(120.0);

                                // Show appropriate options based on connection state
                                match &tab_status {
                                    ConnectionState::Connected { .. } => {
                                        if ui.button("Disconnect").clicked() {
                                            action = Some(ContextMenuAction::Disconnect(tab_session_id.clone()));
                                            close_menu = true;
                                        }
                                    }
                                    ConnectionState::Disconnected | ConnectionState::Error { .. } => {
                                        if ui.button("Reconnect").clicked() {
                                            action = Some(ContextMenuAction::Reconnect(idx, tab.kind.clone()));
                                            close_menu = true;
                                        }
                                    }
                                    ConnectionState::Connecting => {
                                        ui.label("Connecting...");
                                    }
                                    ConnectionState::Reconnecting { .. } => {
                                        ui.label("Reconnecting...");
                                    }
                                }

                                if ui.button("Close Tab").clicked() {
                                    action = Some(ContextMenuAction::CloseTab(idx, tab_session_id.clone()));
                                    close_menu = true;
                                }

                                // Click outside to close
                                if ui.button("Cancel").clicked() {
                                    close_menu = true;
                                }
                            });

                        // Handle actions
                        if let Some(act) = action {
                            match act {
                                ContextMenuAction::Disconnect(session_id) => {
                                    let _ = self.command_tx.try_send(AppCommand::CloseSession {
                                        session_id,
                                    });
                                }
                                ContextMenuAction::Reconnect(idx, kind) => {
                                    // For reconnect, we need to create a new session with the same config
                                    // For now, handle based on tab kind
                                    match kind {
                                        TabKind::Local { shell } => {
                                            self.create_local_session(if shell == "default" { None } else { Some(&shell) });
                                            // Remove the old tab
                                            if idx < self.tabs.len() {
                                                self.tabs.remove(idx);
                                                self.active_tab_idx = if self.tabs.is_empty() {
                                                    None
                                                } else {
                                                    Some(self.tabs.len().saturating_sub(1))
                                                };
                                            }
                                        }
                                        TabKind::Ssh { .. } => {
                                            // For SSH, we'd need to store credentials or prompt again
                                            // For now, show notification
                                            self.notifications.push_back(
                                                "Reconnect for SSH requires re-entering credentials. Use New SSH Session.".to_string()
                                            );
                                        }
                                    }
                                }
                                ContextMenuAction::CloseTab(idx, session_id) => {
                                    // Send close command
                                    let _ = self.command_tx.try_send(AppCommand::CloseSession {
                                        session_id: session_id.clone(),
                                    });
                                    // Remove session and tab
                                    self.sessions.remove(&session_id);
                                    if idx < self.tabs.len() {
                                        self.tabs.remove(idx);
                                        self.active_tab_idx = if self.tabs.is_empty() {
                                            None
                                        } else {
                                            Some(idx.saturating_sub(1).min(self.tabs.len() - 1))
                                        };
                                    }
                                }
                            }
                        }
                    }

                    // Close menu on click outside
                    if ctx.input(|i| i.pointer.any_click()) && ctx.dragged_id().is_none() {
                        // Check if we clicked outside the context menu
                        close_menu = true;
                    }

                    if close_menu {
                        self.context_menu_tab_id = None;
                    }
                }
            });

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

                        // Allocate space for terminal
                        let terminal_rect = ui.available_rect_before_wrap();
                        let response = ui.allocate_rect(terminal_rect, egui::Sense::click());

                        // Request focus on click
                        if response.clicked() {
                            response.request_focus();
                        }

                        // Handle keyboard input when focused
                        if response.has_focus() {
                            ui.input(|i| {
                                for event in &i.raw.events {
                                    match event {
                                        egui::Event::Key { key, modifiers, pressed: true, .. } => {
                                            self.handle_key_input(*key, *modifiers);
                                        }
                                        egui::Event::Text(text) => {
                                            self.handle_text_input(text);
                                        }
                                        _ => {}
                                    }
                                }
                            });
                        }

                        // Draw terminal content
                        let painter = ui.painter_at(terminal_rect);
                        let origin = terminal_rect.min;

                        // Get session again for drawing
                        if let Some(session) = self.sessions.get(&session_id) {
                            let grid = session.terminal.grid();

                            for row in 0..rows {
                                for col in 0..cols {
                                    let cell = grid.cell(row, col);
                                    let x = col as f32 * col_width;
                                    let y = row as f32 * row_height;

                                    // Draw cell background
                                    if !cell.bg.is_default() {
                                        let bg_color = cell.bg.to_egui();
                                        painter.rect_filled(
                                            egui::Rect::from_min_size(
                                                origin + egui::vec2(x, y),
                                                egui::vec2(col_width, row_height),
                                            ),
                                            0.0,
                                            bg_color,
                                        );
                                    }

                                    // Draw character
                                    let ch = cell.char;
                                    if ch != '\0' && ch != ' ' {
                                        let fg_color = cell.fg.to_egui();
                                        painter.text(
                                            origin + egui::vec2(x, y),
                                            egui::Align2::LEFT_TOP,
                                            ch.to_string(),
                                            font_id.clone(),
                                            fg_color,
                                        );
                                    }
                                }
                            }

                            // Draw cursor
                            let cursor_x = cursor_col as f32 * col_width;
                            let cursor_y = cursor_row as f32 * row_height;
                            painter.rect_filled(
                                egui::Rect::from_min_size(
                                    origin + egui::vec2(cursor_x, cursor_y),
                                    egui::vec2(col_width, row_height),
                                ),
                                0.0,
                                egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180),
                            );
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
            _ => {}
        }
    }
}