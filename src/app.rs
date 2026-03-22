// src/app.rs
//! Main application structure

use std::collections::{HashMap, VecDeque};

use eframe::egui;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::backend::{AppCommand, BackendEvent};
use crate::config::{AppSettings, ConfigManager, Theme};
use crate::state::{ConnectionState, ModalDialog, Tab, TabKind};
use crate::terminal::Terminal;

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
            });
        });

        egui::SidePanel::left("sessions")
            .default_width(self.session_panel_width)
            .show(ctx, |ui| {
                ui.heading("Sessions");
                ui.separator();
                for (i, tab) in self.tabs.iter().enumerate() {
                    let is_active = self.active_tab_idx == Some(i);
                    if ui.selectable_label(is_active, &tab.title).clicked() {
                        self.active_tab_idx = Some(i);
                    }
                }
                if self.tabs.is_empty() {
                    ui.label("No sessions");
                    ui.label("Press Ctrl+Shift+T");
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.tabs.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(100.0);
                    ui.heading("Welcome to RSTerm");
                    ui.label("Press Ctrl+Shift+T to open a local terminal");
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
    }
}