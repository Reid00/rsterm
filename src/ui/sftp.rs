use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Local};
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_resizable,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    resizable_panel, ActiveTheme, Disableable, Sizable, WindowExt,
};
use gpui_kit::{
    div, prelude::*, px, AnyElement, AnyWindowHandle, Context, Entity, EventEmitter, Render,
    Subscription, Task, Window,
};
use tokio::sync::mpsc;

use crate::{
    backend::{AppCommand, BackendEvent, FileEntryInfo, TransferOutcome},
    config::AppSettings,
};

use super::transfers::{format_bytes, TransferDirection, TransferQueueView, TransferRequest};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Local,
    Remote,
}

impl Side {
    fn label(self) -> &'static str {
        match self {
            Self::Local => "本地",
            Self::Remote => "远程",
        }
    }
}

struct Pane {
    input: Entity<InputState>,
    path: String,
    requested_path: String,
    entries: Vec<FileEntryInfo>,
    selected: Option<String>,
    loading: bool,
    error: Option<String>,
    request_id: u64,
}

impl Pane {
    fn selected_entry(&self) -> Option<&FileEntryInfo> {
        self.entries
            .iter()
            .find(|entry| Some(&entry.path) == self.selected.as_ref())
    }
}

#[derive(Clone)]
enum PendingDialog {
    Mkdir {
        side: Side,
        parent: String,
    },
    Rename {
        side: Side,
        parent: String,
        entry: FileEntryInfo,
    },
    Delete {
        side: Side,
        parent: String,
        entry: FileEntryInfo,
    },
    Chmod {
        side: Side,
        parent: String,
        entry: FileEntryInfo,
    },
    Overwrite(TransferRequest),
}

impl PendingDialog {
    fn title(&self) -> String {
        match self {
            Self::Mkdir { side, .. } => format!("新建{}文件夹", side.label()),
            Self::Rename { entry, .. } => format!("重命名 {}", entry.name),
            Self::Delete { entry, .. } => format!("删除 {}？", entry.name),
            Self::Chmod { entry, .. } => format!("{} 的权限", entry.name),
            Self::Overwrite(_) => "覆盖已有文件？".into(),
        }
    }

    fn confirm_label(&self) -> &'static str {
        match self {
            Self::Mkdir { .. } => "新建文件夹",
            Self::Rename { .. } => "重命名",
            Self::Delete { .. } => "删除",
            Self::Chmod { .. } => "应用权限",
            Self::Overwrite(_) => "覆盖文件",
        }
    }

    fn needs_input(&self) -> bool {
        !matches!(self, Self::Delete { .. } | Self::Overwrite(_))
    }
}

enum LocalOperation {
    Mkdir(PathBuf),
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    Delete {
        path: PathBuf,
        is_dir: bool,
        is_symlink: bool,
    },
    Chmod {
        path: PathBuf,
        mode: u32,
    },
}

pub enum FileBrowserEvent {
    DialogClosed,
}

