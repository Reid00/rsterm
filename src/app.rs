use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};

use gpui_kit::component::{
    button::{Button, ButtonVariants},
    dialog::DialogButtonProps,
    notification::Notification,
    resizable::{h_resizable, resizable_panel, v_resizable, ResizableState},
    tab::{Tab, TabBar},
    ActiveTheme, Disableable, Icon, IconName, Sizable, Theme, ThemeMode, WindowExt,
};
use gpui_kit::{prelude::*, *};
use tokio::sync::{mpsc, oneshot};

use crate::{
    backend::{AppCommand, BackendEvent, SshAuthMethod, SshConnectionConfig},
    config::{AppSettings, AuthConfig, ConfigManager, HostLibrary, HostNodeId, SessionConfig},
    platform,
    ssh::{KnownHosts, SshConfigSource},
    state::ConnectionState,
    ui::{
        connection::{ConnectionAction, ConnectionForm},
        folders::{FolderForm, FolderFormAction, MoveForm, MoveFormAction},
        hosts::{HostAction, HostId, HostsView},
        inspector::{InspectionTarget, InspectorAction, InspectorPage, InspectorView},
        session_tabs::{self, TabIdentity},
        settings::{SettingsAction, SettingsView},
        sftp::{FileBrowserEvent, FileBrowserView},
        shortcuts::ShortcutsView,
        ssh_import::{SshImportAction, SshImportView},
        terminal::TerminalView,
    },
};

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;

pub use crate::shortcuts::{
    CloseTab, FocusHostSearch, FocusTerminal, NewConnection, NewLocalTerminal, NextTab,
    OpenSettings, PreviousTab, Quit, ReconnectSession, SelectLastTab, SelectTab1, SelectTab2,
    SelectTab3, SelectTab4, SelectTab5, SelectTab6, SelectTab7, SelectTab8, ShowHistory,
    ShowNetwork, ShowShortcuts, ShowSystem, ToggleFiles,
};

struct SessionTab {
    id: String,
    session_id: String,
    title: String,
    ordinal: u64,
    profile: Option<SessionConfig>,
    origin: Option<HostId>,
    secret: Option<String>,
    terminal: Entity<TerminalView>,
    files: Option<Entity<FileBrowserView>>,
    files_visible: bool,
    status: ConnectionState,
    retry_attempt: u32,
    user_disconnected: bool,
    shell_error: Option<String>,
}

impl SessionTab {
    fn target(&self) -> String {
        self.profile
            .as_ref()
            .map(|profile| format!("{}@{}:{}", profile.username, profile.host, profile.port))
            .unwrap_or_else(platform::default_shell)
    }
}

enum LibraryMutation {
    CreateFolder {
        name: String,
        parent: Option<String>,
    },
    RenameFolder {
        id: String,
        name: String,
    },
    Move {
        node: HostNodeId,
        target: Option<String>,
    },
    DeleteFolder(String),
}

struct HostChallenge {
    session_id: String,
    host: String,
    port: u16,
    algorithm: String,
    fingerprint: String,
    response: Option<oneshot::Sender<bool>>,
}

pub struct App {
    config: Arc<ConfigManager>,
    settings: AppSettings,
    library: HostLibrary,
    ssh_source: Option<SshConfigSource>,
    hosts: Entity<HostsView>,
    tabs: Vec<SessionTab>,
    active: Option<String>,
    opened_targets: HashMap<String, u64>,
    tab_scroll: ScrollHandle,
    tab_layout: Option<(f32, usize)>,
    inspector: Entity<InspectorView>,
    inspector_visible: bool,
    inspector_panel: Entity<ResizableState>,
    inspector_inline: bool,
    layout_viewport: Option<(f32, f32, bool, bool)>,
    _runtime: Arc<tokio::runtime::Runtime>,
    backend_tx: mpsc::Sender<AppCommand>,
    commands: mpsc::UnboundedSender<AppCommand>,
    preparations: HashMap<String, Task<()>>,
    retries: HashMap<String, Task<()>>,
    challenges: VecDeque<HostChallenge>,
    dialog_open: bool,
    active_challenge: Option<String>,
    loaded: bool,
    legacy_pending: bool,
    profile_busy: bool,
    settings_busy: bool,
    layout_dirty: bool,
    load_error: Option<String>,
    focus: FocusHandle,
    host_panel: Entity<ResizableState>,
    work_panel: Entity<ResizableState>,
    connection_form: Option<Entity<ConnectionForm>>,
    settings_form: Option<Entity<SettingsView>>,
    folder_form: Option<Entity<FolderForm>>,
    move_form: Option<Entity<MoveForm>>,
    import_form: Option<Entity<SshImportView>>,
    _subscriptions: Vec<Subscription>,
    _event_task: Task<()>,
    _command_task: Task<()>,
    _load_task: Option<Task<()>>,
    _profile_task: Option<Task<()>>,
    _settings_task: Option<Task<()>>,
}

