use super::{App, LibraryMutation, SessionTab};
use crate::{
    backend::{AppCommand, BackendEvent},
    config::{AppSettings, AuthConfig, ConfigManager, HostNodeId, SessionConfig},
    inspection::{
        history::parse_history, HistorySource, InspectionData, InspectionKind, SystemSnapshot,
    },
    ssh::SshConfigSource,
    state::ConnectionState,
    ui::{
        hosts::{HostAction, HostId},
        inspector::{InspectionTarget, InspectorAction, InspectorPage},
        sftp::FileBrowserEvent,
        terminal::TerminalView,
    },
};
use gpui_kit::component::{Theme, ThemeMode, WindowExt};
use gpui_kit::{
    px, size, AppContext, ClipboardItem, Context, Entity, EntityInputHandler, KeyDownEvent,
    Keystroke, MouseButton, MouseDownEvent, MouseUpEvent, PlatformInput, Task, TestAppContext,
    VisualTestContext, Window, WindowHandle,
};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

struct Fixture {
    window: WindowHandle<gpui_kit::base::Root>,
    app: Entity<App>,
    commands: mpsc::Receiver<AppCommand>,
    _events: mpsc::Sender<BackendEvent>,
    _config_dir: TempDir,
}

impl Fixture {
    fn new(cx: &mut TestAppContext) -> Self {
        Self::with_ssh_config(cx, None)
    }

    fn with_ssh_config(cx: &mut TestAppContext, contents: Option<&str>) -> Self {
        cx.update(gpui_kit::init);
        cx.update(crate::ui::terminal::init);
        let config_dir = tempfile::tempdir().expect("temporary config directory");
        let config = ConfigManager::from_dir(config_dir.path().to_path_buf()).expect("test config");
        let shortcuts = crate::config::ShortcutConfig::load(&config);
        cx.update(|cx| crate::shortcuts::init(shortcuts, cx));
        let ssh_source = contents.map(|contents| {
            let home = config_dir.path().join("home");
            let path = home.join(".ssh/config");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, contents).unwrap();
            SshConfigSource { path, home }
        });
        let runtime = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );
        let (backend_tx, commands) = mpsc::channel(256);
        let (events, event_rx) = mpsc::channel(256);
        let (window, app) = cx.update(|cx| {
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut app = App::new(
                        config, ssh_source, backend_tx, event_rx, runtime, window, cx,
                    );
                    app.loaded = true;
                    app
                })
            })
            .expect("test window")
        });
        Self {
            window: window.downcast().expect("Base Root"),
            app,
            commands,
            _events: events,
            _config_dir: config_dir,
        }
    }

    fn update<R>(
        &self,
        cx: &mut TestAppContext,
        update: impl FnOnce(&mut App, &mut Window, &mut Context<App>) -> R,
    ) -> R {
        cx.update_window(self.window.into(), |_, window, cx| {
            self.app.update(cx, |app, cx| update(app, window, cx))
        })
        .expect("update test app")
    }

    fn press(&self, key: &str, cx: &mut TestAppContext) {
        cx.update_window(self.window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            window.dispatch_event(
                PlatformInput::KeyDown(KeyDownEvent {
                    keystroke: Keystroke::parse(key).unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                }),
                cx,
            );
            window.draw(cx).clear(cx);
        })
        .unwrap();
    }

    fn frame(&self, cx: &mut TestAppContext) {
        cx.update_window(self.window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    fn commands(&mut self, cx: &mut TestAppContext) -> Vec<AppCommand> {
        cx.run_until_parked();
        std::iter::from_fn(|| self.commands.try_recv().ok()).collect()
    }

    fn inspection_commands(&mut self, cx: &mut TestAppContext) -> Vec<AppCommand> {
        self.commands(cx)
            .into_iter()
            .filter(|command| {
                matches!(
                    command,
                    AppCommand::Inspect { .. } | AppCommand::CancelInspection { .. }
                )
            })
            .collect()
    }
}

fn add_ssh_tab(
    app: &mut App,
    id: &str,
    session_id: &str,
    window: &mut Window,
    cx: &mut Context<App>,
) {
    let terminal = cx.new(|cx| {
        TerminalView::new(
            session_id.into(),
            app.backend_tx.clone(),
            &app.settings,
            window,
            cx,
        )
    });
    terminal.update(cx, |terminal, cx| {
        terminal.process_output(b"\x1b]2;original-title\x07", cx);
    });
    let ordinal = app.next_ordinal("test@test.invalid:22".into());
    app.tabs.push(SessionTab {
        id: id.into(),
        session_id: session_id.into(),
        title: "Test host".into(),
        ordinal,
        profile: Some(SessionConfig {
            id: format!("profile-{id}"),
            name: "Test host".into(),
            host: "test.invalid".into(),
            username: "test".into(),
            auth: AuthConfig::Agent,
            ..Default::default()
        }),
        origin: Some(HostId::Saved(format!("profile-{id}"))),
        secret: Some("temporary-secret".into()),
        terminal,
        files: None,
        files_visible: false,
        status: ConnectionState::Connecting,
        retry_attempt: 0,
        user_disconnected: false,
        shell_error: None,
    });
    app.active = Some(id.into());
}

fn host_prompt(session_id: &str) -> (BackendEvent, oneshot::Receiver<bool>) {
    let (respond_to, response) = oneshot::channel();
    (
        BackendEvent::HostKeyPrompt {
            session_id: session_id.into(),
            host: format!("{session_id}.invalid"),
            port: 22,
            algorithm: "ssh-ed25519".into(),
            fingerprint: format!("SHA256:{session_id}"),
            respond_to,
        },
        response,
    )
}

fn connect_tab(app: &mut App, session_id: &str, window: &mut Window, cx: &mut Context<App>) {
    app.handle_event(
        BackendEvent::SessionCreated {
            session_id: session_id.into(),
        },
        window,
        cx,
    );
    app.handle_event(
        BackendEvent::ShellReady {
            session_id: session_id.into(),
        },
        window,
        cx,
    );
}

fn only_inspection(commands: Vec<AppCommand>, expected: InspectionKind) -> (String, u64) {
    assert_eq!(
        commands.len(),
        1,
        "unexpected inspection commands: {commands:?}"
    );
    match commands.into_iter().next().unwrap() {
        AppCommand::Inspect {
            session_id,
            request_id,
            kind,
        } => {
            assert_eq!(kind, expected);
            (session_id, request_id)
        }
        command => panic!("expected Inspect, got {command:?}"),
    }
}

fn assert_cancel(commands: Vec<AppCommand>, session: &str, request: u64) {
    assert_eq!(
        commands.len(),
        1,
        "unexpected cancellation commands: {commands:?}"
    );
    assert!(
        matches!(&commands[0], AppCommand::CancelInspection { session_id, request_id }
        if session_id == session && *request_id == request)
    );
}

fn load_history(fixture: &mut Fixture, cx: &mut TestAppContext) -> (InspectionTarget, u64, u64) {
    fixture.update(cx, |app, window, cx| {
        app.show_inspector(InspectorPage::History, window, cx);
        app.inspector
            .update(cx, |view, cx| view.refresh_for_test(cx));
    });
    let (session, request) = only_inspection(
        fixture.inspection_commands(cx),
        InspectionKind::History(HistorySource::Zsh),
    );
    let history = parse_history(HistorySource::Zsh, b": 100:0;echo fixture\n", false);
    let entry = history.entries[0].id;
    let target = fixture.update(cx, |app, window, cx| {
        app.handle_event(
            BackendEvent::InspectionResult {
                session_id: session.clone(),
                request_id: request,
                result: Ok(InspectionData::History(history)),
            },
            window,
            cx,
        );
        app.handle_event(
            BackendEvent::InspectionReady {
                session_id: session,
            },
            window,
            cx,
        );
        let target = app.inspection_target().unwrap();
        assert_eq!(
            app.inspector
                .read(cx)
                .insertion_text(&target, request, entry)
                .as_deref(),
            Some("echo fixture")
        );
        target
    });
    (target, request, entry)
}

fn pending_task(cx: &mut Context<App>) -> (Task<()>, oneshot::Receiver<()>) {
    let (completed, receiver) = oneshot::channel();
    let task = cx.spawn(async move |_, _| {
        std::future::pending::<()>().await;
        let _ = completed.send(());
    });
    (task, receiver)
}

#[gpui::test]
fn obsolete_session_events_do_not_mutate_live_tab(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    fixture.update(cx, |app, window, cx| {
        add_ssh_tab(app, "tab", "current-session", window, cx);
        app.handle_event(
            BackendEvent::SessionCreated {
                session_id: "obsolete-session".into(),
            },
            window,
            cx,
        );
        app.handle_event(
            BackendEvent::SessionOutput {
                session_id: "obsolete-session".into(),
                data: b"\x1b]2;stale-title\x07".to_vec(),
            },
            window,
            cx,
        );
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active.as_deref(), Some("tab"));
        assert_eq!(app.tabs[0].session_id, "current-session");
        assert_eq!(app.tabs[0].status, ConnectionState::Connecting);
        assert_eq!(app.tabs[0].terminal.read(cx).title(), "original-title");

        app.handle_event(
            BackendEvent::SessionCreated {
                session_id: "current-session".into(),
            },
            window,
            cx,
        );
        app.handle_event(
            BackendEvent::SessionOutput {
                session_id: "current-session".into(),
                data: b"\x1b]2;live-title\x07".to_vec(),
            },
            window,
            cx,
        );
        assert_eq!(app.tabs[0].status, ConnectionState::Connected);
        assert_eq!(app.tabs[0].terminal.read(cx).title(), "live-title");
    });
}