pub struct FileBrowserView {
    session_id: String,
    command_tx: mpsc::Sender<AppCommand>,
    local: Pane,
    remote: Pane,
    remote_home: Option<String>,
    connected: bool,
    show_hidden: bool,
    confirm_overwrite: bool,
    active_side: Side,
    next_request_id: u64,
    pending_operations: HashSet<u64>,
    dialog: Option<PendingDialog>,
    dialog_window: Option<AnyWindowHandle>,
    dialog_input: Entity<InputState>,
    dialog_error: Option<String>,
    transfers: Entity<TransferQueueView>,
    tasks: Vec<Task<()>>,
    remote_tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FileBrowserEvent> for FileBrowserView {}

impl FileBrowserView {
    pub fn new(
        session_id: String,
        command_tx: mpsc::Sender<AppCommand>,
        settings: &AppSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let local_path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let local_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("本地目录")
                .default_value(local_path.display().to_string())
        });
        let remote_input = cx.new(|cx| InputState::new(window, cx).placeholder("远程目录"));
        let dialog_input = cx.new(|cx| InputState::new(window, cx).placeholder("名称"));
        let subscriptions = vec![
            cx.subscribe_in(&local_input, window, |view, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let path = view.local.input.read(cx).value().to_string();
                    view.navigate(Side::Local, &path, window, cx);
                }
            }),
            cx.subscribe_in(&remote_input, window, |view, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let path = view.remote.input.read(cx).value().to_string();
                    view.navigate(Side::Remote, &path, window, cx);
                }
            }),
        ];
        let transfers = cx.new(|_| TransferQueueView::new(session_id.clone(), command_tx.clone()));
        let mut view = Self {
            session_id,
            command_tx,
            local: Pane {
                input: local_input,
                path: local_path.display().to_string(),
                requested_path: local_path.display().to_string(),
                entries: Vec::new(),
                selected: None,
                loading: false,
                error: None,
                request_id: 0,
            },
            remote: Pane {
                input: remote_input,
                path: String::new(),
                requested_path: String::new(),
                entries: Vec::new(),
                selected: None,
                loading: false,
                error: None,
                request_id: 0,
            },
            remote_home: None,
            connected: false,
            show_hidden: settings.sftp.show_hidden_files,
            confirm_overwrite: settings.sftp.confirm_overwrite,
            active_side: Side::Local,
            next_request_id: 1,
            pending_operations: HashSet::new(),
            dialog: None,
            dialog_window: None,
            dialog_input,
            dialog_error: None,
            transfers,
            tasks: Vec::new(),
            remote_tasks: Vec::new(),
            _subscriptions: subscriptions,
        };
        view.load_local(local_path, window, cx);
        view
    }

    pub fn apply_settings(&mut self, settings: &AppSettings, cx: &mut Context<Self>) {
        self.show_hidden = settings.sftp.show_hidden_files;
        self.confirm_overwrite = settings.sftp.confirm_overwrite;
        if !self.show_hidden {
            for pane in [&mut self.local, &mut self.remote] {
                if pane
                    .selected_entry()
                    .is_some_and(|entry| entry.name.starts_with('.'))
                {
                    pane.selected = None;
                }
            }
        }
        cx.notify();
    }

    pub fn set_connected(&mut self, connected: bool, cx: &mut Context<Self>) {
        self.connected = connected;
        let ready = self.remote_ready();
        self.transfers
            .update(cx, |view, cx| view.set_connected(ready, cx));
        if !connected {
            self.remote_tasks.clear();
            self.remote.loading = false;
            self.remote_home = None;
            self.pending_operations.clear();
            self.remote.request_id = self.next_id();
            if self.dialog.take().is_some() {
                self.dialog_error = None;
                if let Some(window) = self.dialog_window.take() {
                    let view = cx.entity().downgrade();
                    cx.defer(move |cx| {
                        let _ = window.update(cx, |_, window, cx| window.close_dialog(cx));
                        let _ = view.update(cx, |_, cx| cx.emit(FileBrowserEvent::DialogClosed));
                    });
                }
            }
        }
        cx.notify();
    }

    pub fn handle_event(
        &mut self,
        event: &BackendEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            BackendEvent::SftpReady { session_id, cwd } if session_id == &self.session_id => {
                self.set_connected(true, cx);
                match normalize_remote_path(cwd, "/", None) {
                    Ok(home) => {
                        self.remote_home = Some(home.clone());
                        self.transfers
                            .update(cx, |view, cx| view.set_connected(true, cx));
                        let path = if self.remote.path.is_empty() {
                            home
                        } else {
                            self.remote.path.clone()
                        };
                        self.load_remote(path, window, cx);
                    }
                    Err(error) => self.remote.error = Some(error),
                }
            }
            BackendEvent::SftpError { session_id, error } if session_id == &self.session_id => {
                self.remote.loading = false;
                self.remote.error = Some(error.clone());
            }
            BackendEvent::SftpListResult {
                session_id,
                request_id,
                path,
                result,
            } if session_id == &self.session_id && *request_id == self.remote.request_id => {
                self.remote.loading = false;
                match result {
                    Ok(entries) => {
                        self.remote.path = path.clone();
                        self.remote.entries = entries.clone();
                        sort_entries(&mut self.remote.entries);
                        self.remote.selected = None;
                        self.remote.error = None;
                        self.remote
                            .input
                            .update(cx, |input, cx| input.set_value(path.clone(), window, cx));
                    }
                    Err(error) => self.remote.error = Some(error.clone()),
                }
            }
            BackendEvent::SftpOperationResult {
                session_id,
                request_id,
                path,
                result,
            } if session_id == &self.session_id && self.pending_operations.remove(request_id) => {
                let current_path = if self.remote.loading {
                    &self.remote.requested_path
                } else {
                    &self.remote.path
                };
                match result {
                    Ok(()) if path == current_path => self.refresh(Side::Remote, window, cx),
                    Ok(()) => {}
                    Err(error) => self.remote.error = Some(error.clone()),
                }
            }
            BackendEvent::SftpTransferComplete {
                session_id,
                outcome: TransferOutcome::Completed,
                ..
            } if session_id == &self.session_id => {
                self.refresh(Side::Local, window, cx);
                if self.remote_ready() {
                    self.refresh(Side::Remote, window, cx);
                }
            }
            _ => {}
        }
        self.transfers
            .update(cx, |view, cx| view.handle_event(event, cx));
        cx.notify();
    }

    fn remote_ready(&self) -> bool {
        self.connected && self.remote_home.is_some()
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }

    fn pane(&self, side: Side) -> &Pane {
        match side {
            Side::Local => &self.local,
            Side::Remote => &self.remote,
        }
    }

    fn pane_mut(&mut self, side: Side) -> &mut Pane {
        match side {
            Side::Local => &mut self.local,
            Side::Remote => &mut self.remote,
        }
    }

    fn navigate(&mut self, side: Side, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        match side {
            Side::Local => {
                let base = if self.local.loading {
                    &self.local.requested_path
                } else {
                    &self.local.path
                };
                let path = local_navigation_path(path, Path::new(base));
                self.load_local(path, window, cx);
            }
            Side::Remote => {
                if !self.remote_ready() {
                    return;
                }
                let base = if self.remote.loading {
                    &self.remote.requested_path
                } else {
                    &self.remote.path
                };
                match normalize_remote_path(path, base, self.remote_home.as_deref()) {
                    Ok(path) => self.load_remote(path, window, cx),
                    Err(error) => self.remote.error = Some(error),
                }
            }
        }
        cx.notify();
    }

    fn parent(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        let path = match side {
            Side::Local => Path::new(&self.local.path)
                .parent()
                .unwrap_or(Path::new("/"))
                .display()
                .to_string(),
            Side::Remote => remote_parent(&self.remote.path),
        };
        self.navigate(side, &path, window, cx);
    }

    fn refresh(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.pane(side);
        let path = if pane.loading {
            pane.requested_path.clone()
        } else {
            pane.path.clone()
        };
        if !path.is_empty() {
            self.navigate(side, &path, window, cx);
        }
    }

    fn load_local(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let request_id = self.next_id();
        self.local.request_id = request_id;
        self.local.requested_path = path.display().to_string();
        self.local.loading = true;
        self.local.error = None;
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { read_local_directory(&path) })
                .await;
            let _ = view.update_in(cx, |view, window, cx| {
                if request_id != view.local.request_id {
                    return;
                }
                view.local.loading = false;
                match result {
                    Ok((path, entries)) => {
                        view.local.path = path.display().to_string();
                        view.local.entries = entries;
                        view.local.selected = None;
                        view.local.error = None;
                        view.local.input.update(cx, |input, cx| {
                            input.set_value(view.local.path.clone(), window, cx)
                        });
                    }
                    Err(error) => view.local.error = Some(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn load_remote(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let request_id = self.next_id();
        self.remote.request_id = request_id;
        self.remote.requested_path = path.clone();
        self.remote
            .input
            .update(cx, |input, cx| input.set_value(path.clone(), window, cx));
        self.remote.loading = true;
        self.remote.error = None;
        self.send_command(
            AppCommand::SftpListDir {
                session_id: self.session_id.clone(),
                request_id,
                path,
            },
            window,
            cx,
        );
    }

    fn send_command(&mut self, command: AppCommand, window: &mut Window, cx: &mut Context<Self>) {
        let command_tx = self.command_tx.clone();
        self.remote_tasks.retain(|task| !task.is_ready());
        self.remote_tasks
            .push(cx.spawn_in(window, async move |view, cx| {
                if command_tx.send(command).await.is_err() {
                    let _ = view.update_in(cx, |view, _, cx| {
                        view.remote.loading = false;
                        view.remote.error = Some("后台命令通道已关闭".into());
                        cx.notify();
                    });
                }
            }));
    }

    fn open_operation(&mut self, kind: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let side = self.active_side;
        let pane = self.pane(side);
        if pane.loading || (side == Side::Remote && !self.remote_ready()) {
            return;
        }
        let parent = pane.path.clone();
        let entry = pane.selected_entry().cloned();
        let dialog = match (kind, entry) {
            ("mkdir", _) => PendingDialog::Mkdir { side, parent },
            ("rename", Some(entry)) => PendingDialog::Rename {
                side,
                parent,
                entry,
            },
            ("delete", Some(entry)) => PendingDialog::Delete {
                side,
                parent,
                entry,
            },
            ("chmod", Some(entry)) => PendingDialog::Chmod {
                side,
                parent,
                entry,
            },
            _ => return,
        };
        self.open_dialog(dialog, window, cx);
    }

    fn open_dialog(&mut self, pending: PendingDialog, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            let side = match &pending {
                PendingDialog::Mkdir { side, .. }
                | PendingDialog::Rename { side, .. }
                | PendingDialog::Delete { side, .. }
                | PendingDialog::Chmod { side, .. } => *side,
                PendingDialog::Overwrite(request) => match request.direction {
                    TransferDirection::Upload => Side::Remote,
                    TransferDirection::Download => Side::Local,
                },
            };
            self.pane_mut(side).error = Some("请关闭当前对话框后重试".into());
            cx.notify();
            return;
        }
        let initial = match &pending {
            PendingDialog::Rename { entry, .. } => entry.name.clone(),
            PendingDialog::Chmod { entry, .. } => {
                format!("{:04o}", entry.permissions.unwrap_or(0o644) & 0o7777)
            }
            _ => String::new(),
        };
        self.dialog_input
            .update(cx, |input, cx| input.set_value(initial, window, cx));
        self.dialog = Some(pending);
        self.dialog_window = Some(window.window_handle());
        self.dialog_error = None;
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let Some(entity) = view.upgrade() else {
                return dialog;
            };
            let state = entity.read(cx);
            let Some(pending) = state.dialog.clone() else {
                return dialog;
            };
            let input = state.dialog_input.clone();
            let error = state.dialog_error.clone();
            let mut body = div().flex().flex_col().gap_3();
            let description = match &pending {
                PendingDialog::Mkdir { parent, .. } => format!("在 {parent} 中新建文件夹"),
                PendingDialog::Rename { entry, .. } => {
                    format!("为 {} 输入新名称。", entry.name)
                }
                PendingDialog::Delete { entry, .. } if entry.is_dir && !entry.is_symlink => {
                    "仅支持删除空文件夹，此操作无法撤销。".into()
                }
                PendingDialog::Delete { .. } => "此操作无法撤销。".into(),
                PendingDialog::Chmod { .. } => "输入八进制权限，例如 0644 或 0755。".into(),
                PendingDialog::Overwrite(request) => {
                    let target = match request.direction {
                        TransferDirection::Upload => request.remote_path.clone(),
                        TransferDirection::Download => request.local_path.display().to_string(),
                    };
                    format!("{target} 已存在，是否覆盖其内容？")
                }
            };
            body = body.child(div().text_sm().child(description));
            if pending.needs_input() {
                body = body.child(Input::new(&input).id("file-operation-input"));
            }
            if let Some(error) = error {
                body = body.child(div().text_sm().text_color(cx.theme().danger).child(error));
            }
            let confirm_view = view.clone();
            let close_view = view.clone();
            let destructive = matches!(
                pending,
                PendingDialog::Delete { .. } | PendingDialog::Overwrite(_)
            );
            dialog
                .title(pending.title())
                .child(body)
                .footer(
                    DialogFooter::new()
                        .child(
                            div()
                                .w(px(84.))
                                .h(px(32.))
                                .child(DialogClose::new().trigger(|button| button.label("取消"))),
                        )
                        .child(
                            div().w(px(160.)).h(px(32.)).child(
                                DialogAction::new().child(
                                    Button::new("confirm-file-operation")
                                        .label(pending.confirm_label())
                                        .when(destructive, |button| button.danger())
                                        .when(!destructive, |button| button.primary()),
                                ),
                            ),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    confirm_view
                        .update(cx, |view, cx| view.confirm_dialog(window, cx))
                        .unwrap_or(true)
                })
                .on_close(move |_, _, cx| {
                    let _ = close_view.update(cx, |view, cx| {
                        view.dialog = None;
                        view.dialog_window = None;
                        view.dialog_error = None;
                        cx.emit(FileBrowserEvent::DialogClosed);
                        cx.notify();
                    });
                })
        });
        cx.notify();
    }

    fn confirm_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(pending) = self.dialog.clone() else {
            return true;
        };
        let value = self.dialog_input.read(cx).value().to_string();
        if matches!(
            pending,
            PendingDialog::Mkdir { .. } | PendingDialog::Rename { .. }
        ) {
            if let Err(error) = validate_filename(&value) {
                self.dialog_error = Some(error);
                window.refresh();
                cx.notify();
                return false;
            }
        }
        let mode = if matches!(pending, PendingDialog::Chmod { .. }) {
            match parse_mode(&value) {
                Ok(mode) => Some(mode),
                Err(error) => {
                    self.dialog_error = Some(error);
                    window.refresh();
                    cx.notify();
                    return false;
                }
            }
        } else {
            None
        };
        let side = match &pending {
            PendingDialog::Mkdir { side, .. }
            | PendingDialog::Rename { side, .. }
            | PendingDialog::Delete { side, .. }
            | PendingDialog::Chmod { side, .. } => *side,
            PendingDialog::Overwrite(_) => Side::Remote,
        };
        if side == Side::Remote && !self.remote_ready() {
            self.dialog_error = Some("SFTP 尚未连接".into());
            window.refresh();
            cx.notify();
            return false;
        }
        match pending {
            PendingDialog::Overwrite(request) => self.start_transfer(request, true, window, cx),
            PendingDialog::Mkdir { side, parent } => match side {
                Side::Local => self.local_operation(
                    parent.clone(),
                    LocalOperation::Mkdir(Path::new(&parent).join(&value)),
                    window,
                    cx,
                ),
                Side::Remote => {
                    let request_id = self.operation_id();
                    self.send_command(
                        AppCommand::SftpMkdir {
                            session_id: self.session_id.clone(),
                            request_id,
                            path: remote_join(&parent, &value),
                        },
                        window,
                        cx,
                    );
                }
            },
            PendingDialog::Rename {
                side,
                parent,
                entry,
            } => match side {
                Side::Local => self.local_operation(
                    parent.clone(),
                    LocalOperation::Rename {
                        from: PathBuf::from(entry.path),
                        to: Path::new(&parent).join(&value),
                    },
                    window,
                    cx,
                ),
                Side::Remote => {
                    let request_id = self.operation_id();
                    self.send_command(
                        AppCommand::SftpRename {
                            session_id: self.session_id.clone(),
                            request_id,
                            from: entry.path,
                            to: remote_join(&parent, &value),
                        },
                        window,
                        cx,
                    );
                }
            },
            PendingDialog::Delete {
                side,
                parent,
                entry,
            } => match side {
                Side::Local => self.local_operation(
                    parent,
                    LocalOperation::Delete {
                        path: PathBuf::from(entry.path),
                        is_dir: entry.is_dir,
                        is_symlink: entry.is_symlink,
                    },
                    window,
                    cx,
                ),
                Side::Remote => {
                    let request_id = self.operation_id();
                    self.send_command(
                        AppCommand::SftpDelete {
                            session_id: self.session_id.clone(),
                            request_id,
                            path: entry.path,
                        },
                        window,
                        cx,
                    );
                }
            },
            PendingDialog::Chmod {
                side,
                parent,
                entry,
            } => match side {
                Side::Local => self.local_operation(
                    parent,
                    LocalOperation::Chmod {
                        path: PathBuf::from(entry.path),
                        mode: mode.unwrap(),
                    },
                    window,
                    cx,
                ),
                Side::Remote => {
                    let request_id = self.operation_id();
                    self.send_command(
                        AppCommand::SftpChmod {
                            session_id: self.session_id.clone(),
                            request_id,
                            path: entry.path,
                            mode: mode.unwrap(),
                        },
                        window,
                        cx,
                    );
                }
            },
        }
        self.dialog = None;
        self.dialog_window = None;
        self.dialog_error = None;
        cx.notify();
        true
    }

    fn operation_id(&mut self) -> u64 {
        let id = self.next_id();
        self.pending_operations.insert(id);
        id
    }

    fn local_operation(
        &mut self,
        parent: String,
        operation: LocalOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.local.error = None;
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { perform_local_operation(operation) })
                .await;
            let _ = view.update_in(cx, |view, window, cx| {
                let current_path = if view.local.loading {
                    &view.local.requested_path
                } else {
                    &view.local.path
                };
                match result {
                    Ok(()) if current_path == &parent => view.refresh(Side::Local, window, cx),
                    Ok(()) => {}
                    Err(error) => view.local.error = Some(error),
                }
                cx.notify();
            });
        }));
    }

    fn request_upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.local.selected_entry().cloned() else {
            return;
        };
        if entry.is_dir {
            self.local.error = Some("暂不支持上传文件夹，请选择文件。".into());
            cx.notify();
            return;
        }
        if !self.remote_ready() || self.remote.loading || self.remote.path.is_empty() {
            return;
        }
        let target = self
            .remote
            .entries
            .iter()
            .find(|target| target.name == entry.name);
        if target.is_some_and(|target| target.is_dir) {
            self.remote.error = Some("已存在同名文件夹，请先重命名本地文件再上传。".into());
            cx.notify();
            return;
        }
        let exists = target.is_some();
        let remote_path = remote_join(&self.remote.path, &entry.name);
        let local_path = PathBuf::from(entry.path);
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn_in(window, async move |view, cx| {
            let inspected_path = local_path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { inspect_local_file(&inspected_path) })
                .await;
            let _ = view.update_in(cx, |view, window, cx| {
                match result {
                    Ok(size) => view.prepare_transfer(
                        TransferRequest {
                            direction: TransferDirection::Upload,
                            local_path,
                            remote_path,
                            total_bytes: size,
                        },
                        exists,
                        window,
                        cx,
                    ),
                    Err(error) => view.local.error = Some(error),
                }
                cx.notify();
            });
        }));
    }

    fn request_download(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.remote.selected_entry().cloned() else {
            return;
        };
        if entry.is_dir {
            self.remote.error = Some("暂不支持下载文件夹，请选择文件。".into());
            cx.notify();
            return;
        }
        if !self.remote_ready() || self.local.loading {
            return;
        }
        if let Err(error) = validate_filename(&entry.name) {
            self.remote.error = Some(format!("无法下载此文件名：{error}"));
            cx.notify();
            return;
        }
        let local_path = Path::new(&self.local.path).join(&entry.name);
        let request = TransferRequest {
            direction: TransferDirection::Download,
            local_path: local_path.clone(),
            remote_path: entry.path,
            total_bytes: entry.size,
        };
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn_in(window, async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match fs::symlink_metadata(&local_path) {
                        Ok(metadata) if metadata.is_dir() => {
                            Err("本地已存在同名文件夹，请先重命名远程文件再下载。".into())
                        }
                        Ok(_) => Ok(true),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                        Err(error) => Err(format!("无法检查 {}：{error}", local_path.display())),
                    }
                })
                .await;
            let _ = view.update_in(cx, |view, window, cx| {
                match result {
                    Ok(exists) => view.prepare_transfer(request, exists, window, cx),
                    Err(error) => view.local.error = Some(error),
                }
                cx.notify();
            });
        }));
    }

    fn prepare_transfer(
        &mut self,
        request: TransferRequest,
        exists: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.remote_ready() {
            self.remote.error = Some("SFTP 尚未连接".into());
        } else if exists && self.confirm_overwrite {
            if self.dialog.is_some() {
                self.remote.error = Some("请先关闭当前弹窗，再重试传输。".into());
            } else {
                self.open_dialog(PendingDialog::Overwrite(request), window, cx);
            }
        } else {
            self.start_transfer(request, !self.confirm_overwrite, window, cx);
        }
        cx.notify();
    }

    fn start_transfer(
        &mut self,
        request: TransferRequest,
        overwrite: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.transfers
            .update(cx, |view, cx| view.start(request, overwrite, window, cx));
    }

    fn render_pane(&self, side: Side, cx: &mut Context<Self>) -> AnyElement {
        let pane = self.pane(side);
        let available = side == Side::Local || self.remote_ready();
        let selected = pane.selected.clone();
        let rows: Vec<_> = pane
            .entries
            .iter()
            .filter(|entry| self.show_hidden || !entry.name.starts_with('.'))
            .cloned()
            .collect();
        let mut listing = div()
            .id(match side {
                Side::Local => "local-files",
                Side::Remote => "remote-files",
            })
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        for (index, entry) in rows.iter().enumerate() {
            let path = entry.path.clone();
            let is_dir = entry.is_dir;
            let name = format!(
                "{}{}{}",
                entry.name,
                if entry.is_dir { "/" } else { "" },
                if entry.is_symlink { " ↗" } else { "" }
            );
            let modified = entry
                .modified
                .map(|time| {
                    DateTime::<Local>::from(time)
                        .format("%y-%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_else(|| "—".into());
            let permissions = entry
                .permissions
                .map(|mode| format!("{:04o}", mode & 0o7777))
                .unwrap_or_else(|| "—".into());
            listing = listing.child(
                ListItem::new((
                    match side {
                        Side::Local => "local-entry",
                        Side::Remote => "remote-entry",
                    },
                    index,
                ))
                .selected(selected.as_deref() == Some(entry.path.as_str()))
                .disabled(!available || pane.loading)
                .accessibility_label(format!(
                    "{} {}",
                    if is_dir { "文件夹" } else { "文件" },
                    entry.name
                ))
                .px_2()
                .py_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_xs()
                        .child(div().flex_1().min_w(px(100.)).truncate().child(name))
                        .child(
                            div()
                                .w(px(66.))
                                .flex_none()
                                .text_right()
                                .text_color(cx.theme().muted_foreground)
                                .child(if is_dir {
                                    "—".into()
                                } else {
                                    format_bytes(entry.size)
                                }),
                        )
                        .child(
                            div()
                                .w(px(108.))
                                .flex_none()
                                .text_color(cx.theme().muted_foreground)
                                .child(modified),
                        )
                        .child(
                            div()
                                .w(px(42.))
                                .flex_none()
                                .text_right()
                                .text_color(cx.theme().muted_foreground)
                                .child(permissions),
                        ),
                )
                .on_click(cx.listener(
                    move |view, event: &gpui_kit::ClickEvent, window, cx| {
                        view.active_side = side;
                        view.pane_mut(side).selected = Some(path.clone());
                        if event.click_count() == 2 && is_dir {
                            view.navigate(side, &path, window, cx);
                        }
                        cx.notify();
                    },
                )),
            );
        }
        if rows.is_empty() && !pane.loading {
            let message = if !available {
                "连接后可浏览远程文件"
            } else if side == Side::Remote && pane.path.is_empty() {
                "正在读取远程主目录…"
            } else {
                "此目录为空"
            };
            listing = listing.child(
                div()
                    .p_4()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(message),
            );
        }
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .border_t_2()
            .border_color(if self.active_side == side {
                cx.theme().primary
            } else {
                cx.theme().border
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .flex_1()
                            .child(side.label()),
                    )
                    .child(
                        Button::new(match side {
                            Side::Local => "local-parent",
                            Side::Remote => "remote-parent",
                        })
                        .label("上级")
                        .ghost()
                        .small()
                        .disabled(
                            !available || pane.loading || pane.path.is_empty() || pane.path == "/",
                        )
                        .on_click(cx.listener(
                            move |view, _, window, cx| {
                                view.active_side = side;
                                view.parent(side, window, cx);
                            },
                        )),
                    )
                    .child(
                        Button::new(match side {
                            Side::Local => "local-refresh",
                            Side::Remote => "remote-refresh",
                        })
                        .label("刷新")
                        .ghost()
                        .small()
                        .disabled(!available || pane.path.is_empty())
                        .on_click(cx.listener(
                            move |view, _, window, cx| {
                                view.active_side = side;
                                view.refresh(side, window, cx);
                            },
                        )),
                    ),
            )
            .child(
                div().px_2().pb_2().child(
                    Input::new(&pane.input)
                        .id(match side {
                            Side::Local => "local-path",
                            Side::Remote => "remote-path",
                        })
                        .small()
                        .disabled(!available),
                ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .border_y_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().flex_1().min_w(px(100.)).child("名称"))
                    .child(div().w(px(66.)).flex_none().text_right().child("大小"))
                    .child(div().w(px(108.)).flex_none().child("修改时间"))
                    .child(div().w(px(42.)).flex_none().text_right().child("权限")),
            )
            .when(pane.loading, |this| {
                this.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("正在读取…"),
                )
            })
            .when_some(pane.error.clone(), |this, error| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(listing)
            .child(
                div()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{} 项{}",
                        rows.len(),
                        if self.show_hidden {
                            " · 已显示隐藏文件"
                        } else {
                            ""
                        }
                    )),
            )
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(move |view, _, _, cx| {
                    view.active_side = side;
                    cx.notify();
                }),
            )
            .into_any_element()
    }
}

