use std::{collections::HashSet, path::PathBuf};

use gpui_kit::component::{
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState},
    popover::Popover,
    scroll::ScrollableElement,
    select::{SearchableVec, Select, SelectEvent, SelectState},
    ActiveTheme, Disableable, Icon, IconName, IndexPath, Sizable,
};
use gpui_kit::{prelude::*, *};

use crate::{
    config::{AuthConfig, HostLibrary, ImportOrigin, SessionConfig},
    ssh::{SshConfigEntry, SshConfigSnapshot, SshConfigSource},
    ui::connection::{
        field, folder_options, row, ConnectionAction, ConnectionForm, FolderOption,
        SshImportOptions,
    },
};

#[derive(Clone)]
pub enum SshImportAction {
    Submit {
        generation: u64,
        profiles: Vec<SessionConfig>,
    },
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryStatus {
    Ready,
    Incomplete,
    Duplicate,
    Blocked,
}

impl EntryStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Ready => "可导入",
            Self::Incomplete => "需补全",
            Self::Duplicate => "已导入",
            Self::Blocked => "暂不可导入",
        }
    }
}

struct PreviewEntry {
    original: SshConfigEntry,
    profile: SessionConfig,
    auth_chosen: bool,
    duplicate: bool,
    selected: bool,
}

impl PreviewEntry {
    fn status(&self) -> EntryStatus {
        if !self.original.blocked.is_empty() {
            EntryStatus::Blocked
        } else if self.duplicate {
            EntryStatus::Duplicate
        } else if !self.auth_chosen || !profile_ready(&self.profile) {
            EntryStatus::Incomplete
        } else {
            EntryStatus::Ready
        }
    }
}

struct Preview {
    generation: u64,
    source_path: PathBuf,
    entries: Vec<PreviewEntry>,
    diagnostics: Vec<String>,
}