#[gpui::test]
fn closing_ssh_tab_cancels_prompt_preparation_and_retry(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    let (mut preparation, mut retry, mut response) = fixture.update(cx, |app, window, cx| {
        add_ssh_tab(app, "tab", "session", window, cx);
        let (task, preparation) = pending_task(cx);
        app.preparations.insert("tab".into(), task);
        let (task, retry) = pending_task(cx);
        app.retries.insert("tab".into(), task);
        let (prompt, response) = host_prompt("session");
        app.handle_event(prompt, window, cx);
        assert_eq!(app.active_challenge.as_deref(), Some("session"));
        assert!(app.dialog_open);

        app.close_tab("tab".into(), window, cx);
        assert!(app.tabs.is_empty());
        assert!(app.active.is_none());
        assert!(app.preparations.is_empty());
        assert!(app.retries.is_empty());
        assert!(app.challenges.is_empty());
        assert!(app.active_challenge.is_none());
        assert!(!app.dialog_open);
        (preparation, retry, response)
    });
    cx.run_until_parked();
    assert_eq!(
        preparation.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    );
    assert_eq!(retry.try_recv(), Err(oneshot::error::TryRecvError::Closed));
    assert_eq!(
        response.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    );
}

#[gpui::test]
fn cancelling_active_host_challenge_preserves_queued_identity(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    let (mut response_a, mut response_b) = fixture.update(cx, |app, window, cx| {
        add_ssh_tab(app, "tab-a", "session-a", window, cx);
        add_ssh_tab(app, "tab-b", "session-b", window, cx);
        let (prompt_a, response_a) = host_prompt("session-a");
        let (prompt_b, response_b) = host_prompt("session-b");
        app.handle_event(prompt_a, window, cx);
        app.handle_event(prompt_b, window, cx);
        assert_eq!(app.active_challenge.as_deref(), Some("session-a"));
        assert_eq!(app.challenges.len(), 2);
        app.cancel_challenge("session-a", window, cx);
        assert!(app.active_challenge.is_none());
        assert_eq!(app.challenges.len(), 1);
        assert_eq!(app.challenges[0].session_id, "session-b");
        (response_a, response_b)
    });
    assert_eq!(
        response_a.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    );
    assert_eq!(
        response_b.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    );
    cx.run_until_parked();
    fixture.update(cx, |app, window, cx| {
        assert_eq!(app.active_challenge.as_deref(), Some("session-b"));
        assert!(app.dialog_open);
        app.cancel_challenge("session-a", window, cx);
        assert_eq!(app.active_challenge.as_deref(), Some("session-b"));
        assert!(app.dialog_open);
        assert_eq!(app.challenges.len(), 1);
        assert_eq!(app.challenges[0].session_id, "session-b");
        assert!(app.challenges[0].response.is_some());
    });
    assert_eq!(
        response_b.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    );
}

#[gpui::test]
fn disconnect_ignores_late_creation_output_and_host_prompt(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    let mut response = fixture.update(cx, |app, window, cx| {
        add_ssh_tab(app, "tab", "session", window, cx);
        app.handle_event(
            BackendEvent::SessionCreated {
                session_id: "session".into(),
            },
            window,
            cx,
        );
        assert_eq!(app.tabs[0].status, ConnectionState::Connected);
        app.disconnect(window, cx);
        app.handle_event(
            BackendEvent::SessionCreated {
                session_id: "session".into(),
            },
            window,
            cx,
        );
        app.handle_event(
            BackendEvent::SessionOutput {
                session_id: "session".into(),
                data: b"\x1b]2;late-title\x07".to_vec(),
            },
            window,
            cx,
        );
        let (prompt, response) = host_prompt("session");
        app.handle_event(prompt, window, cx);
        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs[0].user_disconnected);
        assert!(app.tabs[0].secret.is_none());
        assert_eq!(app.tabs[0].status, ConnectionState::Disconnected);
        assert_eq!(app.tabs[0].terminal.read(cx).title(), "original-title");
        assert!(app.challenges.is_empty());
        assert!(!app.dialog_open);
        response
    });
    assert_eq!(
        response.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    );
}

#[gpui::test]
fn disconnected_local_terminal_restarts_with_a_fresh_session(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new(cx);
    let (tab_id, old_session, new_session) = fixture.update(cx, |app, window, cx| {
        app.local_terminal(window, cx);
        let tab_id = app.tabs[0].id.clone();
        let old_session = app.tabs[0].session_id.clone();
        app.handle_event(
            BackendEvent::SessionCreated {
                session_id: old_session.clone(),
            },
            window,
            cx,
        );
        app.disconnect(window, cx);
        assert_eq!(app.tabs[0].status, ConnectionState::Disconnected);
        app.disconnect(window, cx);
        app.reconnect(window, cx);
        let new_session = app.tabs[0].session_id.clone();
        assert_ne!(new_session, old_session);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].id, tab_id);
        assert_eq!(app.active.as_deref(), Some(tab_id.as_str()));
        assert!(!app.tabs[0].user_disconnected);
        assert!(app.tabs[0].profile.is_none());
        assert_eq!(app.tabs[0].status, ConnectionState::Connecting);
        app.reconnect(window, cx);
        assert_eq!(app.tabs[0].session_id, new_session);
        app.handle_event(
            BackendEvent::SessionClosed {
                session_id: old_session.clone(),
                reason: None,
                retryable: false,
            },
            window,
            cx,
        );
        assert_eq!(app.tabs[0].status, ConnectionState::Connecting);
        app.handle_event(
            BackendEvent::SessionCreated {
                session_id: new_session.clone(),
            },
            window,
            cx,
        );
        assert_eq!(app.tabs[0].status, ConnectionState::Connected);
        app.reconnect(window, cx);
        assert_eq!(app.tabs[0].session_id, new_session);
        app.tabs[0].terminal.update(cx, |terminal, cx| {
            assert!(gpui_kit::EntityInputHandler::accepts_text_input(
                terminal, window, cx
            ));
        });
        (tab_id, old_session, new_session)
    });
    cx.run_until_parked();
    let mut created = Vec::new();
    let mut closed = Vec::new();
    while let Ok(command) = fixture.commands.try_recv() {
        match command {
            AppCommand::CreateSession { session_id, .. } => created.push(session_id),
            AppCommand::CloseSession { session_id } => closed.push(session_id),
            _ => {}
        }
    }
    assert_eq!(created, vec![old_session.clone(), new_session]);
    assert_eq!(closed, vec![old_session.clone(), old_session]);
    fixture.update(cx, |app, _, _| {
        assert_eq!(app.active.as_deref(), Some(tab_id.as_str()));
    });
}

#[gpui::test]
fn failed_local_terminal_can_restart_without_an_ssh_profile(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    fixture.update(cx, |app, window, cx| {
        app.local_terminal(window, cx);
        let old_session = app.tabs[0].session_id.clone();
        app.handle_event(
            BackendEvent::SessionClosed {
                session_id: old_session.clone(),
                reason: Some("Shell exited".into()),
                retryable: false,
            },
            window,
            cx,
        );
        assert_eq!(
            app.tabs[0].status,
            ConnectionState::Error("Shell exited".into())
        );
        app.reconnect(window, cx);
        assert_ne!(app.tabs[0].session_id, old_session);
        assert_eq!(app.tabs[0].status, ConnectionState::Connecting);
        assert!(app.tabs[0].profile.is_none());
        assert!(app.preparations.is_empty());
    });
}

#[gpui::test]
fn duplicate_session_failure_schedules_only_one_retry(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new(cx);
    fixture.update(cx, |app, window, cx| {
        add_ssh_tab(app, "tab", "session", window, cx);
        app.settings.ssh.reconnect_max_attempts = 3;
        app.session_failed(0, "transport failed".into(), true, window, cx);
        app.session_failed(0, "transport closed".into(), true, window, cx);
        assert_eq!(app.retries.len(), 1);
        assert!(app.retries.contains_key("tab"));
        assert_eq!(app.tabs[0].retry_attempt, 1);
        assert_eq!(
            app.tabs[0].status,
            ConnectionState::Reconnecting { attempt: 1 }
        );
        assert_eq!(app.tabs[0].session_id, "session");
    });
    cx.run_until_parked();
    let mut close_count = 0;
    while let Ok(command) = fixture.commands.try_recv() {
        if let AppCommand::CloseSession { session_id } = command {
            assert_eq!(session_id, "session");
            close_count += 1;
        }
    }
    assert_eq!(close_count, 1);
    fixture.update(cx, |app, _, _| {
        assert_eq!(app.retries.len(), 1);
        assert_eq!(app.tabs[0].retry_attempt, 1);
        assert_eq!(
            app.tabs[0].status,
            ConnectionState::Reconnecting { attempt: 1 }
        );
    });
}

#[gpui::test]
fn dismissing_connection_dialog_clears_form_and_allows_reopen(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    fixture.update(cx, |app, window, cx| {
        app.open_connection(None, window, cx);
        assert!(app.dialog_open);
        let first_form = app
            .connection_form
            .as_ref()
            .expect("connection form")
            .entity_id();
        app.dismiss_dialog(window, cx);
        assert!(!app.dialog_open);
        assert!(app.connection_form.is_none());
        assert!(app.settings_form.is_none());

        app.open_connection(None, window, cx);
        assert!(app.dialog_open);
        let second_form = app
            .connection_form
            .as_ref()
            .expect("reopened form")
            .entity_id();
        assert_ne!(first_form, second_form);
        app.dismiss_dialog(window, cx);
    });
}