impl App {
    pub fn new(
        config: ConfigManager,
        ssh_source: Option<SshConfigSource>,
        backend_tx: mpsc::Sender<AppCommand>,
        mut event_rx: mpsc::Receiver<BackendEvent>,
        runtime: Arc<tokio::runtime::Runtime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let config = Arc::new(config);
        let hosts = cx.new(|cx| HostsView::new(window, cx));
        let host_subscription =
            cx.subscribe_in(&hosts, window, |this, _, event: &HostAction, window, cx| {
                this.host_action(event.clone(), window, cx);
            });
        let (commands, mut command_rx) = mpsc::unbounded_channel::<AppCommand>();
        let inspector = cx.new(|cx| InspectorView::new(commands.clone(), window, cx));
        let inspector_subscription = cx.subscribe_in(
            &inspector,
            window,
            |this, _, event: &InspectorAction, window, cx| {
                this.inspector_action(event, window, cx);
            },
        );
        let sender = backend_tx.clone();
        let command_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(command) = command_rx.recv().await {
                if sender.send(command).await.is_err() {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.error("连接服务已停止，请重新启动应用", window, cx);
                    });
                    break;
                }
            }
        });
        let event_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = event_rx.recv().await {
                let mut events = vec![event];
                while events.len() < 128 {
                    match event_rx.try_recv() {
                        Ok(event) => events.push(event),
                        Err(_) => break,
                    }
                }
                if this
                    .update_in(cx, |this, window, cx| {
                        for event in events {
                            this.handle_event(event, window, cx);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let quit_sender = backend_tx.clone();
        let quit_subscription = cx.on_app_quit(move |this, _| {
            this.preparations.clear();
            this.retries.clear();
            this.challenges.clear();
            let sender = quit_sender.clone();
            async move {
                let _ = sender.send(AppCommand::Shutdown).await;
            }
        });
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = weak.update(cx, |this, _| {
                this.preparations.clear();
                this.retries.clear();
                let _ = this.commands.send(AppCommand::Shutdown);
            });
            cx.quit();
            true
        });
        Self {
            config,
            settings: AppSettings::default(),
            library: HostLibrary::default(),
            ssh_source,
            hosts,
            tabs: Vec::new(),
            active: None,
            opened_targets: HashMap::new(),
            tab_scroll: ScrollHandle::new(),
            tab_layout: None,
            inspector,
            inspector_visible: false,
            inspector_panel: cx.new(|_| ResizableState::default()),
            inspector_inline: true,
            layout_viewport: None,
            _runtime: runtime,
            backend_tx,
            commands,
            preparations: HashMap::new(),
            retries: HashMap::new(),
            challenges: VecDeque::new(),
            dialog_open: false,
            active_challenge: None,
            loaded: false,
            legacy_pending: false,
            profile_busy: false,
            settings_busy: false,
            layout_dirty: false,
            load_error: None,
            focus: cx.focus_handle(),
            host_panel: cx.new(|_| ResizableState::default()),
            work_panel: cx.new(|_| ResizableState::default()),
            connection_form: None,
            settings_form: None,
            folder_form: None,
            move_form: None,
            import_form: None,
            _subscriptions: vec![host_subscription, inspector_subscription, quit_subscription],
            _event_task: event_task,
            _command_task: command_task,
            _load_task: None,
            _profile_task: None,
            _settings_task: None,
        }
    }

    pub fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        if let Some(warning) = crate::shortcuts::warning(cx).map(str::to_owned) {
            window.push_notification(Notification::warning(warning), cx);
        }
        let config = self.config.clone();
        let work = cx.background_spawn(async move {
            Ok::<_, anyhow::Error>((
                config.load_settings()?,
                config.load_library()?,
                config.has_legacy_credentials()?,
            ))
        });
        self._load_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok((settings, library, legacy)) => {
                        this.loaded = true;
                        this.legacy_pending = legacy;
                        this.settings = settings;
                        this.layout_viewport = None;
                        this.library = library;
                        this.sync_hosts(cx);
                        this.apply_settings(window, cx);
                        if legacy {
                            this.show_legacy_choice(window, cx);
                        }
                    }
                    Err(error) => {
                        this.load_error = Some(error.to_string());
                        this.error(format!("无法读取配置：{error}"), window, cx);
                    }
                }
                cx.notify();
            });
        }));
    }

    fn sync_hosts(&mut self, cx: &mut Context<Self>) {
        self.hosts.update(cx, |hosts, cx| {
            hosts.set_library(self.library.clone(), cx);
            hosts.set_busy(
                !self.loaded || self.profile_busy || self.dialog_open || self.legacy_pending,
                cx,
            );
        });
    }

    fn active_index(&self) -> Option<usize> {
        let active = self.active.as_ref()?;
        self.tabs.iter().position(|tab| &tab.id == active)
    }

    fn emit(&self, command: AppCommand, window: &mut Window, cx: &mut Context<Self>) {
        if self.commands.send(command).is_err() {
            self.error("连接服务不可用", window, cx);
        }
    }

    fn error(&self, message: impl Into<String>, window: &mut Window, cx: &mut Context<Self>) {
        window.push_notification(Notification::error(message.into()), cx);
    }

    fn workspace_action_allowed(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        !self.dialog_open
            && !window.has_active_dialog(cx)
            && !self.active_index().is_some_and(|index| {
                let terminal = self.tabs[index].terminal.read(cx);
                terminal.is_focused(window) && terminal.has_marked_text()
            })
    }

    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inspector_visible && !self.inspector_inline && self.active_index().is_some() {
            self.inspector.read(cx).focus_handle().focus(window, cx);
        } else if let Some(index) = self.active_index() {
            let focus = self.tabs[index].terminal.read(cx).focus_handle().clone();
            focus.focus(window, cx);
        } else {
            self.focus.focus(window, cx);
        }
    }

    fn inspection_target(&self) -> Option<InspectionTarget> {
        let tab = &self.tabs[self.active_index()?];
        Some(InspectionTarget {
            tab_id: tab.id.clone(),
            session_id: tab.session_id.clone(),
            label: tab.target(),
            remote: tab.profile.is_some(),
            connected: tab.status.is_connected() && !tab.user_disconnected,
        })
    }

    fn sync_inspector(&mut self, cx: &mut Context<Self>) {
        let target = self.inspection_target();
        self.inspector.update(cx, |inspector, cx| {
            inspector.set_target(target, cx);
            inspector.set_visible(self.inspector_visible, cx);
        });
    }

    fn activate(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.iter().any(|tab| tab.id == id) {
            return;
        }
        self.active = Some(id);
        let origin = self
            .active_index()
            .and_then(|index| self.tabs[index].origin.clone());
        self.hosts.update(cx, |hosts, cx| hosts.select(origin, cx));
        self.sync_inspector(cx);
        if let Some(index) = self.active_index() {
            self.tab_scroll.scroll_to_item(index);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn cycle_tab(&mut self, previous: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_action_allowed(window, cx) {
            return;
        }
        if let Some(index) =
            session_tabs::next_index(self.active_index(), self.tabs.len(), previous)
        {
            self.activate(self.tabs[index].id.clone(), window, cx);
        }
    }

    fn select_tab(&mut self, position: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_action_allowed(window, cx) {
            return;
        }
        let index = if position == 9 {
            self.tabs.len().checked_sub(1)
        } else {
            position
                .checked_sub(1)
                .filter(|index| *index < self.tabs.len())
        };
        if let Some(index) = index {
            self.activate(self.tabs[index].id.clone(), window, cx);
        }
    }

    fn next_ordinal(&mut self, target: String) -> u64 {
        let ordinal = self.opened_targets.entry(target).or_default();
        *ordinal += 1;
        *ordinal
    }

    fn show_inspector(&mut self, page: InspectorPage, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_action_allowed(window, cx) || self.active_index().is_none() {
            return;
        }
        let same_page = self.inspector.read(cx).page() == page;
        if self.inspector_visible && same_page {
            self.hide_inspector(window, cx);
            return;
        }
        let target = self.inspection_target();
        self.inspector_visible = true;
        self.inspector.update(cx, |inspector, cx| {
            inspector.show_page(page, window, cx);
            inspector.set_target(target, cx);
            inspector.set_visible(true, cx);
            inspector.show_page(page, window, cx);
        });
        cx.notify();
    }

    fn hide_inspector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let restore_body = self.inspector_visible && !self.inspector_inline;
        let active = self.active.clone();
        self.inspector_visible = false;
        self.inspector
            .update(cx, |inspector, cx| inspector.set_visible(false, cx));
        if restore_body {
            // GPUI runs frame callbacks before drawing. Wait across a draw so the
            // restored terminal is in the focus tree before focusing it.
            cx.on_next_frame(window, move |_, window, cx| {
                cx.on_next_frame(window, move |this, window, cx| {
                    if !this.inspector_visible
                        && this.active == active
                        && this.workspace_action_allowed(window, cx)
                    {
                        this.focus_active(window, cx);
                    }
                });
            });
        } else {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    fn focus_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_action_allowed(window, cx) {
            return;
        }
        if !self.inspector_inline {
            self.hide_inspector(window, cx);
        } else {
            self.focus_active(window, cx);
        }
    }

    fn inspector_action(
        &mut self,
        action: &InspectorAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            InspectorAction::Close | InspectorAction::FocusTerminal => {
                self.hide_inspector(window, cx)
            }
            InspectorAction::InsertHistory {
                target,
                snapshot_id,
                entry_id,
            } => {
                if !self.workspace_action_allowed(window, cx)
                    || self.inspection_target().as_ref() != Some(target)
                {
                    return;
                }
                if !self.inspector_visible {
                    return;
                }
                let Some(index) = self.active_index() else {
                    return;
                };
                let terminal = self.tabs[index].terminal.clone();
                if !self.inspector_inline {
                    // Keep the snapshot until the terminal body has been drawn, then
                    // recheck the exact target and record before sending any input.
                    self.inspector_visible = false;
                    let target = target.clone();
                    let snapshot_id = *snapshot_id;
                    let entry_id = *entry_id;
                    cx.on_next_frame(window, move |_, window, cx| {
                        cx.on_next_frame(window, move |this, window, cx| {
                            if !this.inspector_visible {
                                this.insert_history(
                                    &target,
                                    snapshot_id,
                                    entry_id,
                                    &terminal,
                                    window,
                                    cx,
                                );
                            }
                            this.sync_inspector(cx);
                        });
                    });
                    cx.notify();
                } else {
                    self.insert_history(target, *snapshot_id, *entry_id, &terminal, window, cx);
                }
            }
        }
    }

    fn insert_history(
        &mut self,
        target: &InspectionTarget,
        snapshot_id: u64,
        entry_id: u64,
        terminal: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_action_allowed(window, cx)
            || self.inspection_target().as_ref() != Some(target)
        {
            return;
        }
        let Some(index) = self.active_index() else {
            return;
        };
        if &self.tabs[index].terminal != terminal {
            return;
        }
        let Some(text) = self
            .inspector
            .read(cx)
            .insertion_text(target, snapshot_id, entry_id)
        else {
            self.error("历史快照已变化，或该记录不适合填入终端", window, cx);
            return;
        };
        let result = terminal.update(cx, |terminal, cx| {
            terminal.insert_history(&text, &target.session_id, window, cx)
        });
        match result {
            Ok(()) => self.focus_terminal(window, cx),
            Err(error) => self.error(error, window, cx),
        }
    }

    fn local_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_action_allowed(window, cx) {
            return;
        }
        let id = uuid::Uuid::new_v4().to_string();
        let ordinal = self.next_ordinal(platform::default_shell());
        let terminal = cx.new(|cx| {
            TerminalView::new(
                id.clone(),
                self.backend_tx.clone(),
                &self.settings,
                window,
                cx,
            )
        });
        self.tabs.push(SessionTab {
            id: id.clone(),
            session_id: id.clone(),
            title: "本地终端".into(),
            ordinal,
            profile: None,
            origin: None,
            secret: None,
            terminal,
            files: None,
            files_visible: false,
            status: ConnectionState::Connecting,
            retry_attempt: 0,
            user_disconnected: false,
            shell_error: None,
        });
        self.emit(
            AppCommand::CreateSession {
                session_id: id.clone(),
                config: SessionConfig::default(),
            },
            window,
            cx,
        );
        self.activate(id, window, cx);
    }

    fn connect_profile(
        &mut self,
        mut profile: SessionConfig,
        secret: Option<String>,
        origin: HostId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.legacy_pending {
            self.show_legacy_choice(window, cx);
            return;
        }
        match &mut profile.auth {
            AuthConfig::Password { password } => *password = None,
            AuthConfig::Key { passphrase, .. } => *passphrase = None,
            AuthConfig::Agent => {}
        }
        let id = uuid::Uuid::new_v4().to_string();
        let ordinal = self.next_ordinal(format!(
            "{}@{}:{}",
            profile.username, profile.host, profile.port
        ));
        let terminal = cx.new(|cx| {
            TerminalView::new(
                id.clone(),
                self.backend_tx.clone(),
                &self.settings,
                window,
                cx,
            )
        });
        self.tabs.push(SessionTab {
            id: id.clone(),
            session_id: id.clone(),
            title: profile.name.clone(),
            ordinal,
            profile: Some(profile),
            origin: Some(origin),
            secret,
            terminal,
            files: None,
            files_visible: false,
            status: ConnectionState::Connecting,
            retry_attempt: 0,
            user_disconnected: false,
            shell_error: None,
        });
        self.activate(id.clone(), window, cx);
        self.prepare_connection(id, window, cx);
    }

    fn prepare_connection(&mut self, tab_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        let Some(profile) = self.tabs[index].profile.clone() else {
            return;
        };
        let secret = self.tabs[index].secret.clone();
        let session_id = self.tabs[index].session_id.clone();
        let config = self.config.clone();
        let settings = self.settings.ssh.clone();
        let work =
            cx.background_spawn(async move { prepare_ssh(&config, &profile, secret, &settings) });
        let key = tab_id.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(index) = this.tabs.iter().position(|tab| {
                    tab.id == tab_id && tab.session_id == session_id && !tab.user_disconnected
                }) else {
                    return;
                };
                match result {
                    Ok((ssh_config, secret)) => {
                        this.tabs[index].secret = secret;
                        this.emit(
                            AppCommand::CreateSshSession {
                                session_id,
                                ssh_config,
                            },
                            window,
                            cx,
                        );
                    }
                    Err(error) => {
                        this.tabs[index].status = ConnectionState::Error(error.to_string());
                        this.error(error.to_string(), window, cx);
                    }
                }
                this.preparations.remove(&tab_id);
                cx.notify();
            });
        });
        self.preparations.insert(key, task);
    }

    fn file_browser(
        &mut self,
        session_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<FileBrowserView> {
        let files = cx.new(|cx| {
            FileBrowserView::new(
                session_id,
                self.backend_tx.clone(),
                &self.settings,
                window,
                cx,
            )
        });
        self._subscriptions.push(cx.subscribe_in(
            &files,
            window,
            |_, _, _: &FileBrowserEvent, window, cx| {
                let weak = cx.entity().downgrade();
                window.defer(cx, move |window, cx| {
                    let _ = weak.update(cx, |this, cx| this.show_next_challenge(window, cx));
                });
            },
        ));
        files
    }

    fn reset_session(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let session_id = uuid::Uuid::new_v4().to_string();
        let terminal = cx.new(|cx| {
            TerminalView::new(
                session_id.clone(),
                self.backend_tx.clone(),
                &self.settings,
                window,
                cx,
            )
        });
        let files = self.tabs[index]
            .files_visible
            .then(|| self.file_browser(session_id.clone(), window, cx));
        let tab = &mut self.tabs[index];
        tab.session_id = session_id;
        tab.terminal = terminal;
        tab.files = files;
        tab.shell_error = None;
        tab.user_disconnected = false;
    }

    fn reconnect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active_index() else {
            return;
        };
        if !self.workspace_action_allowed(window, cx)
            || matches!(
                self.tabs[index].status,
                ConnectionState::Connected
                    | ConnectionState::Connecting
                    | ConnectionState::Reconnecting { .. }
            )
        {
            return;
        }
        let id = self.tabs[index].id.clone();
        self.retries.remove(&id);
        self.preparations.remove(&id);
        self.emit(
            AppCommand::CloseSession {
                session_id: self.tabs[index].session_id.clone(),
            },
            window,
            cx,
        );
        self.reset_session(index, window, cx);
        self.tabs[index].retry_attempt = 0;
        self.tabs[index].status = ConnectionState::Connecting;
        if self.tabs[index].profile.is_some() {
            self.prepare_connection(id, window, cx);
        } else {
            self.emit(
                AppCommand::CreateSession {
                    session_id: self.tabs[index].session_id.clone(),
                    config: SessionConfig::default(),
                },
                window,
                cx,
            );
        }
        self.sync_inspector(cx);
        self.focus_active(window, cx);
        cx.notify();
    }

    fn disconnect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active_index() else {
            return;
        };
        if window.has_active_dialog(cx)
            || matches!(
                self.tabs[index].status,
                ConnectionState::Disconnected | ConnectionState::Error(_)
            )
        {
            return;
        }
        let tab = &mut self.tabs[index];
        tab.user_disconnected = true;
        tab.secret = None;
        tab.status = ConnectionState::Disconnected;
        tab.terminal
            .update(cx, |terminal, cx| terminal.set_connected(false, cx));
        if let Some(files) = &tab.files {
            files.update(cx, |files, cx| files.set_connected(false, cx));
        }
        let id = tab.id.clone();
        let session_id = tab.session_id.clone();
        self.retries.remove(&id);
        self.preparations.remove(&id);
        self.cancel_challenge(&session_id, window, cx);
        self.emit(AppCommand::CloseSession { session_id }, window, cx);
        self.sync_inspector(cx);
        cx.notify();
    }

    fn close_tab(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) && self.active_challenge.is_none() {
            return;
        }
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        let tab = self.tabs.remove(index);
        self.preparations.remove(&id);
        self.retries.remove(&id);
        self.cancel_challenge(&tab.session_id, window, cx);
        self.emit(
            AppCommand::CloseSession {
                session_id: tab.session_id,
            },
            window,
            cx,
        );
        if self.active.as_ref() == Some(&id) {
            self.active = self
                .tabs
                .get(index.min(self.tabs.len().saturating_sub(1)))
                .map(|tab| tab.id.clone());
        }
        if let Some(active) = self.active.clone() {
            self.activate(active, window, cx);
        } else {
            self.inspector_visible = false;
            self.sync_inspector(cx);
            self.hosts.update(cx, |hosts, cx| hosts.select(None, cx));
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn toggle_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_action_allowed(window, cx) {
            return;
        }
        let Some(index) = self.active_index() else {
            return;
        };
        if self.tabs[index].profile.is_none() {
            return;
        }
        self.tabs[index].files_visible = !self.tabs[index].files_visible;
        if self.tabs[index].files_visible && self.tabs[index].files.is_none() {
            let session_id = self.tabs[index].session_id.clone();
            self.tabs[index].files = Some(self.file_browser(session_id.clone(), window, cx));
            if self.tabs[index].status.is_connected() {
                self.emit(AppCommand::SftpInit { session_id }, window, cx);
            }
        }
        cx.notify();
    }

    fn handle_event(&mut self, event: BackendEvent, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            &event,
            BackendEvent::InspectionResult { .. }
                | BackendEvent::InspectionReady { .. }
                | BackendEvent::SessionClosed { .. }
                | BackendEvent::SessionFailed { .. }
        ) {
            self.inspector
                .update(cx, |inspector, cx| inspector.handle_event(&event, cx));
        }
        if matches!(
            &event,
            BackendEvent::InspectionResult { .. } | BackendEvent::InspectionReady { .. }
        ) {
            return;
        }
        if let BackendEvent::HostKeyPrompt {
            session_id,
            host,
            port,
            algorithm,
            fingerprint,
            respond_to,
        } = event
        {
            if self
                .tabs
                .iter()
                .any(|tab| tab.session_id == session_id && !tab.user_disconnected)
            {
                self.challenges.push_back(HostChallenge {
                    session_id,
                    host,
                    port,
                    algorithm,
                    fingerprint,
                    response: Some(respond_to),
                });
                self.show_next_challenge(window, cx);
            }
            return;
        }
        let session_id = match &event {
            BackendEvent::SessionCreated { session_id }
            | BackendEvent::SessionOutput { session_id, .. }
            | BackendEvent::SessionClosed { session_id, .. }
            | BackendEvent::SessionFailed { session_id, .. }
            | BackendEvent::ShellUnavailable { session_id, .. }
            | BackendEvent::ShellReady { session_id }
            | BackendEvent::InspectionResult { session_id, .. }
            | BackendEvent::InspectionReady { session_id }
            | BackendEvent::SftpReady { session_id, .. }
            | BackendEvent::SftpError { session_id, .. }
            | BackendEvent::SftpListResult { session_id, .. }
            | BackendEvent::SftpOperationResult { session_id, .. }
            | BackendEvent::SftpTransferProgress { session_id, .. }
            | BackendEvent::SftpTransferComplete { session_id, .. } => session_id,
            BackendEvent::HostKeyPrompt { .. } => unreachable!(),
        };
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| &tab.session_id == session_id)
        else {
            return;
        };
        if self.tabs[index].user_disconnected {
            return;
        }
        if let Some(files) = &self.tabs[index].files {
            files.update(cx, |files, cx| files.handle_event(&event, window, cx));
        }
        match event {
            BackendEvent::SessionCreated { session_id } => {
                if self.tabs[index].user_disconnected {
                    return;
                }
                self.tabs[index].status = ConnectionState::Connected;
                self.tabs[index]
                    .terminal
                    .update(cx, |terminal, cx| terminal.set_connected(true, cx));
                if self.tabs[index].files_visible {
                    self.emit(AppCommand::SftpInit { session_id }, window, cx);
                }
            }
            BackendEvent::ShellReady { .. } => {
                self.tabs[index]
                    .terminal
                    .update(cx, |terminal, cx| terminal.set_shell_ready(true, cx));
            }
            BackendEvent::SessionOutput { data, .. } => {
                self.tabs[index]
                    .terminal
                    .update(cx, |terminal, cx| terminal.process_output(&data, cx));
            }
            BackendEvent::SessionClosed {
                reason, retryable, ..
            } => {
                self.session_failed(
                    index,
                    reason.unwrap_or_else(|| "连接已关闭".into()),
                    retryable,
                    window,
                    cx,
                );
            }
            BackendEvent::SessionFailed {
                error, retryable, ..
            } => {
                self.session_failed(index, error, retryable, window, cx);
            }
            BackendEvent::ShellUnavailable { error, .. } => {
                self.tabs[index].shell_error = Some(error);
                self.tabs[index]
                    .terminal
                    .update(cx, |terminal, cx| terminal.set_connected(false, cx));
                if !self.tabs[index].files_visible {
                    self.toggle_files_for(index, window, cx);
                }
            }
            _ => {}
        }
        self.sync_inspector(cx);
    }

    fn toggle_files_for(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs[index].files_visible = true;
        if self.tabs[index].files.is_none() {
            let id = self.tabs[index].session_id.clone();
            self.tabs[index].files = Some(self.file_browser(id.clone(), window, cx));
            self.emit(AppCommand::SftpInit { session_id: id }, window, cx);
        }
    }

    fn session_failed(
        &mut self,
        index: usize,
        error: String,
        retryable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tabs[index]
            .terminal
            .update(cx, |terminal, cx| terminal.set_connected(false, cx));
        if let Some(files) = &self.tabs[index].files {
            files.update(cx, |files, cx| files.set_connected(false, cx));
        }
        if self.tabs[index].user_disconnected || self.retries.contains_key(&self.tabs[index].id) {
            return;
        }
        let session_id = self.tabs[index].session_id.clone();
        self.cancel_challenge(&session_id, window, cx);
        let max = self.settings.ssh.reconnect_max_attempts;
        if retryable && self.tabs[index].profile.is_some() && self.tabs[index].retry_attempt < max {
            let attempt = self.tabs[index].retry_attempt + 1;
            self.tabs[index].retry_attempt = attempt;
            self.tabs[index].status = ConnectionState::Reconnecting { attempt };
            let id = self.tabs[index].id.clone();
            let old_session = self.tabs[index].session_id.clone();
            self.emit(
                AppCommand::CloseSession {
                    session_id: old_session.clone(),
                },
                window,
                cx,
            );
            let delay = Duration::from_secs((1u64 << attempt.min(4)).min(15));
            let timer = cx.background_executor().timer(delay);
            let key = id.clone();
            let task = cx.spawn_in(window, async move |this, cx| {
                timer.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    let Some(index) = this.tabs.iter().position(|tab| {
                        tab.id == id && tab.session_id == old_session && !tab.user_disconnected
                    }) else {
                        return;
                    };
                    this.retries.remove(&id);
                    this.reset_session(index, window, cx);
                    this.tabs[index].status = ConnectionState::Connecting;
                    this.sync_inspector(cx);
                    this.prepare_connection(id.clone(), window, cx);
                    cx.notify();
                });
            });
            self.retries.insert(key, task);
        } else {
            self.tabs[index].status = ConnectionState::Error(error.clone());
            self.error(error, window, cx);
        }
    }

    fn finish_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog_open = false;
        self.connection_form = None;
        self.settings_form = None;
        self.folder_form = None;
        self.move_form = None;
        self.import_form = None;
        self.sync_hosts(cx);
        self.focus_active(window, cx);
        let weak = cx.entity().downgrade();
        window.defer(cx, move |window, cx| {
            let _ = weak.update(cx, |this, cx| this.show_next_challenge(window, cx));
        });
        cx.notify();
    }

    fn dismiss_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        self.finish_dialog(window, cx);
    }

    fn cancel_challenge(&mut self, session_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.challenges
            .retain(|challenge| challenge.session_id != session_id);
        if self.active_challenge.as_deref() == Some(session_id) {
            self.active_challenge = None;
            self.dismiss_dialog(window, cx);
        }
    }

    fn show_next_challenge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open || window.has_active_dialog(cx) {
            return;
        }
        self.challenges
            .retain(|challenge| !challenge.response.as_ref().is_none_or(|tx| tx.is_closed()));
        let Some(challenge) = self.challenges.front() else {
            return;
        };
        let session_id = challenge.session_id.clone();
        let body = format!("{}:{}\n{}\n{}\n\n请通过可信渠道核对指纹。确认后才会发送认证信息，并将此密钥保存到本机。", challenge.host, challenge.port, challenge.algorithm, challenge.fingerprint);
        self.dialog_open = true;
        self.sync_hosts(cx);
        self.active_challenge = Some(session_id.clone());
        let weak = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let accept = weak.clone();
            let reject = weak.clone();
            let closed = weak.clone();
            let accepted_id = session_id.clone();
            let rejected_id = session_id.clone();
            let closed_id = session_id.clone();
            dialog
                .confirm()
                .title("确认服务器主机密钥")
                .description(body.clone())
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("信任并连接")
                        .cancel_text("拒绝连接"),
                )
                .on_ok(move |_, _, cx| {
                    let _ = accept.update(cx, |this, _| {
                        if let Some(challenge) = this
                            .challenges
                            .iter_mut()
                            .find(|item| item.session_id == accepted_id)
                        {
                            if let Some(tx) = challenge.response.take() {
                                let _ = tx.send(true);
                            }
                        }
                    });
                    true
                })
                .on_cancel(move |_, _, cx| {
                    let _ = reject.update(cx, |this, _| {
                        if let Some(challenge) = this
                            .challenges
                            .iter_mut()
                            .find(|item| item.session_id == rejected_id)
                        {
                            if let Some(tx) = challenge.response.take() {
                                let _ = tx.send(false);
                            }
                        }
                    });
                    true
                })
                .on_close(move |_, window, cx| {
                    let _ = closed.update(cx, |this, cx| {
                        this.challenges.retain(|item| item.session_id != closed_id);
                        this.active_challenge = None;
                        this.finish_dialog(window, cx);
                    });
                })
        });
    }

    fn host_action(&mut self, action: HostAction, window: &mut Window, cx: &mut Context<Self>) {
        if !self.loaded || self.profile_busy || self.dialog_open || window.has_active_dialog(cx) {
            return;
        }
        if self.legacy_pending {
            self.show_legacy_choice(window, cx);
            return;
        }
        let id = match &action {
            HostAction::Connect(HostId::Saved(id))
            | HostAction::Edit(id)
            | HostAction::Duplicate(id)
            | HostAction::Delete(id) => id,
            HostAction::NewHost(folder) => {
                self.open_connection(None, window, cx);
                if let Some(form) = &self.connection_form {
                    form.update(cx, |form, cx| form.set_folder(folder.clone(), window, cx));
                }
                return;
            }
            HostAction::CreateFolder(parent) => {
                self.open_folder(None, parent.clone(), window, cx);
                return;
            }
            HostAction::RenameFolder(id) => {
                self.open_folder(Some(id.clone()), None, window, cx);
                return;
            }
            HostAction::DeleteFolder(id) => {
                self.mutate_library(LibraryMutation::DeleteFolder(id.clone()), window, cx);
                return;
            }
            HostAction::ChooseMove(node) => {
                self.open_move(node.clone(), window, cx);
                return;
            }
            HostAction::Move { node, target } => {
                self.mutate_library(
                    LibraryMutation::Move {
                        node: node.clone(),
                        target: target.clone(),
                    },
                    window,
                    cx,
                );
                return;
            }
            HostAction::ImportSshConfig => {
                self.open_import(window, cx);
                return;
            }
        };
        let Some(profile) = self
            .library
            .sessions
            .iter()
            .find(|profile| &profile.id == id)
            .cloned()
        else {
            return;
        };
        match action {
            HostAction::Connect(_) => {
                if matches!(profile.auth,AuthConfig::Password{ref password} if password.is_none())
                    && !profile.remember_credentials
                {
                    self.open_connection(Some(profile), window, cx);
                } else {
                    let origin = HostId::Saved(profile.id.clone());
                    self.connect_profile(profile, None, origin, window, cx);
                }
            }
            HostAction::Edit(_) => self.open_connection(Some(profile), window, cx),
            HostAction::Duplicate(_) => {
                let mut copy = profile;
                copy.id = uuid::Uuid::new_v4().to_string();
                copy.name = format!("{} 副本", copy.name);
                copy.remember_credentials = false;
                copy.import_origin = None;
                match &mut copy.auth {
                    AuthConfig::Password { password } => *password = None,
                    AuthConfig::Key { passphrase, .. } => *passphrase = None,
                    _ => {}
                }
                self.open_connection(Some(copy), window, cx);
            }
            HostAction::Delete(_) => self.confirm_delete(profile, window, cx),
            _ => unreachable!(),
        }
    }

    fn open_folder(
        &mut self,
        id: Option<String>,
        parent: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = id
            .as_ref()
            .and_then(|id| self.library.folders.iter().find(|folder| &folder.id == id))
            .map(|folder| folder.name.as_str())
            .unwrap_or("");
        let form = cx.new(|cx| FolderForm::new(name, window, cx));
        let rename = id.is_some();
        self._subscriptions.push(cx.subscribe_in(
            &form,
            window,
            move |this, _, event: &FolderFormAction, window, cx| match event {
                FolderFormAction::Cancel => this.dismiss_dialog(window, cx),
                FolderFormAction::Save(name) => {
                    let mutation = match &id {
                        Some(id) => LibraryMutation::RenameFolder {
                            id: id.clone(),
                            name: name.clone(),
                        },
                        None => LibraryMutation::CreateFolder {
                            name: name.clone(),
                            parent: parent.clone(),
                        },
                    };
                    this.mutate_library(mutation, window, cx);
                }
            },
        ));
        self.folder_form = Some(form.clone());
        self.dialog_open = true;
        self.sync_hosts(cx);
        let closed = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let closed = closed.clone();
            dialog
                .title(if rename {
                    "重命名文件夹"
                } else {
                    "新建文件夹"
                })
                .w(px(420.))
                .overlay_closable(false)
                .keyboard(false)
                .close_button(false)
                .child(form.clone())
                .on_close(move |_, window, cx| {
                    let _ = closed.update(cx, |this, cx| this.finish_dialog(window, cx));
                })
        });
    }

    fn open_move(&mut self, node: HostNodeId, window: &mut Window, cx: &mut Context<Self>) {
        let form = cx.new(|cx| MoveForm::new(&self.library, node, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &form,
            window,
            |this, _, event: &MoveFormAction, window, cx| match event {
                MoveFormAction::Cancel => this.dismiss_dialog(window, cx),
                MoveFormAction::Move { node, target } => this.mutate_library(
                    LibraryMutation::Move {
                        node: node.clone(),
                        target: target.clone(),
                    },
                    window,
                    cx,
                ),
            },
        ));
        self.move_form = Some(form.clone());
        self.dialog_open = true;
        self.sync_hosts(cx);
        let closed = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let closed = closed.clone();
            dialog
                .title("移动到文件夹")
                .w(px(460.))
                .overlay_closable(false)
                .keyboard(false)
                .close_button(false)
                .child(form.clone())
                .on_close(move |_, window, cx| {
                    let _ = closed.update(cx, |this, cx| this.finish_dialog(window, cx));
                })
        });
    }

    fn mutate_library(
        &mut self,
        mutation: LibraryMutation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.profile_busy || !self.loaded || self.legacy_pending {
            return;
        }
        self.profile_busy = true;
        self.sync_hosts(cx);
        let config = self.config.clone();
        let work = cx.background_spawn(async move {
            match mutation {
                LibraryMutation::CreateFolder { name, parent } => {
                    config.create_folder(&name, parent.as_deref())
                }
                LibraryMutation::RenameFolder { id, name } => config.rename_folder(&id, &name),
                LibraryMutation::Move { node, target } => {
                    config.move_node(&node, target.as_deref())
                }
                LibraryMutation::DeleteFolder(id) => config.delete_empty_folder(&id),
            }
        });
        self._profile_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.profile_busy = false;
                match result {
                    Ok(library) => {
                        this.library = library;
                        if this.folder_form.is_some() || this.move_form.is_some() {
                            this.dismiss_dialog(window, cx);
                        }
                    }
                    Err(error) => {
                        if let Some(form) = &this.folder_form {
                            form.update(cx, |form, cx| {
                                form.busy = false;
                                form.error = Some(error.to_string());
                                cx.notify();
                            });
                        } else if let Some(form) = &this.move_form {
                            form.update(cx, |form, cx| {
                                form.busy = false;
                                form.error = Some(error.to_string());
                                cx.notify();
                            });
                        } else {
                            this.error(error.to_string(), window, cx);
                        }
                    }
                }
                this.sync_hosts(cx);
                cx.notify();
            });
        }));
    }

    fn open_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = cx.new(|cx| {
            SshImportView::new(self.ssh_source.clone(), self.library.clone(), window, cx)
        });
        self._subscriptions.push(cx.subscribe_in(
            &form,
            window,
            |this, _, event: &SshImportAction, window, cx| match event {
                SshImportAction::Cancel => this.dismiss_dialog(window, cx),
                SshImportAction::Submit {
                    generation,
                    profiles,
                } => this.submit_import(*generation, profiles.clone(), window, cx),
            },
        ));
        self.import_form = Some(form.clone());
        self.dialog_open = true;
        self.sync_hosts(cx);
        let closed = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let closed = closed.clone();
            dialog
                .title("从 SSH Config 导入")
                .w(px(680.))
                .overlay_closable(false)
                .keyboard(false)
                .close_button(false)
                .child(form.clone())
                .on_close(move |_, window, cx| {
                    let _ = closed.update(cx, |this, cx| this.finish_dialog(window, cx));
                })
        });
    }

    fn submit_import(
        &mut self,
        generation: u64,
        profiles: Vec<SessionConfig>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.profile_busy
            || !self.loaded
            || self.legacy_pending
            || !self
                .import_form
                .as_ref()
                .is_some_and(|form| form.read(cx).submission_is_current(generation))
        {
            return;
        }
        self.profile_busy = true;
        self.sync_hosts(cx);
        let config = self.config.clone();
        let work = cx.background_spawn(async move { config.import_sessions(&profiles) });
        self._profile_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.profile_busy = false;
                match result {
                    Ok(library) => {
                        this.library = library;
                        this.dismiss_dialog(window, cx);
                        window.push_notification(Notification::success("SSH 主机已导入"), cx);
                    }
                    Err(error) => {
                        if let Some(form) = &this.import_form {
                            form.update(cx, |form, cx| {
                                form.busy = false;
                                form.error = Some(error.to_string());
                                cx.notify();
                            });
                        }
                    }
                }
                this.sync_hosts(cx);
                cx.notify();
            });
        }));
    }

    fn open_connection(
        &mut self,
        profile: Option<SessionConfig>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open || window.has_active_dialog(cx) || self.profile_busy || !self.loaded {
            return;
        }
        if self.legacy_pending {
            self.show_legacy_choice(window, cx);
            return;
        }
        let form = cx.new(|cx| ConnectionForm::new(profile, window, cx));
        form.update(cx, |form, cx| form.set_folders(&self.library, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &form,
            window,
            |this, _, event: &ConnectionAction, window, cx| match event.clone() {
                ConnectionAction::Cancel => this.dismiss_dialog(window, cx),
                ConnectionAction::ApplyDraft(_) => {}
                ConnectionAction::Submit {
                    profile,
                    secret,
                    save,
                    connect,
                } => this.submit_profile(*profile, secret, save, connect, window, cx),
            },
        ));
        self.connection_form = Some(form.clone());
        self.dialog_open = true;
        self.sync_hosts(cx);
        let closed = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let closed = closed.clone();
            dialog
                .title("SSH 连接")
                .w(px(530.))
                .overlay_closable(false)
                .keyboard(false)
                .close_button(false)
                .child(form.clone())
                .on_close(move |_, window, cx| {
                    let _ = closed.update(cx, |this, cx| {
                        this.finish_dialog(window, cx);
                    });
                })
        });
    }

    fn submit_profile(
        &mut self,
        mut profile: SessionConfig,
        secret: Option<String>,
        save: bool,
        connect: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.profile_busy {
            return;
        }
        let origin = HostId::Saved(profile.id.clone());
        self.profile_busy = true;
        self.sync_hosts(cx);
        if !save
            && !self
                .library
                .sessions
                .iter()
                .any(|saved| saved.id == profile.id && saved.remember_credentials)
        {
            profile.remember_credentials = false;
        }
        let config = self.config.clone();
        let new_profile = profile.clone();
        let new_secret = secret.clone();
        let work = cx.background_spawn(async move {
            if save {
                if config.has_legacy_credentials()? {
                    anyhow::bail!("请先处理旧配置中的明文凭据，再保存主机");
                }
                config.save_session_with_credentials(&new_profile, new_secret.as_deref())?;
                return Ok::<_, anyhow::Error>(Some(config.load_library()?));
            }
            Ok(None)
        });
        self._profile_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.profile_busy = false;
                match result {
                    Ok(library) => {
                        if let Some(library) = library {
                            this.library = library;
                            this.sync_hosts(cx);
                        }
                        this.dismiss_dialog(window, cx);
                        if connect {
                            this.connect_profile(profile, secret, origin, window, cx);
                        }
                    }
                    Err(error) => {
                        if let Some(form) = &this.connection_form {
                            form.update(cx, |form, cx| {
                                form.busy = false;
                                form.error = Some(error.to_string());
                                cx.notify();
                            });
                        }
                    }
                }
            });
        }));
    }

    fn confirm_delete(
        &mut self,
        profile: SessionConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open
            || window.has_active_dialog(cx)
            || self.profile_busy
            || self.legacy_pending
        {
            return;
        }
        self.dialog_open = true;
        self.sync_hosts(cx);
        let weak = cx.entity().downgrade();
        let closed = weak.clone();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            let profile = profile.clone();
            let closed = closed.clone();
            dialog
                .confirm()
                .title("删除保存的主机？")
                .description(format!(
                    "{}\n已打开的连接不会关闭；系统钥匙串中关联的凭据也会删除。",
                    profile.name
                ))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("删除主机")
                        .cancel_text("取消"),
                )
                .on_ok(move |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.delete_profile(profile.clone(), window, cx)
                    });
                    true
                })
                .on_close(move |_, window, cx| {
                    let _ = closed.update(cx, |this, cx| this.finish_dialog(window, cx));
                })
        });
    }

    fn delete_profile(
        &mut self,
        profile: SessionConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.profile_busy {
            return;
        }
        self.profile_busy = true;
        self.sync_hosts(cx);
        let config = self.config.clone();
        let id = profile.id.clone();
        let work = cx.background_spawn(async move {
            config.delete_session_with_credentials(&id)?;
            config.load_library()
        });
        self._profile_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.profile_busy = false;
                match result {
                    Ok(library) => this.library = library,
                    Err(error) => this.error(error.to_string(), window, cx),
                }
                this.sync_hosts(cx);
                cx.notify();
            });
        }));
    }

    fn shortcut_tooltip(label: &str, action: &str, cx: &Context<Self>) -> String {
        let hint = crate::shortcuts::hint(action, cx);
        if hint.is_empty() {
            label.to_owned()
        } else {
            format!("{label} · {hint}")
        }
    }

    fn inspector_rail(&self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let disabled = self.active_index().is_none();
        let selected = self.inspector.read(cx).page();
        let entry = |id: &'static str,
                     icon,
                     label: &'static str,
                     action: &str,
                     page,
                     cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .small()
                .icon(icon)
                .tooltip(Self::shortcut_tooltip(label, action, cx))
                .accessibility_label(label)
                .disabled(disabled)
                .when(self.inspector_visible && selected == page, |button| {
                    button.bg(cx.theme().list_active)
                })
                .on_click(
                    cx.listener(move |this, _, window, cx| this.show_inspector(page, window, cx)),
                )
        };
        div()
            .w(px(40.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .py_2()
            .bg(cx.theme().sidebar)
            .border_l_1()
            .border_color(cx.theme().border)
            .child(entry(
                "inspect-system",
                IconName::Cpu,
                "系统监控",
                "show_system",
                InspectorPage::System,
                cx,
            ))
            .child(entry(
                "inspect-network",
                IconName::Network,
                "网络连接",
                "show_network",
                InspectorPage::Network,
                cx,
            ))
            .child(entry(
                "inspect-history",
                IconName::BookOpen,
                "历史命令",
                "show_history",
                InspectorPage::History,
                cx,
            ))
            .child(div().flex_1())
            .child(
                Button::new("shortcut-help")
                    .ghost()
                    .small()
                    .icon(IconName::Info)
                    .tooltip(Self::shortcut_tooltip("快捷键", "show_shortcuts", cx))
                    .accessibility_label("快捷键帮助")
                    .on_click(cx.listener(|this, _, window, cx| this.open_shortcuts(window, cx))),
            )
    }

    fn open_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_action_allowed(window, cx) {
            return;
        }
        let view = cx.new(ShortcutsView::new);
        self.dialog_open = true;
        self.sync_hosts(cx);
        let closed = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let closed = closed.clone();
            dialog
                .title("快捷键")
                .w(px(620.))
                .child(view.clone())
                .on_close(move |_, window, cx| {
                    let _ = closed.update(cx, |this, cx| this.finish_dialog(window, cx));
                })
        });
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open || window.has_active_dialog(cx) || self.settings_busy || !self.loaded {
            return;
        }
        let form = cx.new(|cx| SettingsView::new(self.settings.clone(), window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &form,
            window,
            |this, _, event: &SettingsAction, window, cx| match event {
                SettingsAction::Cancel => this.dismiss_dialog(window, cx),
                SettingsAction::Save(settings) => this.save_settings(settings.clone(), window, cx),
            },
        ));
        self.settings_form = Some(form.clone());
        self.dialog_open = true;
        self.sync_hosts(cx);
        let closed = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let closed = closed.clone();
            dialog
                .title("设置")
                .w(px(520.))
                .overlay_closable(false)
                .keyboard(false)
                .close_button(false)
                .child(form.clone())
                .on_close(move |_, window, cx| {
                    let _ = closed.update(cx, |this, cx| this.finish_dialog(window, cx));
                })
        });
    }

    fn save_settings(
        &mut self,
        mut settings: AppSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_busy {
            return;
        }
        self.settings_busy = true;
        let config = self.config.clone();
        let saved = settings.clone();
        let work = cx.background_spawn(async move { config.save_settings(&saved) });
        self._settings_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.settings_busy = false;
                match result {
                    Ok(()) => {
                        settings.ui = this.settings.ui.clone();
                        this.settings = settings;
                        this.apply_settings(window, cx);
                        this.dismiss_dialog(window, cx);
                    }
                    Err(error) => {
                        if let Some(form) = &this.settings_form {
                            form.update(cx, |form, cx| {
                                form.error = Some(error.to_string());
                                form.busy = false;
                                cx.notify();
                            });
                        }
                    }
                }
                if this.layout_dirty {
                    this.persist_layout(window, cx);
                }
            });
        }));
    }

    fn persist_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.loaded {
            return;
        }
        if self.settings_busy {
            self.layout_dirty = true;
            return;
        }
        self.settings_busy = true;
        self.layout_dirty = false;
        let config = self.config.clone();
        let settings = self.settings.clone();
        let work = cx.background_spawn(async move { config.save_settings(&settings) });
        self._settings_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.settings_busy = false;
                if let Err(error) = result {
                    this.error(error.to_string(), window, cx);
                }
                if this.layout_dirty {
                    this.persist_layout(window, cx);
                }
            });
        }));
    }

    fn apply_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let palette = crate::config::Theme::for_name(&self.settings.app.theme);
        Theme::change(
            if palette.is_light() {
                ThemeMode::Light
            } else {
                ThemeMode::Dark
            },
            Some(window),
            cx,
        );
        Theme::update(cx, |theme| {
            let to_color =
                |[r, g, b]: [u8; 3]| -> Hsla { rgb(u32::from_be_bytes([0, r, g, b])).into() };
            let background = to_color(palette.ui.background);
            let panel = to_color(palette.ui.panel_bg);
            let border = to_color(palette.ui.border);
            let foreground = to_color(palette.ui.foreground);
            let muted = to_color(palette.ui.muted_foreground);
            let primary = to_color(palette.ui.accent);
            let danger = to_color(palette.ui.error);
            let success = to_color(palette.ui.success);
            let warning = to_color(palette.ui.warning);
            let on_accent = if palette.is_light() {
                rgb(0xffffff).into()
            } else {
                background
            };
            let selected = panel.blend(primary.opacity(0.16));
            let hover = panel.blend(foreground.opacity(0.06));
            let active = panel.blend(foreground.opacity(0.1));
            let primary_hover = primary.blend(on_accent.opacity(0.08));
            let primary_active = primary.blend(on_accent.opacity(0.16));
            let colors = &mut theme.colors;
            colors.background = background;
            colors.foreground = foreground;
            colors.border = border;
            colors.input = border;
            colors.accordion = background;
            colors.group_box = panel;
            colors.group_box_foreground = foreground;
            colors.muted = border;
            colors.muted_foreground = muted;
            colors.accent = hover;
            colors.accent_foreground = foreground;
            colors.secondary = border;
            colors.secondary_foreground = foreground;
            colors.secondary_hover = hover;
            colors.secondary_active = active;
            colors.button = panel;
            colors.button_foreground = foreground;
            colors.button_hover = hover;
            colors.button_active = active;
            colors.button_secondary = border;
            colors.button_secondary_foreground = foreground;
            colors.button_secondary_hover = hover;
            colors.button_secondary_active = active;
            colors.sidebar = panel;
            colors.sidebar_border = border;
            colors.sidebar_foreground = foreground;
            colors.sidebar_accent = selected;
            colors.sidebar_accent_foreground = primary;
            colors.sidebar_primary = primary;
            colors.sidebar_primary_foreground = on_accent;
            colors.tab = to_color(palette.ui.tab_inactive_bg);
            colors.tab_bar = to_color(palette.ui.tab_inactive_bg);
            colors.tab_bar_segmented = panel;
            colors.tab_active = to_color(palette.ui.tab_active_bg);
            colors.tab_active_foreground = foreground;
            colors.tab_foreground = muted;
            colors.popover = panel;
            colors.popover_foreground = foreground;
            colors.primary = primary;
            colors.primary_foreground = on_accent;
            colors.primary_hover = primary_hover;
            colors.primary_active = primary_active;
            colors.button_primary = primary;
            colors.button_primary_foreground = on_accent;
            colors.button_primary_hover = primary_hover;
            colors.button_primary_active = primary_active;
            colors.ring = primary;
            colors.caret = primary;
            colors.link = primary;
            colors.link_hover = primary_hover;
            colors.link_active = primary_active;
            colors.selection = rgba(u32::from_be_bytes(palette.ui.selection_bg)).into();
            colors.danger = danger;
            colors.danger_foreground = on_accent;
            colors.danger_hover = danger.blend(on_accent.opacity(0.08));
            colors.danger_active = danger.blend(on_accent.opacity(0.16));
            colors.button_danger = danger;
            colors.button_danger_foreground = on_accent;
            colors.button_danger_hover = colors.danger_hover;
            colors.button_danger_active = colors.danger_active;
            colors.success = success;
            colors.success_foreground = on_accent;
            colors.success_hover = success.blend(on_accent.opacity(0.08));
            colors.success_active = success.blend(on_accent.opacity(0.16));
            colors.button_success = success;
            colors.button_success_foreground = on_accent;
            colors.button_success_hover = colors.success_hover;
            colors.button_success_active = colors.success_active;
            colors.warning = warning;
            colors.warning_foreground = on_accent;
            colors.warning_hover = warning.blend(on_accent.opacity(0.08));
            colors.warning_active = warning.blend(on_accent.opacity(0.16));
            colors.button_warning = warning;
            colors.button_warning_foreground = on_accent;
            colors.button_warning_hover = colors.warning_hover;
            colors.button_warning_active = colors.warning_active;
            colors.info = primary;
            colors.info_foreground = on_accent;
            colors.info_hover = primary_hover;
            colors.info_active = primary_active;
            colors.button_info = primary;
            colors.button_info_foreground = on_accent;
            colors.button_info_hover = primary_hover;
            colors.button_info_active = primary_active;
            colors.list = background;
            colors.list_head = panel;
            colors.list_even = background;
            colors.list_hover = hover;
            colors.list_active = selected;
            colors.list_active_border = primary;
            colors.table = background;
            colors.table_head = panel;
            colors.table_head_foreground = muted;
            colors.table_foot = panel;
            colors.table_foot_foreground = muted;
            colors.table_even = background;
            colors.table_hover = hover;
            colors.table_active = selected;
            colors.table_active_border = primary;
            colors.table_row_border = border;
            colors.title_bar = panel;
            colors.title_bar_border = border;
            colors.status_bar = to_color(palette.ui.statusbar_bg);
            colors.status_bar_border = border;
            colors.scrollbar = background.opacity(0.);
            colors.scrollbar_thumb = border;
            colors.scrollbar_thumb_hover = muted;
            colors.progress_bar = primary;
            colors.drag_border = primary;
            colors.drop_target = selected;
            colors.description_list_label = panel;
            colors.description_list_label_foreground = muted;
        });
        for tab in &self.tabs {
            tab.terminal.update(cx, |terminal, cx| {
                terminal.apply_settings(&self.settings, cx)
            });
            if let Some(files) = &tab.files {
                files.update(cx, |files, cx| files.apply_settings(&self.settings, cx));
            }
        }
        cx.notify();
    }

    fn show_legacy_choice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open || window.has_active_dialog(cx) || self.profile_busy {
            return;
        }
        self.dialog_open = true;
        self.sync_hosts(cx);
        let weak = cx.entity().downgrade();
        window.open_dialog(cx,move|dialog,_,_|{
            let migrate=weak.clone();let discard=weak.clone();let closed=weak.clone();let later=weak.clone();
            dialog.title("旧配置含有明文凭据").w(px(560.)).overlay_closable(false)
                .child(div().text_sm().child("可将密码和私钥口令迁入系统钥匙串，或明确丢弃这些凭据。处理前不能连接或修改保存的主机；稍后处理会保留原文件。"))
                .footer(div().flex().gap_2().justify_end()
                    .child(Button::new("legacy-later").ghost().label("稍后处理").on_click(move|_,window,cx|{let _=later.update(cx,|this,cx|this.dismiss_dialog(window,cx));}))
                    .child(Button::new("legacy-discard").label("丢弃明文凭据").on_click(move|_,window,cx|{let _=discard.update(cx,|this,cx|{this.dismiss_dialog(window,cx);this.resolve_legacy(false,window,cx);});}))
                    .child(Button::new("legacy-migrate").primary().label("迁入系统钥匙串").on_click(move|_,window,cx|{let _=migrate.update(cx,|this,cx|{this.dismiss_dialog(window,cx);this.resolve_legacy(true,window,cx);});})))
                .on_close(move|_,window,cx|{let _=closed.update(cx,|this,cx|this.finish_dialog(window,cx));})
        });
    }

    fn resolve_legacy(&mut self, import: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.profile_busy {
            return;
        }
        self.profile_busy = true;
        self.sync_hosts(cx);
        let config = self.config.clone();
        let work = cx.background_spawn(async move {
            config.resolve_legacy_credentials(import)?;
            config.load_library()
        });
        self._profile_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.profile_busy = false;
                match result {
                    Ok(library) => {
                        this.legacy_pending = false;
                        this.library = library;
                        this.sync_hosts(cx);
                        window.push_notification(Notification::success("明文凭据已从配置移除"), cx);
                    }
                    Err(error) => this.error(error.to_string(), window, cx),
                }
            });
        }));
    }
}