impl Render for FileBrowserView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.pane(self.active_side);
        let operation_disabled = active.loading
            || (self.active_side == Side::Remote && !self.remote_ready())
            || active.path.is_empty();
        let selection_disabled = operation_disabled || active.selected_entry().is_none();
        let chmod_disabled = selection_disabled || (self.active_side == Side::Local && !cfg!(unix));
        let local = self.render_pane(Side::Local, cx);
        let remote = self.render_pane(Side::Remote, cx);
        let mut toolbar = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap_1()
            .px_2()
            .py_1()
            .flex_none()
            .child(
                Button::new("sftp-upload")
                    .label("上传 →")
                    .small()
                    .disabled(
                        !self.remote_ready()
                            || self.remote.path.is_empty()
                            || self.remote.loading
                            || self.local.loading
                            || self.local.selected_entry().is_none(),
                    )
                    .on_click(cx.listener(|view, _, window, cx| view.request_upload(window, cx))),
            )
            .child(
                Button::new("sftp-download")
                    .label("← 下载")
                    .small()
                    .disabled(
                        !self.remote_ready()
                            || self.remote.loading
                            || self.local.loading
                            || self.remote.selected_entry().is_none(),
                    )
                    .on_click(cx.listener(|view, _, window, cx| view.request_download(window, cx))),
            );
        for (kind, label) in [
            ("mkdir", "新建文件夹"),
            ("rename", "重命名"),
            ("delete", "删除"),
            ("chmod", "权限"),
        ] {
            toolbar = toolbar.child(
                Button::new(kind)
                    .label(label)
                    .ghost()
                    .small()
                    .disabled(match kind {
                        "mkdir" => operation_disabled,
                        "chmod" => chmod_disabled,
                        _ => selection_disabled,
                    })
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.open_operation(kind, window, cx)
                    })),
            );
        }
        toolbar = toolbar.child(div().flex_1()).child(
            Button::new("sftp-hidden-files")
                .label(if self.show_hidden {
                    "不显示隐藏文件"
                } else {
                    "显示隐藏文件"
                })
                .ghost()
                .small()
                .on_click(cx.listener(|view, _, _, cx| {
                    view.show_hidden = !view.show_hidden;
                    if !view.show_hidden {
                        for pane in [&mut view.local, &mut view.remote] {
                            if pane
                                .selected_entry()
                                .is_some_and(|entry| entry.name.starts_with('.'))
                            {
                                pane.selected = None;
                            }
                        }
                    }
                    cx.notify();
                })),
        );
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(cx.theme().background)
            .child(toolbar)
            .child(
                div().flex_1().min_h_0().overflow_hidden().child(
                    h_resizable(("sftp-panes", cx.entity_id()))
                        .child(
                            resizable_panel()
                                .size_range(px(200.)..px(10000.))
                                .child(local),
                        )
                        .child(
                            resizable_panel()
                                .size_range(px(200.)..px(10000.))
                                .child(remote),
                        ),
                ),
            )
            .child(self.transfers.clone())
    }
}