#[gpui::test]
fn sftp_global_dialog_defers_app_modals_and_resumes_host_challenge(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    let (files, mut response) = fixture.update(cx, |app, window, cx| {
        add_ssh_tab(app, "tab", "session", window, cx);
        let files = app.file_browser("session".into(), window, cx);
        app.tabs[0].files = Some(files.clone());
        window.open_dialog(cx, |dialog, _, _| dialog.title("SFTP operation"));
        assert!(window.has_active_dialog(cx));
        assert!(!app.dialog_open);

        app.open_connection(None, window, cx);
        assert!(app.connection_form.is_none());
        assert!(!app.dialog_open);
        app.open_settings(window, cx);
        assert!(app.settings_form.is_none());
        assert!(!app.dialog_open);

        let (prompt, response) = host_prompt("session");
        app.handle_event(prompt, window, cx);
        app.show_next_challenge(window, cx);
        assert_eq!(app.challenges.len(), 1);
        assert_eq!(app.challenges[0].session_id, "session");
        assert!(app.active_challenge.is_none());
        assert!(!app.dialog_open);
        (files, response)
    });
    cx.run_until_parked();
    fixture.update(cx, |app, window, cx| {
        assert!(window.has_active_dialog(cx));
        assert!(app.active_challenge.is_none());
        window.close_dialog(cx);
        assert!(!window.has_active_dialog(cx));
    });
    cx.run_until_parked();
    fixture.update(cx, |app, window, cx| {
        assert!(!window.has_active_dialog(cx));
        assert!(app.active_challenge.is_none());
        assert!(!app.dialog_open);
        assert_eq!(app.challenges.len(), 1);
        files.update(cx, |_, cx| cx.emit(FileBrowserEvent::DialogClosed));
    });
    cx.run_until_parked();
    fixture.update(cx, |app, window, cx| {
        assert!(window.has_active_dialog(cx));
        assert!(app.dialog_open);
        assert_eq!(app.active_challenge.as_deref(), Some("session"));
        assert_eq!(app.challenges.len(), 1);
        assert_eq!(app.challenges[0].session_id, "session");
        assert!(app.connection_form.is_none());
        assert!(app.settings_form.is_none());
    });
    assert_eq!(
        response.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    );
}