impl Render for App {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let index = self.active_index();
        let connected = index.is_some_and(|index| self.tabs[index].status.is_connected());
        let pending = index.is_some_and(|index| {
            matches!(
                self.tabs[index].status,
                ConnectionState::Connecting | ConnectionState::Reconnecting { .. }
            )
        });
        let ssh = index.is_some_and(|index| self.tabs[index].profile.is_some());
        let viewport = f32::from(window.viewport_size().width);
        let inspector_width = session_tabs::inspector_width(self.settings.ui.inspector_panel_width);
        let sidebar_width =
            session_tabs::sidebar_width(self.settings.ui.session_panel_width, viewport);
        let inline = session_tabs::inspector_is_inline(viewport, sidebar_width, inspector_width);
        let layout_viewport = (viewport, sidebar_width, inline, self.inspector_visible);
        if self.layout_viewport != Some(layout_viewport) {
            self.layout_viewport = Some(layout_viewport);
            cx.on_next_frame(window, |this, window, cx| {
                let viewport = f32::from(window.viewport_size().width);
                let sidebar =
                    session_tabs::sidebar_width(this.settings.ui.session_panel_width, viewport);
                this.host_panel.update(cx, |state, cx| {
                    state.resize_panel(0, px(sidebar), window, cx)
                });
                // The nested group must measure the restored sidebar first;
                // otherwise its proportional resize undoes the inspector width.
                cx.on_next_frame(window, |this, window, cx| {
                    if this.inspector_visible && this.inspector_inline {
                        let inspector =
                            session_tabs::inspector_width(this.settings.ui.inspector_panel_width);
                        this.inspector_panel.update(cx, |state, cx| {
                            state.resize_panel(1, px(inspector), window, cx)
                        });
                    }
                });
            });
        }
        if self.inspector_inline != inline {
            self.inspector_inline = inline;
            if self.inspector_visible {
                self.inspector.read(cx).focus_handle().focus(window, cx);
            }
        }
        let tab_layout = (viewport - sidebar_width, self.tabs.len());
        if self.tab_layout != Some(tab_layout) {
            self.tab_layout = Some(tab_layout);
            if let Some(index) = index {
                self.tab_scroll.scroll_to_item(index);
            }
        }
        let targets = self.tabs.iter().map(SessionTab::target).collect::<Vec<_>>();
        let identities = self
            .tabs
            .iter()
            .zip(&targets)
            .map(|(tab, target)| TabIdentity {
                title: &tab.title,
                target,
                ordinal: tab.ordinal,
            })
            .collect::<Vec<_>>();
        let labels = session_tabs::labels(&identities, &self.opened_targets);
        let tab_ids = self
            .tabs
            .iter()
            .map(|tab| tab.id.clone())
            .collect::<Vec<_>>();
        let tabs = self
            .tabs
            .iter()
            .zip(labels)
            .enumerate()
            .map(|(position, (tab, label))| {
                let id = tab.id.clone();
                let selector = format!("session-tab-{}", tab.id);
                let selected = self.active.as_ref() == Some(&id);
                let state_color = match tab.status {
                    ConnectionState::Connected => cx.theme().success,
                    ConnectionState::Connecting | ConnectionState::Reconnecting { .. } => {
                        cx.theme().warning
                    }
                    ConnectionState::Error(_) => cx.theme().danger,
                    ConnectionState::Disconnected => cx.theme().muted_foreground,
                };
                let visible_label = label
                    .instance
                    .as_ref()
                    .map(|instance| format!("{instance} · {}", label.label))
                    .unwrap_or_else(|| label.label.clone());
                let navigation_action = if position < 8 {
                    Some(format!("select_tab_{}", position + 1))
                } else if position + 1 == self.tabs.len() {
                    Some("select_last_tab".into())
                } else {
                    None
                };
                let navigation_hint = navigation_action
                    .as_deref()
                    .map(|action| crate::shortcuts::hint(action, cx))
                    .filter(|hint| !hint.is_empty());
                let tooltip = format!(
                    "{}\n{} · {}{}",
                    visible_label,
                    targets[position],
                    tab.status.label(),
                    navigation_hint
                        .map(|hint| format!("\n切换到此标签 · {hint}"))
                        .unwrap_or_default()
                );
                Tab::new()
                    .label(visible_label.clone())
                    .aria_label(format!(
                        "标签 {}，{}，{}，{}",
                        position + 1,
                        visible_label,
                        targets[position],
                        tab.status.label()
                    ))
                    .accessibility_id(SharedString::from(format!("session-tab-{}", tab.id)))
                    .debug_selector(move || selector.clone())
                    .font_weight(if selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .prefix(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .pl_2()
                            .child(div().w(px(3.)).h(px(18.)).rounded_sm().bg(if selected {
                                cx.theme().primary
                            } else {
                                cx.theme().transparent
                            }))
                            .child(
                                Icon::new(if tab.profile.is_some() {
                                    IconName::Network
                                } else {
                                    IconName::SquareTerminal
                                })
                                .small()
                                .text_color(state_color),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child((position + 1).to_string()),
                            ),
                    )
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tooltip.clone())
                            .build(window, cx)
                    })
                    .suffix(
                        Button::new(SharedString::from(format!("close-{}", tab.id)))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip(Self::shortcut_tooltip("关闭标签", "close_tab", cx))
                            .accessibility_label("关闭标签")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(id.clone(), window, cx);
                            })),
                    )
            })
            .collect::<Vec<_>>();
        let toolbar = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .h(px(48.))
            .bg(cx.theme().title_bar)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .mr_3()
                    .child("RSTerm"),
            )
            .child(
                Button::new("new-connection")
                    .small()
                    .primary()
                    .icon(IconName::Plus)
                    .label("新建连接")
                    .tooltip(Self::shortcut_tooltip(
                        "新建 SSH 连接",
                        "new_connection",
                        cx,
                    ))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_connection(None, window, cx)),
                    ),
            )
            .child(
                Button::new("new-local")
                    .small()
                    .ghost()
                    .label("本地终端")
                    .tooltip(Self::shortcut_tooltip(
                        "新建本地终端",
                        "new_local_terminal",
                        cx,
                    ))
                    .on_click(cx.listener(|this, _, window, cx| this.local_terminal(window, cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("toggle-files")
                    .small()
                    .ghost()
                    .label("SFTP 文件")
                    .tooltip(Self::shortcut_tooltip(
                        "SFTP 文件（输入框除外）",
                        "toggle_files",
                        cx,
                    ))
                    .disabled(!ssh)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_files(window, cx))),
            )
            .child(
                Button::new("disconnect")
                    .small()
                    .ghost()
                    .label(if pending { "取消连接" } else { "断开" })
                    .disabled(!connected && !pending)
                    .on_click(cx.listener(|this, _, window, cx| this.disconnect(window, cx))),
            )
            .child(
                Button::new("reconnect")
                    .small()
                    .ghost()
                    .label(if ssh { "重连" } else { "重启终端" })
                    .tooltip(Self::shortcut_tooltip(
                        "重连已断开的会话",
                        "reconnect_session",
                        cx,
                    ))
                    .disabled(index.is_none() || connected || pending)
                    .on_click(cx.listener(|this, _, window, cx| this.reconnect(window, cx))),
            )
            .child(
                Button::new("settings")
                    .small()
                    .ghost()
                    .icon(IconName::Settings)
                    .tooltip(Self::shortcut_tooltip("设置", "open_settings", cx))
                    .accessibility_label("设置")
                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
            );
        let content = if let Some(index) = index {
            let tab = &self.tabs[index];
            let terminal = div()
                .debug_selector(|| "app-terminal-body".into())
                .size_full()
                .overflow_hidden()
                .child(tab.terminal.clone());
            let work = if tab.files_visible {
                if let Some(files) = &tab.files {
                    v_resizable(SharedString::from(format!("work-{}", tab.id)))
                        .with_state(&self.work_panel)
                        .child(
                            resizable_panel()
                                .size(px(360.))
                                .size_range(px(120.)..px(2000.))
                                .child(terminal),
                        )
                        .child(
                            resizable_panel()
                                .size(px(320.))
                                .size_range(px(180.)..px(1600.))
                                .child(files.clone()),
                        )
                        .into_any_element()
                } else {
                    terminal.into_any_element()
                }
            } else {
                terminal.into_any_element()
            };
            let target = tab.target();
            let status_color = match tab.status {
                ConnectionState::Connected => cx.theme().success,
                ConnectionState::Connecting | ConnectionState::Reconnecting { .. } => {
                    cx.theme().warning
                }
                ConnectionState::Error(_) => cx.theme().danger,
                ConnectionState::Disconnected => cx.theme().muted_foreground,
            };
            div()
                .flex()
                .flex_col()
                .size_full()
                .min_w_0()
                .child(
                    TabBar::new("session-tabs")
                        .max_width(px(280.))
                        .menu(true)
                        .track_scroll(&self.tab_scroll)
                        .selected_index(index)
                        .children(tabs)
                        .on_click(cx.listener(move |this, index: &usize, window, cx| {
                            if this.workspace_action_allowed(window, cx) {
                                if let Some(id) = tab_ids.get(*index) {
                                    this.activate(id.clone(), window, cx);
                                }
                            }
                        })),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_3()
                        .h(px(32.))
                        .bg(cx.theme().status_bar)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .text_ellipsis()
                                .child(target),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .max_w(px(360.))
                                .text_xs()
                                .text_color(status_color)
                                .child(
                                    div()
                                        .size(px(5.))
                                        .flex_shrink_0()
                                        .rounded_full()
                                        .bg(status_color),
                                )
                                .child(div().min_w_0().text_ellipsis().child(tab.status.label())),
                        ),
                )
                .when(
                    tab.profile.is_none()
                        && matches!(
                            tab.status,
                            ConnectionState::Disconnected | ConnectionState::Error(_)
                        ),
                    |view| {
                        view.child(
                            div()
                                .px_3()
                                .py_2()
                                .text_sm()
                                .text_color(cx.theme().warning)
                                .child("本地终端已停止，点击工具栏“重启终端”重新打开。"),
                        )
                    },
                )
                .when_some(tab.shell_error.clone(), |view, error| {
                    view.child(
                        div()
                            .px_3()
                            .py_1()
                            .text_sm()
                            .text_color(cx.theme().warning)
                            .child(format!("终端不可用，仍可使用 SFTP：{error}")),
                    )
                })
                .child(div().flex_1().min_h_0().min_w_0().child(
                    if self.inspector_visible && !inline {
                        self.inspector.clone().into_any_element()
                    } else if self.inspector_visible {
                        h_resizable("terminal-inspector")
                            .with_state(&self.inspector_panel)
                            .child(
                                resizable_panel()
                                    .size_range(px(440.)..px(4000.))
                                    .child(work),
                            )
                            .child(
                                resizable_panel()
                                    .size(px(inspector_width))
                                    .size_range(px(280.)..px(480.))
                                    .flex_none()
                                    .child(
                                        div()
                                            .size_full()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .border_l_1()
                                            .border_color(cx.theme().border)
                                            .child(self.inspector.clone()),
                                    ),
                            )
                            .on_resize(cx.listener(
                                |this, state: &Entity<ResizableState>, window, cx| {
                                    if let Some(width) = state.read(cx).sizes().get(1) {
                                        this.settings.ui.inspector_panel_width =
                                            session_tabs::inspector_width(f32::from(*width));
                                        this.persist_layout(window, cx);
                                    }
                                },
                            ))
                            .into_any_element()
                    } else {
                        work
                    },
                ))
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    div()
                        .text_2xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("连接主机，开始工作"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("保存常用主机，在多标签终端和双栏 SFTP 之间切换。"),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("welcome-connect")
                                .primary()
                                .label("新建 SSH 连接")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_connection(None, window, cx)
                                })),
                        )
                        .child(Button::new("welcome-local").label("打开本地终端").on_click(
                            cx.listener(|this, _, window, cx| this.local_terminal(window, cx)),
                        )),
                )
                .when_some(self.load_error.clone(), |view, error| {
                    view.child(
                        div()
                            .max_w(px(600.))
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(error),
                    )
                })
                .into_any_element()
        };
        let modal = self.dialog_open || window.has_active_dialog(cx);
        div()
            .id("rsterm-workspace")
            .track_focus(&self.focus)
            .key_context(if modal {
                "RSTerm RstermModal"
            } else {
                "RSTerm"
            })
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(|this, _: &NewConnection, window, cx| {
                if this.workspace_action_allowed(window, cx) {
                    this.open_connection(None, window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &NewLocalTerminal, window, cx| {
                    this.local_terminal(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if this.workspace_action_allowed(window, cx) {
                    if let Some(id) = this.active.clone() {
                        this.close_tab(id, window, cx);
                    }
                }
            }))
            .on_action(
                cx.listener(|this, _: &NextTab, window, cx| this.cycle_tab(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PreviousTab, window, cx| this.cycle_tab(true, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectTab1, window, cx| this.select_tab(1, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectTab2, window, cx| this.select_tab(2, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectTab3, window, cx| this.select_tab(3, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectTab4, window, cx| this.select_tab(4, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectTab5, window, cx| this.select_tab(5, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectTab6, window, cx| this.select_tab(6, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectTab7, window, cx| this.select_tab(7, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectTab8, window, cx| this.select_tab(8, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectLastTab, window, cx| this.select_tab(9, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowSystem, window, cx| {
                this.show_inspector(InspectorPage::System, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowNetwork, window, cx| {
                this.show_inspector(InspectorPage::Network, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowHistory, window, cx| {
                this.show_inspector(InspectorPage::History, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &FocusTerminal, window, cx| this.focus_terminal(window, cx)),
            )
            .on_action(cx.listener(|this, _: &FocusHostSearch, window, cx| {
                if this.workspace_action_allowed(window, cx) {
                    this.hosts
                        .update(cx, |hosts, cx| hosts.focus_search(window, cx));
                }
            }))
            .on_action(
                cx.listener(|this, _: &ReconnectSession, window, cx| this.reconnect(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowShortcuts, window, cx| this.open_shortcuts(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                if this.workspace_action_allowed(window, cx) {
                    this.open_settings(window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &ToggleFiles, window, cx| this.toggle_files(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Quit, window, cx| {
                if this.workspace_action_allowed(window, cx) {
                    cx.quit();
                }
            }))
            .child(toolbar)
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("workspace-columns")
                        .with_state(&self.host_panel)
                        .child(
                            resizable_panel()
                                .size(px(sidebar_width))
                                .flex_none()
                                .size_range(px(180.)..px((viewport - 482.).clamp(180., 420.)))
                                .child(
                                    div()
                                        .size_full()
                                        .min_w_0()
                                        .debug_selector(|| "app-hosts-panel".into())
                                        .child(self.hosts.clone()),
                                ),
                        )
                        .child(
                            resizable_panel().size_range(px(440.)..px(4000.)).child(
                                div()
                                    .flex()
                                    .size_full()
                                    .min_w_0()
                                    .min_h_0()
                                    .child(div().flex_1().min_w_0().min_h_0().child(content))
                                    .child(self.inspector_rail(window, cx)),
                            ),
                        )
                        .on_resize(cx.listener(
                            |this, state: &Entity<ResizableState>, window, cx| {
                                if let Some(width) = state.read(cx).sizes().first() {
                                    this.settings.ui.session_panel_width = f32::from(*width);
                                    this.persist_layout(window, cx);
                                }
                            },
                        )),
                ),
            )
    }
}

fn prepare_ssh(
    config: &ConfigManager,
    profile: &SessionConfig,
    secret: Option<String>,
    settings: &crate::config::SshConfig,
) -> anyhow::Result<(SshConnectionConfig, Option<String>)> {
    let secret = if secret.is_none() && profile.remember_credentials {
        config.credential_for_session(profile)?
    } else {
        secret
    };
    let auth = match &profile.auth {
        AuthConfig::Password { .. } => SshAuthMethod::Password(
            secret
                .clone()
                .ok_or_else(|| anyhow::anyhow!("未提供密码，请编辑连接后重试"))?,
        ),
        AuthConfig::Key { key_path, .. } => SshAuthMethod::Key {
            private_key: std::fs::read(platform::expand_path(key_path))?,
            passphrase: secret.clone(),
        },
        AuthConfig::Agent => SshAuthMethod::Agent,
    };
    let path = config.base_dir().join("known_hosts");
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(&path)?;
    let known_hosts = KnownHosts::load_from(path)?;
    Ok((
        SshConnectionConfig {
            host: profile.host.clone(),
            port: profile.port,
            username: profile.username.clone(),
            auth,
            known_hosts,
            timeout: Duration::from_secs(settings.connection_timeout_secs),
            keepalive_interval: Duration::from_secs(settings.keepalive_interval_secs),
            keepalive_max: settings.keepalive_max_count,
        },
        secret,
    ))
}