fn validate_filename(name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." {
        return Err("请输入名称，不能使用 . 或 ..。".into());
    }
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err("名称不能包含斜杠、反斜杠或 NUL 字符。".into());
    }
    Ok(())
}

fn parse_mode(value: &str) -> Result<u32, String> {
    if value.is_empty()
        || value.len() > 4
        || !value.bytes().all(|byte| (b'0'..=b'7').contains(&byte))
    {
        return Err("请输入 1–4 位八进制数字，例如 0644。".into());
    }
    u32::from_str_radix(value, 8).map_err(|_| "八进制权限无效。".into())
}

fn normalize_remote_path(input: &str, current: &str, home: Option<&str>) -> Result<String, String> {
    if input.contains('\0') {
        return Err("路径不能包含 NUL 字符。".into());
    }
    let path = if input == "~" {
        home.ok_or("远程主目录尚未就绪。")?.to_string()
    } else if let Some(rest) = input.strip_prefix("~/") {
        remote_join(home.ok_or("远程主目录尚未就绪。")?, rest)
    } else if input.starts_with('~') {
        return Err("请使用 ~ 或 ~/ 访问当前远程用户的主目录。".into());
    } else if input.starts_with('/') {
        input.to_string()
    } else {
        if !current.starts_with('/') {
            return Err("远程目录尚未就绪。".into());
        }
        remote_join(current, input)
    };
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            component => components.push(component),
        }
    }
    Ok(format!("/{}", components.join("/")))
}