#[gpui::test]
fn saving_and_deleting_agent_host_persists_across_config_reload(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new(cx);
    let profile = SessionConfig {
        id: "persisted-agent-host".into(),
        name: "Agent host".into(),
        host: "agent.test.invalid".into(),
        port: 2222,
        username: "test-user".into(),
        auth: AuthConfig::Agent,
        startup_command: Some("pwd".into()),
        environment: [("LANG".into(), "en_US.UTF-8".into())].into(),
        ..Default::default()
    };
    let expected = toml::to_string(&profile).expect("serialize expected profile");
    fixture.update(cx, |app, window, cx| {
        app.open_connection(Some(profile.clone()), window, cx);
        assert!(app.connection_form.is_some());
        app.submit_profile(profile.clone(), None, true, false, window, cx);
        assert!(app.profile_busy);
        assert!(app.library.sessions.is_empty());
    });
    cx.run_until_parked();
    fixture.update(cx, |app, window, cx| {
        assert!(!app.profile_busy);
        assert_eq!(app.library.sessions.len(), 1);
        assert_eq!(toml::to_string(&app.library.sessions[0]).unwrap(), expected);
        let disk = app.config.load_sessions().expect("saved profiles on disk");
        assert_eq!(disk.len(), 1);
        assert_eq!(toml::to_string(&disk[0]).unwrap(), expected);
        assert!(app.tabs.is_empty());
        assert!(app.connection_form.is_none());
        assert!(!app.dialog_open);
        assert!(!window.has_active_dialog(cx));
    });
    let reopened = ConfigManager::from_dir(fixture._config_dir.path().to_path_buf())
        .expect("reopen config manager");
    let restored = reopened.load_sessions().expect("restore saved host");
    assert_eq!(restored.len(), 1);
    assert_eq!(toml::to_string(&restored[0]).unwrap(), expected);
    assert!(matches!(restored[0].auth, AuthConfig::Agent));
    assert!(!restored[0].remember_credentials);
    assert!(matches!(
        fixture.commands.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    fixture.update(cx, |app, window, cx| {
        app.delete_profile(profile, window, cx);
        assert!(app.profile_busy);
    });
    cx.run_until_parked();
    fixture.update(cx, |app, _, _| {
        assert!(!app.profile_busy);
        assert!(app.library.sessions.is_empty());
        assert!(app.config.load_sessions().unwrap().is_empty());
    });
    assert!(reopened.load_sessions().unwrap().is_empty());
    let reopened_after_delete = ConfigManager::from_dir(fixture._config_dir.path().to_path_buf())
        .expect("reopen config after host deletion");
    assert!(reopened_after_delete.load_sessions().unwrap().is_empty());
}

#[gpui::test]
fn saving_settings_persists_and_applies_all_builtin_themes_without_restarting_tabs(
    cx: &mut TestAppContext,
) {
    let mut fixture = Fixture::new(cx);
    let terminal = fixture.update(cx, |app, window, cx| {
        add_ssh_tab(app, "theme-tab", "theme-session", window, cx);
        let terminal = app.tabs[0].terminal.clone();
        terminal.update(cx, |terminal, cx| {
            terminal.process_output(b"preserved terminal output\r\n\x1b[34mdirectory\x1b[0m", cx);
        });
        terminal
    });
    cx.run_until_parked();
    while fixture.commands.try_recv().is_ok() {}
    let mut settings = AppSettings::default();
    settings.app.font_family = "Monaco".into();
    settings.app.font_size = 18.0;
    settings.terminal.scroll_buffer_lines = 2048;
    settings.terminal.copy_on_select = true;
    settings.terminal.right_click_paste = false;
    settings.terminal.cursor_style = "underline".into();
    settings.terminal.cursor_blink = false;
    settings.ssh.connection_timeout_secs = 47;
    settings.ssh.keepalive_interval_secs = 19;
    settings.ssh.keepalive_max_count = 5;
    settings.ssh.reconnect_max_attempts = 2;
    settings.sftp.show_hidden_files = true;
    settings.sftp.confirm_overwrite = false;
    let mut previous_background = None;

    for (name, mode) in [
        ("light", ThemeMode::Light),
        ("catppuccin-mocha", ThemeMode::Dark),
        ("nord", ThemeMode::Dark),
        ("dark", ThemeMode::Dark),
    ] {
        settings.app.theme = name.into();
        let expected = toml::to_string(&settings).expect("serialize expected settings");
        fixture.update(cx, |app, window, cx| {
            app.open_settings(window, cx);
            assert!(app.settings_form.is_some());
            app.save_settings(settings.clone(), window, cx);
            assert!(app.settings_busy);
        });
        cx.run_until_parked();
        let background = fixture.update(cx, |app, window, cx| {
            assert!(!app.settings_busy);
            assert_eq!(toml::to_string(&app.settings).unwrap(), expected);
            assert!(app.settings_form.is_none());
            assert!(!app.dialog_open);
            assert!(!window.has_active_dialog(cx));
            assert_eq!(Theme::global(cx).mode, mode);
            assert_eq!(app.tabs[0].id, "theme-tab");
            assert_eq!(app.tabs[0].session_id, "theme-session");
            assert_eq!(app.tabs[0].terminal, terminal);
            let existing = terminal.read(cx);
            assert_eq!(existing.theme_for_test().name, name);
            assert_eq!(existing.title(), "original-title");
            assert!(existing
                .snapshot_for_test()
                .cells
                .iter()
                .any(|cell| cell.text == "p"));
            let fresh = cx.new(|cx| {
                TerminalView::new(
                    "fresh-theme-session".into(),
                    app.backend_tx.clone(),
                    &app.settings,
                    window,
                    cx,
                )
            });
            assert_eq!(fresh.read(cx).theme_for_test().name, name);
            Theme::global(cx).colors.background
        });
        if let Some(previous) = previous_background {
            assert_ne!(background, previous);
        }
        previous_background = Some(background);
        let reopened = ConfigManager::from_dir(fixture._config_dir.path().to_path_buf())
            .expect("reopen settings config");
        let restored = reopened
            .load_settings()
            .expect("restore settings from disk");
        assert_eq!(toml::to_string(&restored).unwrap(), expected);
        assert!(matches!(
            fixture.commands.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    fixture.update(cx, |app, window, cx| {
        app.settings = AppSettings::default();
        app.loaded = false;
        Theme::change(ThemeMode::Light, Some(window), cx);
        app.load(window, cx);
    });
    cx.run_until_parked();
    fixture.update(cx, |app, _, cx| {
        assert!(app.loaded);
        assert!(app.load_error.is_none());
        assert_eq!(
            toml::to_string(&app.settings).unwrap(),
            toml::to_string(&settings).unwrap()
        );
        assert_eq!(Theme::global(cx).mode, ThemeMode::Dark);
        assert_eq!(
            Some(Theme::global(cx).colors.background),
            previous_background
        );
    });
}

#[gpui::test]
fn failed_theme_save_and_cancel_preserve_the_applied_theme(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    let before = fixture.update(cx, |app, window, cx| {
        app.apply_settings(window, cx);
        app.open_settings(window, cx);
        app.dismiss_dialog(window, cx);
        assert_eq!(app.settings.app.theme, "dark");
        let path = app.config.base_dir().join("config.toml");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let mut draft = app.settings.clone();
        draft.app.theme = "light".into();
        app.open_settings(window, cx);
        app.save_settings(draft, window, cx);
        Theme::global(cx).colors.background
    });
    cx.run_until_parked();
    fixture.update(cx, |app, window, cx| {
        assert!(!app.settings_busy);
        assert_eq!(app.settings.app.theme, "dark");
        assert_eq!(Theme::global(cx).colors.background, before);
        assert_eq!(Theme::global(cx).mode, ThemeMode::Dark);
        let form = app.settings_form.as_ref().unwrap().read(cx);
        assert!(form.error.is_some());
        assert!(!form.busy);
        app.dismiss_dialog(window, cx);
    });
}

#[gpui::test]
fn settings_dialog_keeps_previews_and_footer_visible_when_resized(cx: &mut TestAppContext) {
    use gpui_kit::{point, px, size, ScrollDelta, ScrollWheelEvent, VisualTestContext};

    let fixture = Fixture::new(cx);
    fixture.update(cx, |app, window, cx| app.open_settings(window, cx));
    let mut visual = VisualTestContext::from_window(fixture.window.into(), cx);
    for viewport in [size(px(1280.), px(820.)), size(px(860.), px(600.))] {
        visual.simulate_resize(viewport);
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let content = visual.debug_bounds("settings-scroll-content").unwrap();
        let previews = visual.debug_bounds("settings-theme-selector").unwrap();
        let footer = visual.debug_bounds("settings-footer").unwrap();
        assert!(content.size.height >= px(120.));
        assert!(previews.size.height > px(100.));
        assert!(previews.origin.y >= px(0.));
        assert!(previews.origin.y + previews.size.height <= footer.origin.y);
        assert!(footer.origin.y + footer.size.height <= viewport.height);
        assert!(footer.origin.x >= px(0.));
        assert!(footer.origin.x + footer.size.width <= viewport.width);
    }
    let before = visual.debug_bounds("settings-scroll-content").unwrap();
    let footer = visual.debug_bounds("settings-footer").unwrap();
    visual.simulate_event(ScrollWheelEvent {
        position: point(before.center().x, footer.origin.y - px(30.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-600.))),
        ..Default::default()
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let scrolled = visual.debug_bounds("settings-scroll-content").unwrap();
    let footer = visual.debug_bounds("settings-footer").unwrap();
    assert!(scrolled.origin.y < before.origin.y);
    assert!(scrolled.origin.y + scrolled.size.height <= footer.origin.y);
    assert!(footer.origin.y + footer.size.height <= px(600.));
}

#[gpui::test]
fn failed_host_delete_unlocks_sidebar_and_preserves_library(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new(cx);
    fixture.update(cx, |app, window, cx| {
        let profile = SessionConfig {
            id: "keep-host".into(),
            name: "Keep".into(),
            auth: AuthConfig::Agent,
            ..Default::default()
        };
        add_ssh_tab(app, "live-tab", "live-session", window, cx);
        app.config.save_session(&profile).unwrap();
        app.library = app.config.load_library().unwrap();
        app.sync_hosts(cx);
        std::fs::write(app.config.base_dir().join("sessions.toml"), "broken = [").unwrap();
        app.delete_profile(profile, window, cx);
        assert!(app.profile_busy);
        assert!(app.hosts.read(cx).is_busy_for_test());
    });
    cx.run_until_parked();
    fixture.update(cx, |app, _, cx| {
        assert!(!app.profile_busy);
        assert!(!app.hosts.read(cx).is_busy_for_test());
        assert_eq!(app.library.sessions.len(), 1);
        assert_eq!(app.library.sessions[0].id, "keep-host");
        assert_eq!(app.tabs[0].session_id, "live-session");
        assert_eq!(app.tabs[0].terminal.read(cx).title(), "original-title");
    });
    assert!(fixture.commands.try_recv().is_err());
}

const SSH_CONFIG_FIXTURE: &str = "Host db\n HostName database.test.invalid\n User fixture\n IdentityFile ~/.ssh/keys/nonexistent\n IdentitiesOnly yes\n";

#[gpui::test]
fn startup_loads_saved_library_without_reading_external_source(cx: &mut TestAppContext) {
    let mut fixture = Fixture::with_ssh_config(cx, Some("Host broken\n User \"unclosed\n"));
    let source_path = fixture.update(cx, |app, window, cx| {
        app.library = app.config.create_folder("Saved folder", None).unwrap();
        app.config
            .save_session_with_credentials(
                &SessionConfig {
                    id: "saved-db".into(),
                    name: "db".into(),
                    host: "saved.test.invalid".into(),
                    username: "saved-user".into(),
                    folder_id: Some(app.library.folders[0].id.clone()),
                    auth: AuthConfig::Agent,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        app.loaded = false;
        app.load(window, cx);
        assert!(app.import_form.is_none());
        app.ssh_source.as_ref().unwrap().path.clone()
    });
    let permissions = std::fs::metadata(&source_path).unwrap().permissions();
    cx.run_until_parked();
    fixture.update(cx, |app, _, _| {
        assert!(app.loaded);
        assert!(app.load_error.is_none());
        assert_eq!(app.library.sessions.len(), 1);
        assert_eq!(app.library.sessions[0].id, "saved-db");
        assert_eq!(app.library.folders.len(), 1);
        assert!(app.import_form.is_none());
        assert!(app.tabs.is_empty());
        assert!(app.preparations.is_empty());
        assert!(!app.config.base_dir().join("known_hosts").exists());
    });
    assert_eq!(
        std::fs::read_to_string(&source_path).unwrap(),
        "Host broken\n User \"unclosed\n"
    );
    assert_eq!(
        std::fs::metadata(source_path).unwrap().permissions(),
        permissions
    );
    assert!(matches!(
        fixture.commands.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[gpui::test]
fn manual_import_reads_a_fixed_snapshot_and_persists_independent_hosts(cx: &mut TestAppContext) {
    let mut fixture = Fixture::with_ssh_config(cx, Some(SSH_CONFIG_FIXTURE));
    let source = fixture.update(cx, |app, window, cx| {
        app.host_action(HostAction::ImportSshConfig, window, cx);
        let form = app.import_form.as_ref().unwrap().clone();
        assert_eq!(form.read(cx).preview_count_for_test(), 0);
        form.update(cx, |form, cx| form.load_for_test(window, cx));
        app.ssh_source.as_ref().unwrap().path.clone()
    });
    let permissions = std::fs::metadata(&source).unwrap().permissions();
    cx.run_until_parked();
    fixture.update(cx, |app, _, cx| {
        let form = app.import_form.as_ref().unwrap().clone();
        assert_eq!(form.read(cx).preview_count_for_test(), 1);
        assert!(app.library.sessions.is_empty());
        assert!(app.config.load_sessions().unwrap().is_empty());
        std::fs::write(
            &source,
            SSH_CONFIG_FIXTURE.replace("database.test.invalid", "changed.test.invalid"),
        )
        .unwrap();
        form.update(cx, |form, cx| {
            form.select_for_test(0, true, cx);
            form.submit_for_test(cx);
        });
    });
    cx.run_until_parked();
    fixture.update(cx, |app, _, _| {
        assert!(!app.profile_busy);
        assert!(app.import_form.is_none());
        assert_eq!(app.library.sessions.len(), 1);
        let saved = &app.library.sessions[0];
        assert!(uuid::Uuid::parse_str(&saved.id).is_ok());
        assert_eq!(saved.host, "database.test.invalid");
        assert!(!saved.remember_credentials);
        assert_eq!(saved.import_origin.as_ref().unwrap().alias, "db");
        assert_eq!(
            saved.import_origin.as_ref().unwrap().source_path,
            source.canonicalize().unwrap()
        );
        let restored = app.config.load_library().unwrap();
        assert_eq!(restored.sessions[0].id, saved.id);
        assert!(app.tabs.is_empty());
        assert!(app.preparations.is_empty());
        assert!(!app.config.base_dir().join("known_hosts").exists());
    });
    assert_eq!(
        std::fs::read_to_string(&source).unwrap(),
        SSH_CONFIG_FIXTURE.replace("database.test.invalid", "changed.test.invalid")
    );
    assert_eq!(
        std::fs::metadata(source).unwrap().permissions(),
        permissions
    );
    assert!(matches!(
        fixture.commands.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[gpui::test]
fn manual_read_failure_and_cancel_do_not_modify_saved_hosts(cx: &mut TestAppContext) {
    let mut fixture = Fixture::with_ssh_config(cx, Some("Host broken\n User \"unclosed\n"));
    fixture.update(cx, |app, window, cx| {
        app.library = app.config.create_folder("Keep", None).unwrap();
        app.host_action(HostAction::ImportSshConfig, window, cx);
        let form = app.import_form.as_ref().unwrap().clone();
        form.update(cx, |form, cx| form.load_for_test(window, cx));
    });
    cx.run_until_parked();
    fixture.update(cx, |app, window, cx| {
        let form = app.import_form.as_ref().unwrap().read(cx);
        assert!(form.error.is_some());
        assert_eq!(form.preview_count_for_test(), 0);
        app.dismiss_dialog(window, cx);
        assert_eq!(app.library.folders.len(), 1);
        assert!(app.library.sessions.is_empty());
        assert!(app.config.load_library().unwrap().sessions.is_empty());
    });
    assert!(matches!(
        fixture.commands.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[gpui::test]
fn changing_import_path_invalidates_preview_and_prevents_stale_submit(cx: &mut TestAppContext) {
    let fixture = Fixture::with_ssh_config(cx, Some(SSH_CONFIG_FIXTURE));
    fixture.update(cx, |app, window, cx| {
        app.host_action(HostAction::ImportSshConfig, window, cx);
        app.import_form
            .as_ref()
            .unwrap()
            .update(cx, |form, cx| form.load_for_test(window, cx));
    });
    cx.run_until_parked();
    fixture.update(cx, |app, window, cx| {
        let form = app.import_form.as_ref().unwrap().clone();
        form.update(cx, |form, cx| {
            form.select_for_test(0, true, cx);
            form.set_path_for_test("/nonexistent/other-config", window, cx);
            form.submit_for_test(cx);
        });
    });
    cx.run_until_parked();
    fixture.update(cx, |app, _, cx| {
        assert!(!app.profile_busy);
        assert!(app.library.sessions.is_empty());
        assert_eq!(
            app.import_form
                .as_ref()
                .unwrap()
                .read(cx)
                .preview_count_for_test(),
            0
        );
        assert!(app.config.load_sessions().unwrap().is_empty());
    });
}

#[gpui::test]
fn unsupported_routes_cannot_be_imported_or_connected(cx: &mut TestAppContext) {
    for directive in [
        "ProxyJump gateway",
        "Include other.conf",
        "Match exec \"never-execute\"",
    ] {
        let mut fixture =
            Fixture::with_ssh_config(cx, Some(&format!("{SSH_CONFIG_FIXTURE}{directive}\n")));
        fixture.update(cx, |app, window, cx| {
            app.host_action(HostAction::ImportSshConfig, window, cx);
            app.import_form
                .as_ref()
                .unwrap()
                .update(cx, |form, cx| form.load_for_test(window, cx));
        });
        cx.run_until_parked();
        fixture.update(cx, |app, _, cx| {
            let form = app.import_form.as_ref().unwrap().clone();
            assert_eq!(form.read(cx).preview_count_for_test(), 1);
            form.update(cx, |form, cx| {
                form.select_for_test(0, true, cx);
                assert_eq!(form.selected_count_for_test(), 0);
                form.submit_for_test(cx);
            });
            assert!(app.connection_form.is_none());
            assert!(app.tabs.is_empty());
            assert!(app.library.sessions.is_empty());
            assert!(app.config.load_sessions().unwrap().is_empty());
        });
        assert!(matches!(
            fixture.commands.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }
}

#[gpui::test]
fn folder_moves_preserve_live_session_and_publish_only_after_save(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new(cx);
    let target = fixture.update(cx, |app, window, cx| {
        app.library = app.config.create_folder("Production", None).unwrap();
        let target = app.library.folders[0].id.clone();
        let profile = SessionConfig {
            id: "stable-host".into(),
            name: "Live host".into(),
            host: "fixture.invalid".into(),
            username: "fixture".into(),
            auth: AuthConfig::Agent,
            ..Default::default()
        };
        app.config.save_session(&profile).unwrap();
        app.library = app.config.load_library().unwrap();
        add_ssh_tab(app, "live-tab", "live-session", window, cx);
        app.tabs[0].profile = Some(profile);
        app.tabs[0].origin = Some(HostId::Saved("stable-host".into()));
        target
    });
    cx.run_until_parked();
    while fixture.commands.try_recv().is_ok() {}
    let terminal = fixture.update(cx, |app, window, cx| {
        let terminal = app.tabs[0].terminal.clone();
        app.host_action(
            HostAction::Move {
                node: HostNodeId::Host("stable-host".into()),
                target: Some(target.clone()),
            },
            window,
            cx,
        );
        assert!(app.profile_busy);
        assert!(app.library.sessions[0].folder_id.is_none());
        terminal
    });
    cx.run_until_parked();
    fixture.update(cx, |app, _, _| {
        assert!(!app.profile_busy);
        assert_eq!(
            app.library.sessions[0].folder_id.as_deref(),
            Some(target.as_str())
        );
        assert_eq!(app.library.sessions[0].id, "stable-host");
        assert_eq!(app.tabs[0].session_id, "live-session");
        assert_eq!(app.tabs[0].terminal, terminal);
        assert_eq!(
            app.tabs[0].origin,
            Some(HostId::Saved("stable-host".into()))
        );
        assert_eq!(
            app.tabs[0].profile.as_ref().unwrap().host,
            "fixture.invalid"
        );
        assert_eq!(
            app.config.load_library().unwrap().sessions[0]
                .folder_id
                .as_deref(),
            Some(target.as_str())
        );
    });
    assert!(matches!(
        fixture.commands.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    fixture.update(cx, |app, window, cx| {
        std::fs::write(app.config.base_dir().join("sessions.toml"), "broken = [").unwrap();
        app.mutate_library(
            LibraryMutation::Move {
                node: HostNodeId::Host("stable-host".into()),
                target: None,
            },
            window,
            cx,
        );
    });
    cx.run_until_parked();
    fixture.update(cx, |app, _, _| {
        assert_eq!(
            app.library.sessions[0].folder_id.as_deref(),
            Some(target.as_str())
        );
        assert_eq!(app.tabs[0].terminal, terminal);
        assert_eq!(app.tabs[0].session_id, "live-session");
    });
    assert!(matches!(
        fixture.commands.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[gpui::test]
fn root_tab_shortcuts_wrap_select_all_positions_and_ignore_missing_positions(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new(cx);
    fixture.update(cx, |app, window, cx| {
        for index in 1..=10 {
            add_ssh_tab(
                app,
                &format!("tab-{index}"),
                &format!("session-{index}"),
                window,
                cx,
            );
            app.library
                .sessions
                .push(app.tabs.last().unwrap().profile.clone().unwrap());
        }
        app.sync_hosts(cx);
        app.activate("tab-10".into(), window, cx);
    });
    fixture.press("ctrl-tab", cx);
    fixture.update(cx, |app, window, cx| {
        assert_eq!(app.active.as_deref(), Some("tab-1"));
        assert!(app.tabs[0].terminal.read(cx).is_focused(window));
    });
    fixture.press("ctrl-shift-tab", cx);
    fixture.update(cx, |app, _, _| {
        assert_eq!(app.active.as_deref(), Some("tab-10"))
    });
    for position in 1..=9 {
        let modifier = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "alt"
        };
        fixture.press(&format!("{modifier}-{position}"), cx);
        fixture.update(cx, |app, window, cx| {
            let expected = if position == 9 { 10 } else { position };
            assert_eq!(app.active, Some(format!("tab-{expected}")));
            assert!(app.tabs[expected - 1].terminal.read(cx).is_focused(window));
            assert_eq!(
                app.hosts.read(cx).active_for_test(),
                Some(HostId::Saved(format!("profile-tab-{expected}")))
            );
        });
    }
    fixture.update(cx, |app, window, cx| {
        for position in (4..=10).rev() {
            app.close_tab(format!("tab-{position}"), window, cx);
        }
        app.activate("tab-2".into(), window, cx);
    });
    fixture.press(
        if cfg!(target_os = "macos") {
            "cmd-8"
        } else {
            "alt-8"
        },
        cx,
    );
    fixture.update(cx, |app, _, _| {
        assert_eq!(app.active.as_deref(), Some("tab-2"))
    });
    fixture.press(
        if cfg!(target_os = "macos") {
            "cmd-9"
        } else {
            "alt-9"
        },
        cx,
    );
    fixture.update(cx, |app, _, _| {
        assert_eq!(app.active.as_deref(), Some("tab-3"))
    });
}

#[gpui::test]
fn root_workspace_shortcuts_respect_global_modal_and_terminal_composition(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    fixture.update(cx, |app, window, cx| {
        add_ssh_tab(app, "one", "session-one", window, cx);
        add_ssh_tab(app, "two", "session-two", window, cx);
        connect_tab(app, "session-one", window, cx);
        app.activate("one".into(), window, cx);
        app.tabs[0].terminal.update(cx, |terminal, cx| {
            terminal.replace_and_mark_text_in_range(None, "中", Some(0..1), window, cx);
        });
        assert!(app.tabs[0].terminal.read(cx).has_marked_text());
    });
    for key in [
        "ctrl-tab",
        "ctrl-shift-tab",
        if cfg!(target_os = "macos") {
            "cmd-2"
        } else {
            "alt-2"
        },
        if cfg!(target_os = "macos") {
            "cmd-shift-h"
        } else {
            "ctrl-alt-shift-h"
        },
    ] {
        fixture.press(key, cx);
        fixture.update(cx, |app, window, cx| {
            assert_eq!(app.active.as_deref(), Some("one"));
            assert!(app.tabs[0].terminal.read(cx).has_marked_text());
            assert!(app.tabs[0].terminal.read(cx).is_focused(window));
            assert!(!app.inspector_visible);
        });
    }
    fixture.update(cx, |app, window, cx| {
        app.tabs[0]
            .terminal
            .update(cx, |terminal, cx| terminal.unmark_text(window, cx));
        window.open_dialog(cx, |dialog, _, _| dialog.title("Blocking dialog"));
    });
    for key in [
        "ctrl-tab",
        "ctrl-shift-tab",
        if cfg!(target_os = "macos") {
            "cmd-2"
        } else {
            "alt-2"
        },
    ] {
        fixture.press(key, cx);
        fixture.update(cx, |app, _, _| {
            assert_eq!(app.active.as_deref(), Some("one"))
        });
    }
    fixture.update(cx, |app, window, cx| {
        window.close_dialog(cx);
        app.focus_active(window, cx);
    });
    fixture.press("ctrl-tab", cx);
    fixture.update(cx, |app, _, _| {
        assert_eq!(app.active.as_deref(), Some("two"))
    });
}

#[gpui::test]
fn duplicate_ordinals_and_stale_frame_tab_clicks_keep_stable_identity(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    fixture.update(cx, |app, window, cx| {
        for id in ["one", "two", "three"] {
            add_ssh_tab(app, id, &format!("session-{id}"), window, cx);
        }
        assert_eq!(
            app.tabs.iter().map(|tab| tab.ordinal).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        app.activate("two".into(), window, cx);
    });
    let mut visual = VisualTestContext::from_window(fixture.window.into(), cx);
    visual.simulate_resize(size(px(1280.), px(820.)));
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let third = visual
        .debug_bounds("session-tab-three")
        .expect("third tab bounds");
    cx.update_window(fixture.window.into(), |_, window, cx| {
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                position: third.center(),
                button: MouseButton::Left,
                click_count: 1,
                ..Default::default()
            }),
            cx,
        );
        fixture.app.update(cx, |app, cx| {
            app.close_tab("one".into(), window, cx);
            assert_eq!(
                app.tabs.iter().map(|tab| tab.ordinal).collect::<Vec<_>>(),
                [2, 3]
            );
        });
        // Dispatch against the same painted frame after the live vector changed.
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                position: third.center(),
                button: MouseButton::Left,
                click_count: 1,
                ..Default::default()
            }),
            cx,
        );
    })
    .unwrap();
    fixture.update(cx, |app, window, cx| {
        assert_eq!(app.active.as_deref(), Some("three"));
        add_ssh_tab(app, "four", "session-four", window, cx);
        assert_eq!(
            app.tabs.iter().map(|tab| tab.ordinal).collect::<Vec<_>>(),
            [2, 3, 4]
        );
        let targets = app.tabs.iter().map(SessionTab::target).collect::<Vec<_>>();
        let identities = app
            .tabs
            .iter()
            .zip(&targets)
            .map(|(tab, target)| super::TabIdentity {
                title: &tab.title,
                target,
                ordinal: tab.ordinal,
            })
            .collect::<Vec<_>>();
        let labels = super::session_tabs::labels(&identities, &app.opened_targets);
        assert_eq!(
            labels
                .iter()
                .map(|label| label.instance.as_deref())
                .collect::<Vec<_>>(),
            [Some("#2"), Some("#3"), Some("#4")]
        );
    });
}

#[gpui::test]
fn inspector_default_and_history_open_do_not_collect_and_system_show_uses_current_page(
    cx: &mut TestAppContext,
) {
    let mut fixture = Fixture::new(cx);
    fixture.update(cx, |app, window, cx| {
        add_ssh_tab(app, "tab", "session", window, cx);
        connect_tab(app, "session", window, cx);
        assert!(!app.inspector_visible);
    });
    assert!(fixture.inspection_commands(cx).is_empty());
    fixture.update(cx, |app, window, cx| {
        app.show_inspector(InspectorPage::History, window, cx)
    });
    assert!(fixture.inspection_commands(cx).is_empty());
    fixture.update(cx, |app, window, cx| app.hide_inspector(window, cx));
    assert!(fixture.inspection_commands(cx).is_empty());
    fixture.update(cx, |app, window, cx| {
        app.show_inspector(InspectorPage::System, window, cx)
    });
    let (session, request) =
        only_inspection(fixture.inspection_commands(cx), InspectionKind::System);
    assert_eq!(session, "session");
    fixture.update(cx, |app, window, cx| app.hide_inspector(window, cx));
    assert_cancel(fixture.inspection_commands(cx), &session, request);
}

#[gpui::test]
fn inspector_tab_switch_hide_and_late_results_keep_exact_request_identity(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new(cx);
    fixture.update(cx, |app, window, cx| {
        add_ssh_tab(app, "a", "session-a", window, cx);
        add_ssh_tab(app, "b", "session-b", window, cx);
        connect_tab(app, "session-a", window, cx);
        connect_tab(app, "session-b", window, cx);
        app.activate("a".into(), window, cx);
        app.show_inspector(InspectorPage::System, window, cx);
    });
    let (session_a, request_a) =
        only_inspection(fixture.inspection_commands(cx), InspectionKind::System);
    fixture.update(cx, |app, window, cx| app.activate("b".into(), window, cx));
    let commands = fixture.inspection_commands(cx);
    assert_eq!(commands.len(), 2, "{commands:?}");
    assert!(
        matches!(&commands[0], AppCommand::CancelInspection { session_id, request_id } if session_id == &session_a && *request_id == request_a)
    );
    let (session_b, request_b) = only_inspection(
        commands.into_iter().skip(1).collect(),
        InspectionKind::System,
    );
    assert_eq!(session_b, "session-b");
    fixture.update(cx, |app, window, cx| {
        for (session, request) in [
            ("session-a", request_a),
            ("session-b", request_b + 10000),
            ("obsolete", request_b),
        ] {
            app.handle_event(
                BackendEvent::InspectionResult {
                    session_id: session.into(),
                    request_id: request,
                    result: Ok(InspectionData::System(SystemSnapshot {
                        hostname: Some("stale".into()),
                        ..Default::default()
                    })),
                },
                window,
                cx,
            );
        }
        assert!(app.inspector.read(cx).system_for_test().is_none());
        app.handle_event(
            BackendEvent::InspectionResult {
                session_id: session_b.clone(),
                request_id: request_b,
                result: Ok(InspectionData::System(SystemSnapshot {
                    hostname: Some("current".into()),
                    ..Default::default()
                })),
            },
            window,
            cx,
        );
        assert_eq!(
            app.inspector
                .read(cx)
                .system_for_test()
                .unwrap()
                .hostname
                .as_deref(),
            Some("current")
        );
        app.handle_event(
            BackendEvent::InspectionReady {
                session_id: session_b.clone(),
            },
            window,
            cx,
        );
        app.inspector
            .update(cx, |view, cx| view.refresh_for_test(cx));
    });
    let (_, next_request_b) =
        only_inspection(fixture.inspection_commands(cx), InspectionKind::System);
    assert_ne!(next_request_b, request_b);
    fixture.update(cx, |app, window, cx| app.hide_inspector(window, cx));
    assert_cancel(fixture.inspection_commands(cx), &session_b, next_request_b);
    fixture.update(cx, |app, window, cx| {
        app.handle_event(
            BackendEvent::InspectionResult {
                session_id: session_b,
                request_id: next_request_b,
                result: Ok(InspectionData::System(SystemSnapshot::default())),
            },
            window,
            cx,
        );
        assert!(app.inspector.read(cx).system_for_test().is_none());
        assert!(!app.inspector_visible);
    });
}

#[gpui::test]
fn inspector_reconnect_clears_history_and_rejects_obsolete_shell_ready(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new(cx);
    fixture.update(cx, |app, window, cx| {
        add_ssh_tab(app, "tab", "old-session", window, cx);
        app.tabs[0].profile = None;
        connect_tab(app, "old-session", window, cx);
        app.activate("tab".into(), window, cx);
    });
    let (target, snapshot, entry) = load_history(&mut fixture, cx);
    let new_session = fixture.update(cx, |app, window, cx| {
        app.disconnect(window, cx);
        assert!(app.inspector.read(cx).history_for_test().is_none());
        app.reconnect(window, cx);
        assert_eq!(app.tabs[0].id, "tab");
        assert_ne!(app.tabs[0].session_id, target.session_id);
        assert!(app.inspector.read(cx).history_for_test().is_none());
        assert!(app
            .inspector
            .read(cx)
            .insertion_text(&target, snapshot, entry)
            .is_none());
        let session = app.tabs[0].session_id.clone();
        app.handle_event(
            BackendEvent::SessionCreated {
                session_id: session.clone(),
            },
            window,
            cx,
        );
        app.handle_event(
            BackendEvent::ShellReady {
                session_id: target.session_id.clone(),
            },
            window,
            cx,
        );
        let result = app.tabs[0].terminal.update(cx, |terminal, cx| {
            terminal.insert_history("echo ready", &session, window, cx)
        });
        assert!(result.is_err());
        app.handle_event(
            BackendEvent::ShellReady {
                session_id: session.clone(),
            },
            window,
            cx,
        );
        let result = app.tabs[0].terminal.update(cx, |terminal, cx| {
            terminal.insert_history("echo ready", &session, window, cx)
        });
        assert!(result.is_ok());
        session
    });
    let writes = fixture
        .commands(cx)
        .into_iter()
        .filter_map(|command| match command {
            AppCommand::WriteInput { session_id, data } => Some((session_id, data)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(writes, [(new_session, b"echo ready".to_vec())]);
}

#[gpui::test]
fn app_layout_keeps_terminal_entity_across_wide_narrow_and_maximum_panels(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    let terminal = fixture.update(cx, |app, window, cx| {
        add_ssh_tab(app, "tab", "session", window, cx);
        connect_tab(app, "session", window, cx);
        app.activate("tab".into(), window, cx);
        app.tabs[0].terminal.clone()
    });
    let mut visual = VisualTestContext::from_window(fixture.window.into(), cx);
    for (width, height, sidebar, inspector, inline) in [
        (1280., 820., 200., 320., true),
        (860., 600., 200., 320., false),
        (1280., 820., 420., 480., false),
    ] {
        visual.simulate_resize(size(px(width), px(height)));
        fixture.update(cx, |app, window, cx| {
            app.settings.ui.session_panel_width = sidebar;
            app.settings.ui.inspector_panel_width = inspector;
            app.host_panel = cx.new(|_| Default::default());
            app.inspector_panel = cx.new(|_| Default::default());
            if app.inspector_visible {
                app.hide_inspector(window, cx);
            }
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        fixture.update(cx, |app, window, cx| {
            app.show_inspector(InspectorPage::History, window, cx)
        });
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let panel = visual
            .debug_bounds("app-inspector")
            .expect("inspector bounds");
        let hosts = visual
            .debug_bounds("app-hosts-panel")
            .expect("sidebar bounds");
        assert!(panel.size.width >= px(280.));
        assert!(panel.origin.x >= hosts.origin.x + hosts.size.width);
        assert!(panel.origin.x + panel.size.width <= px(width));
        assert!(panel.origin.y + panel.size.height <= px(height));
        assert_eq!(visual.debug_bounds("app-terminal-body").is_some(), inline);
        if inline {
            let body = visual.debug_bounds("app-terminal-body").unwrap();
            assert!(body.size.width >= px(440.));
            assert!(panel.size.width <= px(480.));
            assert!(body.origin.x + body.size.width <= panel.origin.x + px(2.));
        }
        fixture.update(cx, |app, _, _| {
            assert_eq!(app.inspector_inline, inline);
            assert_eq!(app.tabs[0].terminal, terminal);
        });
        fixture.press(
            if cfg!(target_os = "macos") {
                "cmd-shift-t"
            } else {
                "ctrl-alt-shift-t"
            },
            cx,
        );
        fixture.frame(cx);
        fixture.frame(cx);
        fixture.update(cx, |app, window, cx| {
            assert_eq!(app.tabs[0].terminal, terminal);
            assert!(terminal.read(cx).is_focused(window));
            if !inline {
                assert!(!app.inspector_visible);
            }
        });
        assert!(visual.debug_bounds("app-terminal-body").is_some());
    }
}

#[gpui::test]
fn narrow_history_fill_restores_body_before_filling_and_rejects_changed_target(
    cx: &mut TestAppContext,
) {
    let mut fixture = Fixture::new(cx);
    fixture.update(cx, |app, window, cx| {
        add_ssh_tab(app, "a", "session-a", window, cx);
        add_ssh_tab(app, "b", "session-b", window, cx);
        connect_tab(app, "session-a", window, cx);
        connect_tab(app, "session-b", window, cx);
        app.activate("a".into(), window, cx);
    });
    let mut visual = VisualTestContext::from_window(fixture.window.into(), cx);
    visual.simulate_resize(size(px(860.), px(600.)));
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let (target, snapshot, entry) = load_history(&mut fixture, cx);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert!(visual.debug_bounds("app-terminal-body").is_none());
    fixture.commands(cx);
    visual.update(|window, cx| {
        fixture.app.update(cx, |app, cx| {
            assert!(!app.inspector_inline);
            app.inspector_action(
                &InspectorAction::InsertHistory {
                    target: target.clone(),
                    snapshot_id: snapshot,
                    entry_id: entry,
                },
                window,
                cx,
            );
        });
        // The native frame loop delivers callbacks before drawing. Keep both in
        // one update so GPUI's automatic test redraw cannot restore the body first.
        assert!(window.simulate_next_frame(cx) > 0);
        fixture.app.update(cx, |app, cx| {
            assert!(!app.tabs[0].terminal.read(cx).is_focused(window));
        });
    });
    assert!(fixture
        .commands(cx)
        .into_iter()
        .all(|command| !matches!(command, AppCommand::WriteInput { .. })));
    assert!(visual.debug_bounds("app-terminal-body").is_some());
    fixture.frame(cx);
    let writes = fixture
        .commands(cx)
        .into_iter()
        .filter_map(|command| match command {
            AppCommand::WriteInput { session_id, data } => Some((session_id, data)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(writes, [("session-a".into(), b"echo fixture".to_vec())]);
    fixture.update(cx, |app, window, cx| {
        assert!(app.tabs[0].terminal.read(cx).is_focused(window))
    });

    let (target, snapshot, entry) = load_history(&mut fixture, cx);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    fixture.commands(cx);
    fixture.update(cx, |app, _, cx| {
        app.inspector.update(cx, |_, cx| {
            cx.emit(InspectorAction::InsertHistory {
                target: target.clone(),
                snapshot_id: snapshot,
                entry_id: entry,
            })
        });
    });
    cx.run_until_parked();
    fixture.update(cx, |app, window, cx| app.activate("b".into(), window, cx));
    fixture.frame(cx);
    fixture.frame(cx);
    assert!(fixture
        .commands(cx)
        .into_iter()
        .all(|command| !matches!(command, AppCommand::WriteInput { .. })));
    fixture.update(cx, |app, _, _| assert_eq!(app.active.as_deref(), Some("b")));
}

#[gpui::test]
fn app_resize_restores_saved_widths_without_replacing_resizable_state(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    let (hosts_state, inspector_state) = fixture.update(cx, |app, window, cx| {
        add_ssh_tab(app, "tab", "session", window, cx);
        app.settings.ui.session_panel_width = 420.;
        app.settings.ui.inspector_panel_width = 480.;
        app.activate("tab".into(), window, cx);
        (app.host_panel.clone(), app.inspector_panel.clone())
    });
    let mut visual = VisualTestContext::from_window(fixture.window.into(), cx);
    visual.simulate_resize(size(px(1600.), px(820.)));
    fixture.update(cx, |app, window, cx| {
        app.show_inspector(InspectorPage::History, window, cx)
    });
    for (width, expected_sidebar, inline) in [
        (1600., 420., true),
        (860., 378., false),
        (1280., 420., false),
        (1600., 420., true),
    ] {
        visual.simulate_resize(size(px(width), px(820.)));
        fixture.frame(cx);
        fixture.frame(cx);
        let sidebar = visual.debug_bounds("app-hosts-panel").unwrap();
        assert!(
            (f32::from(sidebar.size.width) - expected_sidebar).abs() <= 2.,
            "viewport {width}: {sidebar:?}"
        );
        if inline {
            let panel = visual.debug_bounds("app-inspector").unwrap();
            assert!(
                (f32::from(panel.size.width) - 480.).abs() <= 2.,
                "{panel:?}"
            );
        }
        fixture.update(cx, |app, _, _| {
            assert_eq!(app.host_panel, hosts_state);
            assert_eq!(app.inspector_panel, inspector_state);
            assert_eq!(app.settings.ui.session_panel_width, 420.);
            assert_eq!(app.settings.ui.inspector_panel_width, 480.);
            assert_eq!(app.inspector_inline, inline);
        });
    }
}

#[gpui::test]
fn inspector_cleanup_resumes_only_latest_page_after_closed_session_ready(cx: &mut TestAppContext) {
    let mut fixture = Fixture::new(cx);
    fixture.update(cx, |app, window, cx| {
        add_ssh_tab(app, "a", "session-a", window, cx);
        add_ssh_tab(app, "b", "session-b", window, cx);
        connect_tab(app, "session-a", window, cx);
        connect_tab(app, "session-b", window, cx);
        app.activate("a".into(), window, cx);
        app.show_inspector(InspectorPage::System, window, cx);
    });
    let (session, request) =
        only_inspection(fixture.inspection_commands(cx), InspectionKind::System);
    fixture.update(cx, |app, window, cx| {
        app.show_inspector(InspectorPage::Network, window, cx);
        app.show_inspector(InspectorPage::History, window, cx);
        app.show_inspector(InspectorPage::System, window, cx);
    });
    assert_cancel(fixture.inspection_commands(cx), &session, request);
    fixture.update(cx, |app, window, cx| {
        app.handle_event(
            BackendEvent::InspectionReady {
                session_id: "unrelated".into(),
            },
            window,
            cx,
        )
    });
    assert!(fixture.inspection_commands(cx).is_empty());
    fixture.update(cx, |app, window, cx| {
        app.handle_event(
            BackendEvent::InspectionReady {
                session_id: session.clone(),
            },
            window,
            cx,
        )
    });
    let (_, next_request) =
        only_inspection(fixture.inspection_commands(cx), InspectionKind::System);
    assert_ne!(next_request, request);
    fixture.update(cx, |app, window, cx| app.close_tab("a".into(), window, cx));
    let commands = fixture.inspection_commands(cx);
    assert_eq!(commands.len(), 2, "{commands:?}");
    assert!(
        matches!(&commands[0], AppCommand::CancelInspection { session_id, request_id } if session_id == "session-a" && *request_id == next_request)
    );
    let (current_session, current_request) = only_inspection(
        commands.into_iter().skip(1).collect(),
        InspectionKind::System,
    );
    assert_eq!(current_session, "session-b");
    fixture.update(cx, |app, window, cx| {
        app.handle_event(
            BackendEvent::InspectionResult {
                session_id: session.clone(),
                request_id: next_request,
                result: Ok(InspectionData::System(SystemSnapshot::default())),
            },
            window,
            cx,
        );
        app.handle_event(
            BackendEvent::InspectionReady {
                session_id: session,
            },
            window,
            cx,
        );
        assert!(app.inspector.read(cx).system_for_test().is_none());
        assert_eq!(app.active.as_deref(), Some("b"));
        app.hide_inspector(window, cx);
    });
    assert_cancel(
        fixture.inspection_commands(cx),
        &current_session,
        current_request,
    );
}

#[gpui::test]
fn narrow_history_fill_revalidates_snapshot_entry_and_terminal_identity(cx: &mut TestAppContext) {
    for invalidation in ["snapshot-id", "entry-id", "terminal", "snapshot-replaced"] {
        let mut fixture = Fixture::new(cx);
        fixture.update(cx, |app, window, cx| {
            add_ssh_tab(app, "tab", "session", window, cx);
            connect_tab(app, "session", window, cx);
            app.activate("tab".into(), window, cx);
        });
        let mut visual = VisualTestContext::from_window(fixture.window.into(), cx);
        visual.simulate_resize(size(px(860.), px(600.)));
        let (target, snapshot, entry) = load_history(&mut fixture, cx);
        fixture.commands(cx);
        fixture.update(cx, |app, window, cx| {
            app.inspector_action(
                &InspectorAction::InsertHistory {
                    target: target.clone(),
                    snapshot_id: if invalidation == "snapshot-id" {
                        snapshot + 10000
                    } else {
                        snapshot
                    },
                    entry_id: if invalidation == "entry-id" {
                        entry + 10000
                    } else {
                        entry
                    },
                },
                window,
                cx,
            );
            match invalidation {
                "terminal" => {
                    app.tabs[0].terminal = cx.new(|cx| {
                        TerminalView::new(
                            target.session_id.clone(),
                            app.backend_tx.clone(),
                            &app.settings,
                            window,
                            cx,
                        )
                    });
                    connect_tab(app, &target.session_id, window, cx);
                }
                "snapshot-replaced" => app
                    .inspector
                    .update(cx, |view, cx| view.refresh_for_test(cx)),
                _ => {}
            }
        });
        if invalidation == "snapshot-replaced" {
            let (session_id, request_id) = only_inspection(
                fixture.inspection_commands(cx),
                InspectionKind::History(HistorySource::Zsh),
            );
            assert_ne!(request_id, snapshot);
            fixture.update(cx, |app, window, cx| {
                app.handle_event(
                    BackendEvent::InspectionResult {
                        session_id: session_id.clone(),
                        request_id,
                        result: Ok(InspectionData::History(parse_history(
                            HistorySource::Zsh,
                            b": 200:0;echo replacement\n",
                            false,
                        ))),
                    },
                    window,
                    cx,
                );
                app.handle_event(BackendEvent::InspectionReady { session_id }, window, cx);
            });
        }
        fixture.frame(cx);
        fixture.frame(cx);
        assert!(visual.debug_bounds("app-terminal-body").is_some());
        assert!(
            fixture
                .commands(cx)
                .into_iter()
                .all(|command| !matches!(command, AppCommand::WriteInput { .. })),
            "invalidation {invalidation} wrote to terminal"
        );
    }
}

#[gpui::test]
fn app_root_modal_marker_blocks_terminal_clipboard_even_if_terminal_retains_focus(
    cx: &mut TestAppContext,
) {
    let mut fixture = Fixture::new(cx);
    fixture.update(cx, |app, window, cx| {
        add_ssh_tab(app, "tab", "session", window, cx);
        connect_tab(app, "session", window, cx);
        app.activate("tab".into(), window, cx);
        cx.write_to_clipboard(ClipboardItem::new_string("clipboard fixture".into()));
        app.dialog_open = true;
        assert!(!window.has_active_dialog(cx));
        assert!(app.tabs[0].terminal.read(cx).is_focused(window));
    });
    fixture.commands(cx);
    let paste = if cfg!(target_os = "macos") {
        "cmd-v"
    } else {
        "ctrl-shift-v"
    };
    fixture.press(paste, cx);
    assert!(fixture
        .commands(cx)
        .into_iter()
        .all(|command| !matches!(command, AppCommand::WriteInput { .. })));
    fixture.update(cx, |app, _, _| app.dialog_open = false);
    fixture.press(paste, cx);
    let writes = fixture
        .commands(cx)
        .into_iter()
        .filter_map(|command| match command {
            AppCommand::WriteInput { session_id, data } => Some((session_id, data)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(writes, [("session".into(), b"clipboard fixture".to_vec())]);
}

#[gpui::test]
fn shortcut_configuration_path_copies_from_real_help_and_settings_clicks(cx: &mut TestAppContext) {
    use std::time::Duration;

    let fixture = Fixture::new(cx);
    let config_dir = tempfile::Builder::new()
        .prefix("rsterm 快捷键 ")
        .tempdir()
        .unwrap();
    let config = ConfigManager::from_dir(config_dir.path().to_path_buf()).unwrap();
    let shortcuts = crate::config::ShortcutConfig::load(&config);
    let expected = shortcuts.keymap.path.display().to_string();
    cx.update(|cx| {
        cx.set_global(shortcuts);
        cx.set_reduce_motion(true);
    });
    for settings in [true, false, true, false] {
        fixture.update(cx, |app, window, cx| {
            if settings {
                app.open_settings(window, cx);
            } else {
                app.open_shortcuts(window, cx);
            }
        });
        let mut visual = VisualTestContext::from_window(fixture.window.into(), cx);
        for viewport in [size(px(1280.), px(820.)), size(px(860.), px(600.))] {
            visual.simulate_resize(viewport);
            visual.update(|window, cx| window.draw(cx).clear(cx));
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let button = visual
                .debug_bounds("shortcuts-copy-path-button")
                .expect("copy path button is rendered");
            assert_eq!(button.size.width, px(88.));
            assert!(button.size.height > px(0.));
            assert!(button.left() >= px(0.) && button.right() <= viewport.width);
            if settings {
                let footer = visual.debug_bounds("settings-footer").unwrap();
                assert!(button.bottom() <= footer.top());
                visual.simulate_event(gpui_kit::ScrollWheelEvent {
                    position: button.center(),
                    delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-40.))),
                    ..Default::default()
                });
                visual.update(|window, cx| window.draw(cx).clear(cx));
            }
            let button = visual.debug_bounds("shortcuts-copy-path-button").unwrap();
            for _ in 0..2 {
                visual.update(|_, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string("before click".into()));
                });
                visual.simulate_click(button.center(), Default::default());
                visual.update(|window, cx| {
                    window.draw(cx).clear(cx);
                    assert_eq!(
                        cx.read_from_clipboard().and_then(|item| item.text()),
                        Some(expected.clone()),
                        "copy did not fire in settings={settings}"
                    );
                });
                assert!(visual.debug_bounds("shortcuts-copy-path-copied").is_some());
                assert!(visual.debug_bounds("shortcuts-copy-path-button").is_none());
                cx.executor().advance_clock(Duration::from_secs(1));
                cx.run_until_parked();
            }
            visual.update(|window, cx| window.draw(cx).clear(cx));
            assert!(visual.debug_bounds("shortcuts-copy-path-copied").is_some());
            cx.executor().advance_clock(Duration::from_secs(1));
            cx.run_until_parked();
            visual.update(|window, cx| window.draw(cx).clear(cx));
            assert!(visual.debug_bounds("shortcuts-copy-path-copied").is_none());
            assert!(visual.debug_bounds("shortcuts-copy-path-button").is_some());
        }
        visual.update(|_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string("before closing".into()));
        });
        let button = visual.debug_bounds("shortcuts-copy-path-button").unwrap();
        visual.simulate_click(button.center(), Default::default());
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(visual.debug_bounds("shortcuts-copy-path-copied").is_some());
        fixture.update(cx, |app, window, cx| app.dismiss_dialog(window, cx));
        fixture.frame(cx);
        cx.executor().advance_clock(Duration::from_secs(3));
        cx.run_until_parked();
        visual.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some(expected.clone())
            );
        });
    }
}

#[gpui::test]
fn asynchronous_settings_save_merges_live_sidebar_and_inspector_widths(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    fixture.update(cx, |app, window, cx| {
        let mut draft = app.settings.clone();
        draft.app.font_size = 20.;
        draft.app.theme = "nord".into();
        app.open_settings(window, cx);
        app.save_settings(draft, window, cx);
        assert!(app.settings_busy);
        app.settings.ui.session_panel_width = 316.;
        app.settings.ui.inspector_panel_width = 468.;
        app.persist_layout(window, cx);
        assert!(app.layout_dirty);
    });
    cx.run_until_parked();
    fixture.update(cx, |app, _, _| {
        assert!(!app.settings_busy);
        assert!(!app.layout_dirty);
        assert_eq!(app.settings.ui.session_panel_width, 316.);
        assert_eq!(app.settings.ui.inspector_panel_width, 468.);
        assert_eq!(app.settings.app.font_size, 20.);
        assert_eq!(app.settings.app.theme, "nord");
        let restored = ConfigManager::from_dir(app.config.base_dir().to_path_buf())
            .unwrap()
            .load_settings()
            .unwrap();
        assert_eq!(
            toml::to_string(&restored).unwrap(),
            toml::to_string(&app.settings).unwrap()
        );
    });
}
