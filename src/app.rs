// src/app.rs
//! Main application structure

use std::collections::VecDeque;

use eframe::egui;
use tokio::sync::mpsc;

use crate::channel::AppEvent;
use crate::config::{AppSettings, ConfigManager, Theme};
use crate::state::{ModalDialog, Tab};

pub struct App {
    // Channels
    command_tx: mpsc::Sender<crate::channel::AppCommand>,
    event_rx: mpsc::Receiver<AppEvent>,
    _runtime: tokio::runtime::Runtime,

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
        command_tx: mpsc::Sender<crate::channel::AppCommand>,
        event_rx: mpsc::Receiver<AppEvent>,
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
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Pull events
        while let Ok(_event) = self.event_rx.try_recv() {
            // TODO: handle events
        }

        // Render UI
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.label("RSTerm - Toolbar (TODO)");
        });

        egui::SidePanel::left("sessions")
            .default_width(self.session_panel_width)
            .show(ctx, |ui| {
                ui.label("Sessions Panel (TODO)");
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.tabs.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(100.0);
                    ui.heading("Welcome to RSTerm");
                    ui.label("Press Ctrl+Shift+T to open a local terminal");
                });
            } else {
                ui.label("Terminal tabs (TODO)");
            }
        });
    }
}