fn remote_join(parent: &str, name: &str) -> String {
    format!("{}/{}", parent.trim_end_matches('/'), name)
}

fn remote_parent(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit_once('/')
        .map(|(parent, _)| {
            if parent.is_empty() {
                "/".into()
            } else {
                parent.into()
            }
        })
        .unwrap_or_else(|| "/".into())
}

fn local_navigation_path(input: &str, current: &Path) -> PathBuf {
    if input == "~" {
        return dirs::home_dir().unwrap_or_else(|| current.to_path_buf());
    }
    if let Some(rest) = input.strip_prefix("~/") {
        return dirs::home_dir()
            .unwrap_or_else(|| current.to_path_buf())
            .join(rest);
    }
    let path = Path::new(input);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        current.join(path)
    }
}

fn sort_entries(entries: &mut [FileEntryInfo]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

fn read_local_directory(path: &Path) -> Result<(PathBuf, Vec<FileEntryInfo>), String> {
    let path =
        fs::canonicalize(path).map_err(|error| format!("无法打开 {}：{error}", path.display()))?;
    let directory =
        fs::read_dir(&path).map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
    let mut entries = Vec::new();
    for entry in directory {
        let entry = entry.map_err(|error| format!("无法读取目录项：{error}"))?;
        let entry_path = entry.path();
        let metadata = fs::symlink_metadata(&entry_path)
            .map_err(|error| format!("无法检查 {}：{error}", entry_path.display()))?;
        let is_symlink = metadata.is_symlink();
        let is_dir = metadata.is_dir()
            || (is_symlink && fs::metadata(&entry_path).is_ok_and(|metadata| metadata.is_dir()));
        #[cfg(unix)]
        let permissions = {
            use std::os::unix::fs::PermissionsExt;
            Some(metadata.permissions().mode())
        };
        #[cfg(not(unix))]
        let permissions = None;
        entries.push(FileEntryInfo {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry_path.display().to_string(),
            is_dir,
            is_symlink,
            size: metadata.len(),
            modified: metadata.modified().ok(),
            permissions,
        });
    }
    sort_entries(&mut entries);
    Ok((path, entries))
}

fn inspect_local_file(path: &Path) -> Result<u64, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("无法打开 {}：{error}", path.display()))?;
    if !metadata.is_file() {
        return Err("仅支持上传普通文件。".into());
    }
    Ok(metadata.len())
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "redox"
))]
fn rename_local_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use rustix::fs::{renameat_with, RenameFlags, CWD};
    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(Into::into)
}