pub struct SshImportView {
    path: Entity<InputState>,
    observed_path: String,
    home: PathBuf,
    library: HostLibrary,
    folder: Entity<SelectState<SearchableVec<FolderOption>>>,
    generation: u64,
    preview: Option<Preview>,
    loading: bool,
    picking: bool,
    closed: bool,
    draft: Option<Entity<ConnectionForm>>,
    draft_index: Option<usize>,
    draft_subscription: Option<Subscription>,
    read_task: Option<Task<()>>,
    picker_task: Option<Task<()>>,
    pub busy: bool,
    pub error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl SshImportView {
    pub fn new(
        default_source: Option<SshConfigSource>,
        library: HostLibrary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (default_path, home) = match default_source {
            Some(source) => (source.path.display().to_string(), source.home),
            None => ("~/.ssh/config".into(), dirs::home_dir().unwrap_or_default()),
        };
        let path = field(&default_path, "~/.ssh/config", false, window, cx);
        let path_subscription = cx.subscribe(&path, |this, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value().to_string();
                if value != this.observed_path {
                    this.observed_path = value;
                    this.invalidate(cx);
                }
            }
        });
        let folder = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(folder_options(&library)),
                Some(IndexPath::new(0)),
                window,
                cx,
            )
            .searchable(true)
        });
        let folder_subscription = cx.subscribe(&folder, |_, _, _: &SelectEvent<_>, cx| cx.notify());
        Self {
            path,
            observed_path: default_path,
            home,
            library,
            folder,
            generation: 0,
            preview: None,
            loading: false,
            picking: false,
            closed: false,
            draft: None,
            draft_index: None,
            draft_subscription: None,
            read_task: None,
            picker_task: None,
            busy: false,
            error: None,
            _subscriptions: vec![path_subscription, folder_subscription],
        }
    }

    #[cfg(test)]
    pub(crate) fn load_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load(window, cx);
    }

    #[cfg(test)]
    pub(crate) fn submit_for_test(&mut self, cx: &mut Context<Self>) {
        self.submit(cx);
    }

    #[cfg(test)]
    pub(crate) fn set_path_for_test(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.path
            .update(cx, |input, cx| input.set_value(path, window, cx));
        self.observed_path = path.into();
        self.invalidate(cx);
    }

    #[cfg(test)]
    pub(crate) fn preview_count_for_test(&self) -> usize {
        self.preview
            .as_ref()
            .map_or(0, |preview| preview.entries.len())
    }

    #[cfg(test)]
    pub(crate) fn selected_count_for_test(&self) -> usize {
        self.preview.as_ref().map_or(0, |preview| {
            preview
                .entries
                .iter()
                .filter(|entry| entry.selected)
                .count()
        })
    }

    #[cfg(test)]
    pub(crate) fn select_for_test(&mut self, index: usize, selected: bool, cx: &mut Context<Self>) {
        self.set_selected(index, selected, cx);
    }

    fn invalidate(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        self.read_task = None;
        self.preview = None;
        self.loading = false;
        self.draft = None;
        self.draft_index = None;
        self.draft_subscription = None;
        self.error = None;
        cx.notify();
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.loading || self.picking || self.closed {
            return;
        }
        self.picking = true;
        let generation = self.generation;
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("选择 SSH Config".into()),
        });
        self.picker_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.picking = false;
                if this.closed || this.busy || generation != this.generation {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.first() {
                            let path = path.display().to_string();
                            this.observed_path = path.clone();
                            this.path
                                .update(cx, |input, cx| input.set_value(path, window, cx));
                            this.invalidate(cx);
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => this.error = Some(format!("无法选择文件：{error}")),
                    Err(_) => this.error = Some("文件选择窗口已关闭".into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.loading || self.picking || self.closed {
            return;
        }
        self.observed_path = self.path.read(cx).value().to_string();
        let path = self.observed_path.trim().to_owned();
        self.invalidate(cx);
        if path.is_empty() {
            self.error = Some("请填写 SSH Config 路径".into());
            cx.notify();
            return;
        }
        let home = self.home.clone();
        let generation = self.generation;
        self.loading = true;
        let work = cx.background_spawn(async move { read_source(&path, home) });
        self.read_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, _, cx| this.finish_load(generation, result, cx));
        }));
        cx.notify();
    }

    fn finish_load(
        &mut self,
        generation: u64,
        result: Result<(PathBuf, SshConfigSnapshot), String>,
        cx: &mut Context<Self>,
    ) {
        if self.closed || generation != self.generation || !self.loading {
            return;
        }
        self.loading = false;
        match result {
            Ok((source_path, snapshot)) => {
                let imported: HashSet<_> = self
                    .library
                    .sessions
                    .iter()
                    .filter_map(|profile| profile.import_origin.as_ref())
                    .collect();
                let entries = snapshot
                    .entries
                    .into_iter()
                    .map(|original| {
                        let duplicate = imported.contains(&ImportOrigin {
                            source_path: source_path.clone(),
                            alias: original.id.alias.clone(),
                        });
                        let profile = original.profile.clone();
                        let auth_chosen = original.identity_files.len() == 1;
                        PreviewEntry {
                            original,
                            profile,
                            auth_chosen,
                            duplicate,
                            selected: false,
                        }
                    })
                    .collect();
                self.preview = Some(Preview {
                    generation,
                    source_path,
                    entries,
                    diagnostics: snapshot.diagnostics,
                });
                self.error = None;
            }
            Err(error) => {
                self.preview = None;
                self.error = Some(error);
            }
        }
        cx.notify();
    }

    fn set_selected(&mut self, index: usize, selected: bool, cx: &mut Context<Self>) {
        if self.busy || self.loading || self.closed {
            return;
        }
        if let Some(entry) = self
            .preview
            .as_mut()
            .and_then(|preview| preview.entries.get_mut(index))
        {
            if matches!(entry.status(), EntryStatus::Ready | EntryStatus::Incomplete) {
                entry.selected = selected;
            }
        }
        cx.notify();
    }

    fn edit(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.loading || self.closed {
            return;
        }
        let Some(preview) = &self.preview else {
            return;
        };
        let Some(entry) = preview.entries.get(index) else {
            return;
        };
        if matches!(
            entry.status(),
            EntryStatus::Blocked | EntryStatus::Duplicate
        ) {
            return;
        }
        let generation = preview.generation;
        let options = SshImportOptions {
            identity_files: entry.original.identity_files.clone(),
            notices: entry.original.warnings.clone(),
            require_auth_choice: !entry.auth_chosen,
        };
        let profile = entry.profile.clone();
        let form = cx.new(|cx| {
            ConnectionForm::new_import_draft(profile, &self.library, options, window, cx)
        });
        self.draft_subscription = Some(cx.subscribe(&form, move |this, _, action, cx| {
            if this.closed
                || this.busy
                || this.generation != generation
                || this.draft_index != Some(index)
            {
                return;
            }
            match action {
                ConnectionAction::ApplyDraft(profile) => {
                    let Some(entry) = this
                        .preview
                        .as_mut()
                        .and_then(|preview| preview.entries.get_mut(index))
                    else {
                        return;
                    };
                    if !entry.original.blocked.is_empty()
                        || entry.duplicate
                        || !profile_ready(profile)
                    {
                        return;
                    }
                    let mut profile = profile.as_ref().clone();
                    clear_secrets(&mut profile);
                    entry.profile = profile;
                    entry.auth_chosen = true;
                    this.draft = None;
                    this.draft_index = None;
                    this.draft_subscription = None;
                }
                ConnectionAction::Cancel => {
                    this.draft = None;
                    this.draft_index = None;
                    this.draft_subscription = None;
                }
                ConnectionAction::Submit { .. } => return,
            }
            cx.notify();
        }));
        self.draft = Some(form);
        self.draft_index = Some(index);
        self.error = None;
        cx.notify();
    }

    fn selection_is_ready(&self, cx: &App) -> bool {
        let Some(preview) = &self.preview else {
            return false;
        };
        if self.closed
            || self.loading
            || self.picking
            || self.draft.is_some()
            || preview.generation != self.generation
            || self.path.read(cx).value().as_str() != self.observed_path
        {
            return false;
        }
        let Some(folder_id) = self.folder.read(cx).selected_value() else {
            return false;
        };
        if folder_id
            .as_ref()
            .is_some_and(|id| !self.library.folders.iter().any(|folder| &folder.id == id))
        {
            return false;
        }
        let selected: Vec<_> = preview
            .entries
            .iter()
            .filter(|entry| entry.selected)
            .collect();
        !selected.is_empty()
            && selected
                .iter()
                .all(|entry| entry.status() == EntryStatus::Ready)
    }

    pub fn submission_is_current(&self, generation: u64) -> bool {
        !self.closed
            && self.busy
            && !self.loading
            && self.generation == generation
            && self
                .preview
                .as_ref()
                .is_some_and(|preview| preview.generation == generation)
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy || !self.selection_is_ready(cx) {
            return;
        }
        let preview = self.preview.as_ref().unwrap();
        let folder_id = self.folder.read(cx).selected_value().cloned().unwrap();
        let profiles = preview
            .entries
            .iter()
            .filter(|entry| entry.selected)
            .map(|entry| {
                let mut profile = entry.profile.clone();
                profile.id = uuid::Uuid::new_v4().to_string();
                profile.folder_id = folder_id.clone();
                profile.import_origin = Some(ImportOrigin {
                    source_path: preview.source_path.clone(),
                    alias: entry.original.id.alias.clone(),
                });
                clear_secrets(&mut profile);
                profile
            })
            .collect();
        self.busy = true;
        self.error = None;
        cx.emit(SshImportAction::Submit {
            generation: self.generation,
            profiles,
        });
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.closed = true;
        self.picker_task = None;
        self.invalidate(cx);
        cx.emit(SshImportAction::Cancel);
    }

    fn entry_row(
        &self,
        index: usize,
        entry: &PreviewEntry,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let status = entry.status();
        let selectable = matches!(status, EntryStatus::Ready | EntryStatus::Incomplete);
        let details = entry
            .original
            .warnings
            .iter()
            .chain(&entry.original.blocked)
            .cloned()
            .collect::<Vec<_>>();
        let alias = entry.original.id.alias.clone();
        div()
            .flex()
            .items_center()
            .gap_2()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .child(
                Checkbox::new(("ssh-import-select", index))
                    .accessibility_label(format!("选择 {alias}"))
                    .checked(entry.selected)
                    .disabled(self.busy || !selectable)
                    .on_change(cx.listener(move |this, checked, _, cx| {
                        this.set_selected(index, *checked, cx)
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(div().text_sm().truncate().child(alias.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(format!(
                                "{}@{}:{}",
                                entry.profile.username, entry.profile.host, entry.profile.port
                            )),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(if status == EntryStatus::Blocked {
                        cx.theme().danger
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(status.label()),
            )
            .when(!details.is_empty(), |view| {
                view.child(
                    Popover::new(("ssh-import-details", index))
                        .trigger(
                            Button::new("ssh-import-details-trigger")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::Info).size_3p5())
                                .tooltip("查看配置说明")
                                .accessibility_label(format!("{alias}，查看配置说明")),
                        )
                        .content(move |_, _, cx| {
                            div()
                                .id("ssh-import-details-content")
                                .flex()
                                .flex_col()
                                .gap_2()
                                .w(px(310.))
                                .max_h(px(240.))
                                .overflow_y_scroll()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .children(
                                    details.iter().map(|detail| {
                                        div().whitespace_normal().child(detail.clone())
                                    }),
                                )
                        }),
                )
            })
            .child(
                Button::new(("ssh-import-edit", index))
                    .ghost()
                    .small()
                    .label(if status == EntryStatus::Incomplete {
                        "补全"
                    } else {
                        "编辑"
                    })
                    .disabled(self.busy || !selectable)
                    .on_click(cx.listener(move |this, _, window, cx| this.edit(index, window, cx))),
            )
    }
}

impl EventEmitter<SshImportAction> for SshImportView {}

impl Render for SshImportView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(draft) = self.draft.clone() {
            return div().w_full().child(draft).into_any_element();
        }
        let selected = self.preview.as_ref().map_or(0, |preview| {
            preview
                .entries
                .iter()
                .filter(|entry| entry.selected)
                .count()
        });
        let ready = self.selection_is_ready(cx);
        let locked = self.busy || self.loading || self.picking;
        let body = div()
            .id("ssh-import-scroll")
            .debug_selector(|| "ssh-import-scroll-content".into())
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .min_h_0()
            .h((window.viewport_size().height * 0.70 - px(110.)).max(px(120.)))
            .pr_1()
            .child(
                div()
                    .debug_selector(|| "ssh-import-source-field".into())
                    .child(row(
                        "SSH Config 路径",
                        Input::new(&self.path)
                            .id("ssh-import-path")
                            .disabled(locked),
                    )),
            )
            .child(
                div()
                    .debug_selector(|| "ssh-import-source-actions".into())
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("ssh-import-browse")
                            .label("浏览…")
                            .disabled(locked)
                            .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                    )
                    .child(
                        Button::new("ssh-import-load")
                            .label(if self.loading {
                                "读取中…"
                            } else {
                                "读取配置"
                            })
                            .disabled(locked)
                            .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                    "仅在点击读取后访问配置；原文件保持只读。导入不会连接，也不会读取私钥或凭据。",
                ),
            )
            .when_some(self.preview.as_ref(), |view, preview| {
                view.child(row(
                    "导入到文件夹",
                    Select::new(&self.folder)
                        .id("ssh-import-folder")
                        .disabled(self.busy),
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .truncate()
                        .child(format!("来源 · {}", preview.source_path.display())),
                )
                .when(!preview.diagnostics.is_empty(), |view| {
                    view.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("该来源包含未支持的条件或包含指令；请查看各主机说明。"),
                    )
                })
                .when(preview.entries.is_empty(), |view| {
                    view.child(
                        div()
                            .py_4()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("配置已读取，没有可导入的主机别名。"),
                    )
                })
                .children(
                    preview
                        .entries
                        .iter()
                        .enumerate()
                        .map(|(index, entry)| self.entry_row(index, entry, cx)),
                )
            })
            .when(
                self.preview.is_none() && !self.loading && self.error.is_none(),
                |view| {
                    view.child(
                        div()
                            .py_4()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("选择来源并读取配置后，在这里预览和选择主机。"),
                    )
                },
            )
            .when_some(self.error.clone(), |view, error| {
                view.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .overflow_y_scrollbar();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .child(
                div()
                    .debug_selector(|| "ssh-import-viewport".into())
                    .w_full()
                    .child(body),
            )
            .child(
                div()
                    .debug_selector(|| "ssh-import-footer".into())
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .pt_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("已选择 {selected} 台；需补全的主机请先编辑。")),
                    )
                    .child(
                        Button::new("ssh-import-cancel")
                            .ghost()
                            .label("取消")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                    )
                    .child(
                        Button::new("ssh-import-confirm")
                            .primary()
                            .label(if self.busy {
                                "导入中…"
                            } else {
                                "确认导入"
                            })
                            .disabled(self.busy || !ready)
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    ),
            )
            .into_any_element()
    }
}

fn read_source(path: &str, home: PathBuf) -> Result<(PathBuf, SshConfigSnapshot), String> {
    let path = if let Some(relative) = path.strip_prefix("~/") {
        if home.as_os_str().is_empty() {
            return Err("无法确定主目录，请输入完整路径".into());
        }
        home.join(relative)
    } else {
        PathBuf::from(path)
    };
    let source_path = path
        .canonicalize()
        .map_err(|error| format!("无法读取 SSH Config：{error}"))?;
    let snapshot = SshConfigSource {
        path: source_path.clone(),
        home,
    }
    .load()?;
    Ok((source_path, snapshot))
}

fn profile_ready(profile: &SessionConfig) -> bool {
    !profile.host.trim().is_empty()
        && !profile.host.chars().any(char::is_whitespace)
        && !profile.username.trim().is_empty()
        && profile.port > 0
        && match &profile.auth {
            AuthConfig::Key { key_path, .. } => !key_path.trim().is_empty(),
            AuthConfig::Password { .. } | AuthConfig::Agent => true,
        }
}

fn clear_secrets(profile: &mut SessionConfig) {
    profile.remember_credentials = false;
    match &mut profile.auth {
        AuthConfig::Key { passphrase, .. } => *passphrase = None,
        AuthConfig::Password { password } => *password = None,
        AuthConfig::Agent => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{read_source, EntryStatus, SshImportAction, SshImportView};
    use crate::{
        config::{AuthConfig, Folder, HostLibrary, ImportOrigin, SessionConfig},
        ssh::SshConfigSource,
        ui::connection::ConnectionAction,
    };
    use gpui_kit::component::input::InputEvent;
    use gpui_kit::{
        AppContext, Context, Entity, Subscription, TestAppContext, Window, WindowHandle,
    };
    use std::{cell::RefCell, fs, rc::Rc};
    use tempfile::TempDir;

    const READY: &str = "Host first\n HostName first.test.invalid\n User fixture\n IdentityFile ~/.ssh/never-read-key\nHost second\n HostName second.test.invalid\n User fixture\n IdentityFile ~/.ssh/another-never-read-key\n";

    struct Fixture {
        window: WindowHandle<gpui_kit::base::Root>,
        view: Entity<SshImportView>,
        actions: Rc<RefCell<Vec<SshImportAction>>>,
        source: SshConfigSource,
        _directory: TempDir,
        _subscription: Subscription,
    }

    impl Fixture {
        fn new(cx: &mut TestAppContext, text: &str, library: HostLibrary) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let home = directory.path().join("synthetic-home");
            fs::create_dir_all(home.join(".ssh")).unwrap();
            let source = SshConfigSource {
                path: home.join(".ssh/config"),
                home,
            };
            fs::write(&source.path, text).unwrap();
            cx.update(gpui_kit::init);
            let (window, view) = cx.update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| SshImportView::new(Some(source.clone()), library, window, cx))
                })
                .expect("import window")
            });
            let actions = Rc::new(RefCell::new(Vec::new()));
            let collected = actions.clone();
            let subscription = cx.update(|cx| {
                cx.subscribe(&view, move |_, action: &SshImportAction, _| {
                    collected.borrow_mut().push(action.clone())
                })
            });
            Self {
                window: window.downcast().unwrap(),
                view,
                actions,
                source,
                _directory: directory,
                _subscription: subscription,
            }
        }

        fn update<R>(
            &self,
            cx: &mut TestAppContext,
            update: impl FnOnce(&mut SshImportView, &mut Window, &mut Context<SshImportView>) -> R,
        ) -> R {
            cx.update_window(self.window.into(), |_, window, cx| {
                self.view.update(cx, |view, cx| update(view, window, cx))
            })
            .unwrap()
        }

        fn load(&self, cx: &mut TestAppContext) {
            self.update(cx, |view, window, cx| view.load(window, cx));
            cx.run_until_parked();
        }
    }

    fn library() -> HostLibrary {
        HostLibrary {
            folders: vec![Folder {
                id: "destination".into(),
                name: "Imported".into(),
                parent_id: None,
            }],
            sessions: vec![],
        }
    }

    #[gpui::test]
    fn constructor_and_file_picker_do_not_read_or_import(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx, "Host broken\n User \"unterminated", library());
        cx.run_until_parked();
        fixture.update(cx, |view, window, cx| {
            assert!(view.preview.is_none());
            assert!(view.error.is_none());
            assert!(!view.loading);
            view.browse(window, cx);
        });
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|options| {
            assert!(options.files && !options.directories && !options.multiple);
            None
        });
        cx.run_until_parked();
        fixture.update(cx, |view, window, cx| {
            assert!(view.preview.is_none() && view.error.is_none());
            assert!(!view.picking);
            view.browse(window, cx);
        });
        cx.simulate_path_prompt_response(|_| Some(vec![fixture.source.path.clone()]));
        cx.run_until_parked();
        fixture.update(cx, |view, _, _| {
            assert!(view.preview.is_none() && view.error.is_none())
        });
        assert!(fixture.actions.borrow().is_empty());
        fixture.load(cx);
        fixture.update(cx, |view, _, _| {
            assert!(view.error.is_some() && view.preview.is_none())
        });
    }

    #[gpui::test]
    fn explicit_load_imports_only_selected_frozen_metadata_into_folder(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx, READY, library());
        let original = fs::read(&fixture.source.path).unwrap();
        fixture.load(cx);
        fixture.update(cx, |view, window, cx| {
            assert_eq!(view.preview_count_for_test(), 2);
            assert_eq!(view.selected_count_for_test(), 0);
            assert_eq!(
                view.preview.as_ref().unwrap().entries[0].status(),
                EntryStatus::Ready
            );
            view.folder.update(cx, |state, cx| {
                state.set_selected_value(&Some("destination".into()), window, cx)
            });
            view.set_selected(1, true, cx);
            assert!(view.selection_is_ready(cx));
        });
        fs::write(&fixture.source.path, "Host changed\n User someone-else\n").unwrap();
        fixture.update(cx, |view, _, cx| {
            view.submit(cx);
            assert!(view.busy);
            assert!(view.submission_is_current(view.generation));
            view.submit(cx);
        });
        cx.run_until_parked();
        let actions = fixture.actions.borrow();
        let [SshImportAction::Submit {
            generation,
            profiles,
        }] = actions.as_slice()
        else {
            panic!("one import submission");
        };
        assert_eq!(profiles.len(), 1);
        let profile = &profiles[0];
        assert!(uuid::Uuid::parse_str(&profile.id).is_ok());
        assert_eq!(profile.name, "second");
        assert_eq!(profile.host, "second.test.invalid");
        assert_eq!(profile.folder_id.as_deref(), Some("destination"));
        assert!(!profile.remember_credentials);
        assert!(
            matches!(&profile.auth, AuthConfig::Key { key_path, passphrase: None } if key_path == &fixture.source.home.join(".ssh/another-never-read-key").display().to_string())
        );
        assert_eq!(
            profile.import_origin.as_ref().unwrap().source_path,
            fixture.source.path.canonicalize().unwrap()
        );
        assert_eq!(profile.import_origin.as_ref().unwrap().alias, "second");
        fixture.update(cx, |view, _, _| {
            assert!(view.submission_is_current(*generation))
        });
        assert_ne!(original, fs::read(&fixture.source.path).unwrap());
        assert_eq!(
            fs::read_to_string(&fixture.source.path).unwrap(),
            "Host changed\n User someone-else\n"
        );
    }

    #[gpui::test]
    fn unsupported_routes_and_repeat_imports_cannot_be_selected_or_edited(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx, &format!("{READY}Host blocked\n User fixture\n ProxyJump gateway.test.invalid\n IdentityFile ~/.ssh/key\n"), library());
        let origin = ImportOrigin {
            source_path: fixture.source.path.canonicalize().unwrap(),
            alias: "first".into(),
        };
        fixture.update(cx, |view, _, _| {
            view.library.sessions.push(SessionConfig {
                id: "existing".into(),
                name: "Renamed and moved".into(),
                import_origin: Some(origin),
                ..Default::default()
            })
        });
        fixture.load(cx);
        fixture.update(cx, |view, window, cx| {
            let preview = view.preview.as_ref().unwrap();
            assert_eq!(preview.entries[0].status(), EntryStatus::Duplicate);
            assert_eq!(preview.entries[2].status(), EntryStatus::Blocked);
            for index in [0, 2] {
                view.set_selected(index, true, cx);
                view.edit(index, window, cx);
                assert!(view.draft.is_none());
            }
            assert_eq!(view.selected_count_for_test(), 0);
            view.submit(cx);
            assert!(!view.busy);
        });
        assert!(fixture.actions.borrow().is_empty());
        for directive in ["Include other.conf", "Match exec never-run-this"] {
            fs::write(&fixture.source.path, format!("{READY}{directive}\n")).unwrap();
            fixture.load(cx);
            fixture.update(cx, |view, _, _| {
                assert!(view
                    .preview
                    .as_ref()
                    .unwrap()
                    .entries
                    .iter()
                    .all(|entry| entry.status() == EntryStatus::Blocked))
            });
        }
    }

    #[gpui::test]
    fn incomplete_authentication_requires_draft_completion_and_keeps_secrets_out(
        cx: &mut TestAppContext,
    ) {
        let fixture = Fixture::new(cx, "Host missing\n HostName missing.test.invalid\nHost many\n User fixture\n IdentityFile ~/.ssh/first\n IdentityFile ~/.ssh/second\n", library());
        fixture.load(cx);
        fixture.update(cx, |view, window, cx| {
            assert!(view
                .preview
                .as_ref()
                .unwrap()
                .entries
                .iter()
                .all(|entry| entry.status() == EntryStatus::Incomplete));
            view.set_selected(0, true, cx);
            view.set_selected(1, true, cx);
            view.submit(cx);
            assert!(!view.busy);
            view.edit(0, window, cx);
            let form = view.draft.as_ref().unwrap().clone();
            form.update(cx, |form, cx| form.submit_for_test(true, true, cx));
            assert!(form.read(cx).error.is_some());
            let mut completed = view.preview.as_ref().unwrap().entries[0].profile.clone();
            completed.username = "explicit-user".into();
            completed.auth = AuthConfig::Password {
                password: Some("discard-me".into()),
            };
            completed.remember_credentials = true;
            form.update(cx, |_, cx| {
                cx.emit(ConnectionAction::ApplyDraft(Box::new(completed)))
            });
        });
        cx.run_until_parked();
        fixture.update(cx, |view, window, cx| {
            assert!(view.draft.is_none());
            assert_eq!(
                view.preview.as_ref().unwrap().entries[0].status(),
                EntryStatus::Ready
            );
            view.edit(1, window, cx);
            let form = view.draft.as_ref().unwrap().clone();
            form.update(cx, |form, cx| form.submit_for_test(false, false, cx));
            assert!(form.read(cx).error.is_some());
            let mut completed = view.preview.as_ref().unwrap().entries[1].profile.clone();
            completed.auth = AuthConfig::Key {
                key_path: "/explicit/second".into(),
                passphrase: Some("discard-passphrase".into()),
            };
            form.update(cx, |_, cx| {
                cx.emit(ConnectionAction::ApplyDraft(Box::new(completed)))
            });
        });
        cx.run_until_parked();
        fixture.update(cx, |view, _, cx| {
            assert!(view.selection_is_ready(cx));
            view.submit(cx);
        });
        cx.run_until_parked();
        let actions = fixture.actions.borrow();
        let [SshImportAction::Submit { profiles, .. }] = actions.as_slice() else {
            panic!("metadata import");
        };
        assert_eq!(profiles.len(), 2);
        assert!(profiles.iter().all(|profile| !profile.remember_credentials));
        assert!(matches!(
            profiles[0].auth,
            AuthConfig::Password { password: None }
        ));
        assert!(
            matches!(&profiles[1].auth, AuthConfig::Key { key_path, passphrase: None } if key_path == "/explicit/second")
        );
    }

    #[gpui::test]
    fn path_changes_ignore_focus_events_and_reject_stale_results(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx, READY, library());
        fixture.load(cx);
        fixture.update(cx, |view, window, cx| {
            let old_generation = view.generation;
            view.path.update(cx, |_, cx| cx.emit(InputEvent::Focus));
            assert_eq!(view.preview_count_for_test(), 2);
            view.path.update(cx, |input, cx| {
                input.set_value("/synthetic/changed-config", window, cx);
                cx.emit(InputEvent::Change);
            });
            view.set_selected(0, true, cx);
            view.submit(cx);
            assert!(!view.busy);
            assert_eq!(view.generation, old_generation);
        });
        cx.run_until_parked();
        fixture.update(cx, |view, _, cx| {
            assert!(view.preview.is_none());
            assert_eq!(view.generation, 2);
            view.loading = true;
            view.finish_load(
                1,
                read_source(
                    &fixture.source.path.display().to_string(),
                    fixture.source.home.clone(),
                ),
                cx,
            );
            assert!(view.preview.is_none());
            view.finish_load(
                2,
                read_source(
                    &fixture.source.path.display().to_string(),
                    fixture.source.home.clone(),
                ),
                cx,
            );
            assert_eq!(view.preview_count_for_test(), 2);
        });
        assert!(fixture.actions.borrow().is_empty());
    }

    #[gpui::test]
    fn missing_empty_failed_and_cancelled_reads_never_leave_a_stale_selection(
        cx: &mut TestAppContext,
    ) {
        let fixture = Fixture::new(cx, READY, library());
        fixture.load(cx);
        fixture.update(cx, |view, _, cx| view.set_selected(0, true, cx));
        fs::remove_file(&fixture.source.path).unwrap();
        fixture.load(cx);
        fixture.update(cx, |view, _, cx| {
            assert!(view.error.is_some());
            assert!(view.preview.is_none());
            view.submit(cx);
            assert!(!view.busy);
        });
        fs::write(&fixture.source.path, "# empty configuration\n").unwrap();
        fixture.load(cx);
        fixture.update(cx, |view, _, _| {
            assert!(view.error.is_none());
            assert!(view.preview.is_some());
            assert_eq!(view.preview_count_for_test(), 0);
        });
        fs::write(&fixture.source.path, READY).unwrap();
        fixture.update(cx, |view, window, cx| {
            view.load(window, cx);
            view.cancel(cx);
        });
        cx.run_until_parked();
        fixture.update(cx, |view, _, _| {
            assert!(view.closed && view.preview.is_none())
        });
        assert!(matches!(
            fixture.actions.borrow().as_slice(),
            [SshImportAction::Cancel]
        ));
        assert_eq!(fs::read_to_string(&fixture.source.path).unwrap(), READY);
    }

    #[gpui::test]
    fn import_preview_and_draft_keep_a_reachable_footer_at_small_viewports(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::{px, size, VisualTestContext};

        let fixture = Fixture::new(cx, READY, library());
        fixture.load(cx);
        let mut visual = VisualTestContext::from_window(fixture.window.into(), cx);
        for viewport in [size(px(1280.), px(820.)), size(px(860.), px(600.))] {
            visual.simulate_resize(viewport);
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let content = visual.debug_bounds("ssh-import-viewport").unwrap();
            let footer = visual.debug_bounds("ssh-import-footer").unwrap();
            assert!(content.size.height >= px(120.));
            assert!(
                footer.origin.y >= content.origin.y + content.size.height,
                "content={content:?}; footer={footer:?}"
            );
            assert!(footer.origin.y + footer.size.height <= viewport.height);
        }
        visual.update(|window, cx| fixture.view.update(cx, |view, cx| view.edit(0, window, cx)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let content = visual.debug_bounds("connection-viewport").unwrap();
        let footer = visual.debug_bounds("connection-footer").unwrap();
        assert!(content.size.height >= px(120.));
        assert!(
            footer.origin.y >= content.origin.y + content.size.height,
            "content={content:?}; footer={footer:?}"
        );
        assert!(footer.origin.y + footer.size.height <= px(600.));
    }

    #[gpui::test]
    fn empty_preview_keeps_read_controls_next_to_the_path_field(cx: &mut TestAppContext) {
        use gpui_kit::{px, size, VisualTestContext};

        let fixture = Fixture::new(cx, READY, library());
        let mut visual = VisualTestContext::from_window(fixture.window.into(), cx);
        for viewport in [size(px(1280.), px(820.)), size(px(860.), px(600.))] {
            visual.simulate_resize(viewport);
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let field = visual.debug_bounds("ssh-import-source-field").unwrap();
            let actions = visual.debug_bounds("ssh-import-source-actions").unwrap();
            assert!(field.size.height > px(30.) && field.size.height < px(80.));
            assert!(actions.origin.y >= field.origin.y + field.size.height);
            assert!(actions.origin.y - field.origin.y < px(100.));
        }
    }

    #[gpui::test]
    fn missing_destination_cannot_silently_import_into_root(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx, READY, library());
        fixture.load(cx);
        fixture.update(cx, |view, window, cx| {
            view.set_selected(0, true, cx);
            view.folder.update(cx, |state, cx| {
                state.set_selected_value(&Some("missing".into()), window, cx)
            });
            view.submit(cx);
            assert!(!view.busy);
            assert!(!view.selection_is_ready(cx));
            view.folder
                .update(cx, |state, cx| state.set_selected_value(&None, window, cx));
            assert!(view.selection_is_ready(cx));
        });
        assert!(fixture.actions.borrow().is_empty());
    }

    #[gpui::test]
    fn failed_persistence_can_retry_without_reread_or_lost_selection(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx, READY, library());
        fixture.load(cx);
        fixture.update(cx, |view, _, cx| {
            view.set_selected(0, true, cx);
            view.submit(cx);
        });
        cx.run_until_parked();
        let generation = fixture.update(cx, |view, _, cx| {
            let generation = view.generation;
            assert!(view.submission_is_current(generation));
            view.busy = false;
            view.error = Some("synthetic write failure".into());
            assert!(!view.submission_is_current(generation));
            assert_eq!(view.selected_count_for_test(), 1);
            view.submit(cx);
            generation
        });
        cx.run_until_parked();
        let actions = fixture.actions.borrow();
        assert_eq!(actions.len(), 2);
        for action in actions.iter() {
            let SshImportAction::Submit {
                generation: actual,
                profiles,
            } = action
            else {
                panic!("submission");
            };
            assert_eq!(*actual, generation);
            assert_eq!(profiles.len(), 1);
        }
    }

    #[test]
    fn tilde_source_and_key_expansion_use_the_injected_home() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        fs::create_dir_all(home.join("other")).unwrap();
        fs::write(
            home.join("other/config"),
            "Host alias\n User fixture\n IdentityFile ~/.ssh/never-read\n",
        )
        .unwrap();
        let (source_path, snapshot) = read_source("~/other/config", home.clone()).unwrap();
        assert_eq!(
            source_path,
            home.join("other/config").canonicalize().unwrap()
        );
        assert_eq!(
            snapshot.entries[0].identity_files,
            vec![home.join(".ssh/never-read").display().to_string()]
        );
        assert!(!home.join(".ssh/never-read").exists());
    }
}