#[cfg(windows)]
fn rename_local_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if from[..from.len() - 1].contains(&0) || to[..to.len() - 1].contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "文件路径包含空字符。",
        ));
    }
    let result = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0)
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "redox",
    windows
)))]
fn rename_local_no_replace(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "当前平台不支持禁止覆盖的原子重命名。",
    ))
}

fn perform_local_operation(operation: LocalOperation) -> Result<(), String> {
    match operation {
        LocalOperation::Mkdir(path) => {
            fs::create_dir(&path).map_err(|error| format!("无法创建 {}：{error}", path.display()))
        }
        LocalOperation::Rename { from, to } => {
            if from == to {
                return Ok(());
            }
            rename_local_no_replace(&from, &to).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    format!("{} 已存在，请使用其他名称。", to.display())
                } else {
                    format!("无法重命名 {}：{error}", from.display())
                }
            })
        }
        LocalOperation::Delete {
            path,
            is_dir,
            is_symlink,
        } => {
            let result = if is_dir && !is_symlink {
                fs::remove_dir(&path)
            } else {
                fs::remove_file(&path)
            };
            result.map_err(|error| format!("无法删除 {}：{error}", path.display()))
        }
        LocalOperation::Chmod { path, mode } => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(mode))
                    .map_err(|error| format!("无法修改 {} 的权限：{error}", path.display()))
            }
            #[cfg(not(unix))]
            {
                let _ = (path, mode);
                Err("当前平台不支持 POSIX 权限。".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_remote_path, parse_mode, perform_local_operation, read_local_directory,
        remote_join, remote_parent, validate_filename, FileBrowserView, LocalOperation,
    };
    use crate::{
        backend::{AppCommand, BackendEvent, TransferOutcome},
        config::AppSettings,
    };
    use gpui_kit::{AppContext, Context, Entity, TestAppContext, Window, WindowHandle};
    use tokio::sync::mpsc;

    struct Fixture {
        window: WindowHandle<gpui_kit::base::Root>,
        view: Entity<FileBrowserView>,
        _commands: mpsc::Receiver<AppCommand>,
    }

    impl Fixture {
        fn new(cx: &mut TestAppContext) -> Self {
            cx.update(gpui_kit::init);
            let (command_tx, commands) = mpsc::channel(256);
            let (window, view) = cx.update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| {
                        let mut view = FileBrowserView::new(
                            "session".into(),
                            command_tx,
                            &AppSettings::default(),
                            window,
                            cx,
                        );
                        view.tasks.clear();
                        view
                    })
                })
                .expect("test window")
            });
            Self {
                window: window.downcast().expect("Base Root"),
                view,
                _commands: commands,
            }
        }

        fn update<R>(
            &self,
            cx: &mut TestAppContext,
            update: impl FnOnce(&mut FileBrowserView, &mut Window, &mut Context<FileBrowserView>) -> R,
        ) -> R {
            cx.update_window(self.window.into(), |_, window, cx| {
                self.view.update(cx, |view, cx| update(view, window, cx))
            })
            .expect("update test file browser")
        }
    }

    #[gpui::test]
    fn completed_transfer_replaces_loading_requests_without_reversing_navigation(
        cx: &mut TestAppContext,
    ) {
        let fixture = Fixture::new(cx);
        let directory = tempfile::tempdir().unwrap();
        let requested_local = directory.path().display().to_string();
        fixture.update(cx, |view, window, cx| {
            view.local.path = "/previous-local".into();
            view.load_local(directory.path().to_path_buf(), window, cx);
            view.connected = true;
            view.remote_home = Some("/home/test".into());
            view.remote.path = "/previous-remote".into();
            view.load_remote("/requested-remote".into(), window, cx);
            let old_local_request = view.local.request_id;
            let old_remote_request = view.remote.request_id;

            view.handle_event(
                &BackendEvent::SftpTransferComplete {
                    session_id: "session".into(),
                    transfer_id: 1,
                    outcome: TransferOutcome::Completed,
                },
                window,
                cx,
            );
            assert_ne!(view.local.request_id, old_local_request);
            assert_ne!(view.remote.request_id, old_remote_request);
            assert_eq!(view.local.requested_path, requested_local);
            assert_eq!(view.remote.requested_path, "/requested-remote");
            assert_eq!(view.local.path, "/previous-local");
            assert_eq!(view.remote.path, "/previous-remote");
            assert!(view.local.loading);
            assert!(view.remote.loading);

            view.handle_event(
                &BackendEvent::SftpListResult {
                    session_id: "session".into(),
                    request_id: old_remote_request,
                    path: "/previous-remote".into(),
                    result: Err("obsolete listing".into()),
                },
                window,
                cx,
            );
            assert_eq!(view.remote.path, "/previous-remote");
            assert_eq!(view.remote.requested_path, "/requested-remote");
            assert!(view.remote.loading);
            assert!(view.remote.error.is_none());

            view.handle_event(
                &BackendEvent::SftpListResult {
                    session_id: "session".into(),
                    request_id: view.remote.request_id,
                    path: "/requested-remote".into(),
                    result: Ok(Vec::new()),
                },
                window,
                cx,
            );
            assert_eq!(view.remote.path, "/requested-remote");
            assert!(!view.remote.loading);
        });
        cx.run_until_parked();
        fixture.update(cx, |view, _, _| {
            assert_eq!(
                view.local.path,
                std::fs::canonicalize(directory.path())
                    .unwrap()
                    .display()
                    .to_string()
            );
            assert!(!view.local.loading);
        });
    }

    #[gpui::test]
    fn completed_remote_operation_refreshes_only_the_pending_destination(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx);
        fixture.update(cx, |view, window, cx| {
            view.connected = true;
            view.remote_home = Some("/home/test".into());
            view.remote.path = "/previous-remote".into();
            view.load_remote("/requested-remote".into(), window, cx);
            let old_request = view.remote.request_id;
            let operation_id = view.operation_id();
            view.handle_event(
                &BackendEvent::SftpOperationResult {
                    session_id: "session".into(),
                    request_id: operation_id,
                    path: "/requested-remote".into(),
                    result: Ok(()),
                },
                window,
                cx,
            );
            let refreshed_request = view.remote.request_id;
            assert_ne!(refreshed_request, old_request);
            assert_eq!(view.remote.requested_path, "/requested-remote");
            assert_eq!(view.remote.path, "/previous-remote");
            assert!(!view.pending_operations.contains(&operation_id));

            let operation_id = view.operation_id();
            view.handle_event(
                &BackendEvent::SftpOperationResult {
                    session_id: "session".into(),
                    request_id: operation_id,
                    path: "/previous-remote".into(),
                    result: Ok(()),
                },
                window,
                cx,
            );
            assert_eq!(view.remote.request_id, refreshed_request);
            assert_eq!(view.remote.requested_path, "/requested-remote");

            view.handle_event(
                &BackendEvent::SftpListResult {
                    session_id: "session".into(),
                    request_id: old_request,
                    path: "/requested-remote".into(),
                    result: Err("obsolete listing".into()),
                },
                window,
                cx,
            );
            assert_eq!(view.remote.request_id, refreshed_request);
            assert!(view.remote.loading);
            assert!(view.remote.error.is_none());
        });
    }

    #[gpui::test]
    fn completed_local_operation_refreshes_the_pending_destination(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx);
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().display().to_string();
        let old_request = fixture.update(cx, |view, window, cx| {
            view.local.path = "/previous-local".into();
            view.local.requested_path = parent.clone();
            view.local.loading = true;
            view.local.request_id = view.next_id();
            let old_request = view.local.request_id;
            view.local_operation(
                parent.clone(),
                LocalOperation::Mkdir(directory.path().join("created")),
                window,
                cx,
            );
            old_request
        });
        cx.run_until_parked();
        fixture.update(cx, |view, _, _| {
            assert_ne!(view.local.request_id, old_request);
            assert_eq!(view.local.requested_path, parent);
            assert!(!view.local.loading);
            assert!(view.local.error.is_none());
            assert_eq!(
                view.local.path,
                std::fs::canonicalize(directory.path())
                    .unwrap()
                    .display()
                    .to_string()
            );
            assert!(view
                .local
                .entries
                .iter()
                .any(|entry| entry.name == "created"));
        });
    }

    #[gpui::test]
    fn completed_local_operation_does_not_reverse_pending_navigation(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx);
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().display().to_string();
        let old_request = fixture.update(cx, |view, window, cx| {
            view.local.path = parent.clone();
            view.local.requested_path = "/requested-local".into();
            view.local.loading = true;
            view.local.request_id = view.next_id();
            let old_request = view.local.request_id;
            view.local_operation(
                parent,
                LocalOperation::Mkdir(directory.path().join("created")),
                window,
                cx,
            );
            old_request
        });
        cx.run_until_parked();
        fixture.update(cx, |view, _, _| {
            assert_eq!(view.local.request_id, old_request);
            assert_eq!(view.local.requested_path, "/requested-local");
            assert!(view.local.loading);
            assert!(view.local.error.is_none());
            assert!(directory.path().join("created").is_dir());
        });
    }

    #[test]
    fn remote_paths_are_posix_absolute_and_home_is_remote() {
        assert_eq!(
            normalize_remote_path("../next//./dir", "/srv/work", Some("/home/remote")).unwrap(),
            "/srv/next/dir"
        );
        assert_eq!(
            normalize_remote_path("~/files", "/srv/work", Some("/home/remote")).unwrap(),
            "/home/remote/files"
        );
        assert_eq!(
            normalize_remote_path("../../..", "/srv", None).unwrap(),
            "/"
        );
        assert_eq!(
            normalize_remote_path("/var/a\\b", "/", None).unwrap(),
            "/var/a\\b"
        );
        assert!(normalize_remote_path("~", "/", None).is_err());
        assert!(normalize_remote_path("~someone", "/", Some("/home/remote")).is_err());
        assert!(normalize_remote_path("bad\0path", "/", None).is_err());
        assert_eq!(remote_join("/", "file"), "/file");
        assert_eq!(remote_parent("/file"), "/");
        assert_eq!(remote_parent("/srv/work/"), "/srv");
    }

    #[test]
    fn names_cannot_escape_a_directory() {
        for invalid in ["", ".", "..", "../file", "/absolute", "a\\b", "a\0b"] {
            assert!(validate_filename(invalid).is_err(), "{invalid:?}");
        }
        assert!(validate_filename("my file.txt").is_ok());
    }

    #[test]
    fn permissions_are_bounded_octal() {
        assert_eq!(parse_mode("0755").unwrap(), 0o755);
        assert_eq!(parse_mode("0").unwrap(), 0);
        assert_eq!(parse_mode("7777").unwrap(), 0o7777);
        for invalid in ["", "888", "-1", "10000", "0o755", " 644"] {
            assert!(parse_mode(invalid).is_err());
        }
    }

    #[test]
    fn local_operations_create_rename_and_delete_real_files() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("folder");
        perform_local_operation(LocalOperation::Mkdir(folder.clone())).unwrap();
        assert!(folder.is_dir());
        let source = folder.join("old");
        let target = folder.join("new");
        std::fs::write(&source, "kept").unwrap();
        perform_local_operation(LocalOperation::Rename {
            from: source.clone(),
            to: target.clone(),
        })
        .unwrap();
        assert!(!source.exists());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "kept");
        assert!(perform_local_operation(LocalOperation::Delete {
            path: folder.clone(),
            is_dir: true,
            is_symlink: false
        })
        .is_err());
        perform_local_operation(LocalOperation::Delete {
            path: target,
            is_dir: false,
            is_symlink: false,
        })
        .unwrap();
        perform_local_operation(LocalOperation::Delete {
            path: folder.clone(),
            is_dir: true,
            is_symlink: false,
        })
        .unwrap();
        assert!(!folder.exists());
    }

    #[test]
    fn local_rename_never_replaces_an_existing_target() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        std::fs::write(&source, "source").unwrap();
        std::fs::write(&target, "target").unwrap();
        assert!(perform_local_operation(LocalOperation::Rename {
            from: source.clone(),
            to: target.clone()
        })
        .is_err());
        assert_eq!(std::fs::read_to_string(source).unwrap(), "source");
        assert_eq!(std::fs::read_to_string(target).unwrap(), "target");
    }

    #[test]
    fn concurrent_local_renames_publish_only_one_source() {
        for _ in 0..32 {
            let directory = tempfile::tempdir().unwrap();
            let sources = [
                directory.path().join("first"),
                directory.path().join("second"),
            ];
            let target = directory.path().join("target");
            for (index, source) in sources.iter().enumerate() {
                std::fs::write(source, [index as u8]).unwrap();
            }
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
            let workers = sources.each_ref().map(|source| {
                let from = source.clone();
                let to = target.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    perform_local_operation(LocalOperation::Rename { from, to })
                })
            });
            barrier.wait();
            let results = workers.map(|worker| worker.join().unwrap());
            assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
            let winner = results.iter().position(Result::is_ok).unwrap();
            assert_eq!(std::fs::read(&target).unwrap(), [winner as u8]);
            assert!(!sources[winner].exists());
            assert_eq!(
                std::fs::read(&sources[1 - winner]).unwrap(),
                [(1 - winner) as u8]
            );
        }
    }

    #[test]
    fn local_directory_rename_never_replaces_an_existing_empty_directory() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(source.join("kept"), b"source").unwrap();
        assert!(perform_local_operation(LocalOperation::Rename {
            from: source.clone(),
            to: target.clone(),
        })
        .is_err());
        assert!(source.join("kept").is_file());
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn local_rename_preserves_an_existing_dangling_symlink() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let target = directory.path().join("target");
        let missing = directory.path().join("missing");
        std::fs::write(&source, b"source").unwrap();
        std::os::unix::fs::symlink(&missing, &target).unwrap();
        assert!(perform_local_operation(LocalOperation::Rename {
            from: source.clone(),
            to: target.clone(),
        })
        .is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"source");
        assert_eq!(std::fs::read_link(&target).unwrap(), missing);
    }

    #[cfg(unix)]
    #[test]
    fn deleting_a_symlink_preserves_its_target() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        let link = directory.path().join("link");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("kept"), "kept").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        perform_local_operation(LocalOperation::Delete {
            path: link.clone(),
            is_dir: true,
            is_symlink: true,
        })
        .unwrap();
        assert!(!link.exists());
        assert!(target.join("kept").exists());
    }

    #[test]
    fn local_listing_reads_real_metadata_and_sorts_directories_first() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("a-file"), "contents").unwrap();
        std::fs::create_dir(directory.path().join("z-folder")).unwrap();
        let (_, entries) = read_local_directory(directory.path()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "z-folder");
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].size, 8);
    }
}
