use gpui_kit::component::{
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, InputState},
    scroll::ScrollableElement,
    select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState},
    ActiveTheme, Disableable, IndexPath,
};
use gpui_kit::{prelude::*, *};

use crate::config::{AuthConfig, HostLibrary, SessionConfig};

#[derive(Clone)]
pub(crate) struct FolderOption {
    pub id: Option<String>,
    pub label: SharedString,
}

impl SelectItem for FolderOption {
    type Value = Option<String>;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.id
    }
}

pub(crate) fn folder_options(library: &HostLibrary) -> Vec<FolderOption> {
    library
        .folder_options()
        .into_iter()
        .map(|(id, label)| FolderOption {
            id,
            label: label.into(),
        })
        .collect()
}

#[derive(Clone, Debug, Default)]
pub struct SshImportOptions {
    pub identity_files: Vec<String>,
    pub notices: Vec<String>,
    pub require_auth_choice: bool,
}

#[derive(Clone)]
pub enum ConnectionAction {
    Submit {
        profile: Box<SessionConfig>,
        secret: Option<String>,
        save: bool,
        connect: bool,
    },
    ApplyDraft(Box<SessionConfig>),
    Cancel,
}

pub struct ConnectionForm {
    profile: SessionConfig,
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    folder: Entity<SelectState<SearchableVec<FolderOption>>>,
    folders: Vec<FolderOption>,
    draft: bool,
    key_path: Entity<InputState>,
    secret: Entity<InputState>,
    authentication: Entity<SelectState<SearchableVec<&'static str>>>,
    remember: bool,
    ssh_import: Option<SshImportOptions>,
    pub busy: bool,
    pub error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl ConnectionForm {
    pub fn new(
        profile: Option<SessionConfig>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let profile = profile.unwrap_or_else(|| SessionConfig {
            id: uuid::Uuid::new_v4().to_string(),
            ..Default::default()
        });
        let auth_index = match profile.auth {
            AuthConfig::Password { .. } => 0,
            AuthConfig::Key { .. } => 1,
            AuthConfig::Agent => 2,
        };
        let key_path = match &profile.auth {
            AuthConfig::Key { key_path, .. } => key_path.clone(),
            _ => String::new(),
        };
        let authentication = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(vec!["密码", "私钥", "SSH Agent"]),
                Some(IndexPath::new(auth_index)),
                window,
                cx,
            )
        });
        let subscription =
            cx.subscribe(&authentication, |_, _, _: &SelectEvent<_>, cx| cx.notify());
        let folders = folder_options(&HostLibrary::default());
        let folder = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(folders.clone()),
                profile.folder_id.is_none().then(|| IndexPath::new(0)),
                window,
                cx,
            )
            .searchable(true)
        });
        let folder_subscription = cx.subscribe(&folder, |_, _, _: &SelectEvent<_>, cx| cx.notify());
        Self {
            name: field(&profile.name, "例如：生产数据库", false, window, cx),
            host: field(&profile.host, "主机名或 IP 地址", false, window, cx),
            port: field(&profile.port.to_string(), "22", false, window, cx),
            username: field(&profile.username, "远程用户名", false, window, cx),
            folder,
            folders,
            draft: false,
            key_path: field(&key_path, "~/.ssh/id_ed25519", false, window, cx),
            secret: field("", "不保存时仅用于当前会话", true, window, cx),
            remember: profile.remember_credentials,
            authentication,
            profile,
            ssh_import: None,
            busy: false,
            error: None,
            _subscriptions: vec![subscription, folder_subscription],
        }
    }

    pub fn set_folders(
        &mut self,
        library: &HostLibrary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self
            .folder
            .read(cx)
            .selected_value()
            .cloned()
            .unwrap_or_else(|| self.profile.folder_id.clone());
        self.folders = folder_options(library);
        self.folder.update(cx, |state, cx| {
            state.set_items(SearchableVec::new(self.folders.clone()), window, cx);
            state.set_selected_value(&selected, window, cx);
        });
        cx.notify();
    }

    pub fn set_folder(
        &mut self,
        folder_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.folder.update(cx, |state, cx| {
            state.set_selected_value(&folder_id, window, cx);
        });
        cx.notify();
    }

    pub fn new_import_draft(
        mut profile: SessionConfig,
        library: &HostLibrary,
        options: SshImportOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        profile.remember_credentials = false;
        let mut form = Self::new(Some(profile), window, cx);
        form.draft = true;
        form.remember = false;
        form.set_folders(library, window, cx);
        form.configure_ssh_import(options, window, cx);
        form
    }

    pub fn configure_ssh_import(
        &mut self,
        options: SshImportOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if options.require_auth_choice {
            self.authentication.update(cx, |state, cx| {
                state.set_selected_index(None, window, cx);
            });
            self.key_path
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.ssh_import = Some(options);
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn submit_for_test(&mut self, save: bool, connect: bool, cx: &mut Context<Self>) {
        self.submit(save, connect, cx);
    }

    fn choose_identity(&mut self, key_path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.key_path
            .update(cx, |input, cx| input.set_value(key_path, window, cx));
        self.authentication.update(cx, |state, cx| {
            state.set_selected_index(Some(IndexPath::new(1)), window, cx);
        });
        self.error = None;
        cx.notify();
    }

    fn submit(&mut self, save: bool, connect: bool, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let host = self.host.read(cx).value().trim().to_owned();
        let username = self.username.read(cx).value().trim().to_owned();
        let port = self.port.read(cx).value().parse::<u16>();
        let port = match port {
            Ok(port) if port > 0 => port,
            _ => {
                self.error = Some("端口必须是 1–65535 之间的整数".into());
                cx.notify();
                return;
            }
        };
        if host.is_empty() || username.is_empty() || host.chars().any(char::is_whitespace) {
            self.error = Some("请填写有效主机名和用户名".into());
            cx.notify();
            return;
        }
        let auth = match self.authentication.read(cx).selected_value().copied() {
            Some("私钥") => {
                let key_path = self.key_path.read(cx).value().trim().to_owned();
                if key_path.is_empty() {
                    self.error = Some("请填写私钥文件路径".into());
                    cx.notify();
                    return;
                }
                AuthConfig::Key {
                    key_path,
                    passphrase: None,
                }
            }
            Some("SSH Agent") => AuthConfig::Agent,
            Some("密码") => AuthConfig::Password { password: None },
            _ => {
                self.error = Some("请选择认证方式，或选择下方配置中的私钥".into());
                cx.notify();
                return;
            }
        };
        let mut profile = self.profile.clone();
        profile.host = host;
        profile.username = username;
        profile.port = port;
        profile.name = self.name.read(cx).value().trim().to_owned();
        if profile.name.is_empty() {
            profile.name = format!("{}@{}", profile.username, profile.host);
        }
        if !self.draft {
            let Some(folder_id) = self.folder.read(cx).selected_value().cloned() else {
                self.error = Some("请选择有效的目标文件夹".into());
                cx.notify();
                return;
            };
            if !self.folders.iter().any(|folder| folder.id == folder_id) {
                self.error = Some("目标文件夹已不存在，请重新选择".into());
                cx.notify();
                return;
            }
            profile.folder_id = folder_id;
        }
        profile.remember_credentials =
            !self.draft && self.remember && !matches!(auth, AuthConfig::Agent);
        profile.auth = auth;
        if self.draft {
            self.busy = true;
            self.error = None;
            cx.emit(ConnectionAction::ApplyDraft(Box::new(profile)));
            cx.notify();
            return;
        }
        let secret = self.secret.read(cx).value().to_string();
        let target_changed = profile.host != self.profile.host
            || profile.port != self.profile.port
            || profile.username != self.profile.username
            || !same_auth_target(&profile.auth, &self.profile.auth);
        if self.profile.remember_credentials
            && profile.remember_credentials
            && target_changed
            && secret.is_empty()
        {
            self.error = Some(
                "主机或认证方式已改变，请输入新凭据，或取消记住凭据。原凭据不会用于新目标。".into(),
            );
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        cx.emit(ConnectionAction::Submit {
            profile: Box::new(profile),
            secret: (!secret.is_empty()).then_some(secret),
            save,
            connect,
        });
        cx.notify();
    }
}

impl EventEmitter<ConnectionAction> for ConnectionForm {}

impl Render for ConnectionForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let authentication = self.authentication.read(cx).selected_value().copied();
        let key = authentication == Some("私钥");
        let password = authentication == Some("密码");
        let body = div()
            .id("connection-scroll")
            .debug_selector(|| "connection-scroll-content".into())
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .min_h_0()
            .h((window.viewport_size().height * 0.70 - px(110.)).max(px(120.)))
            .pr_1()
            .when_some(self.ssh_import.clone(), |view, options| {
                view.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("来自 SSH Config；原文件只读，保存仅创建应用内副本。")
                        .children(
                            options
                                .notices
                                .into_iter()
                                .map(|notice| div().child(notice)),
                        ),
                )
                .when(!options.identity_files.is_empty(), |view| {
                    view.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_sm().child("配置中的私钥（点击选择）"))
                            .children(options.identity_files.into_iter().enumerate().map(
                                |(index, path)| {
                                    Button::new(("ssh-import-identity", index))
                                        .label(path.clone())
                                        .w_full()
                                        .disabled(self.busy)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.choose_identity(path.clone(), window, cx);
                                        }))
                                },
                            )),
                    )
                })
            })
            .child(row("名称", Input::new(&self.name).id("connection-name")))
            .child(row("主机", Input::new(&self.host).id("connection-host")))
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(
                        row("端口", Input::new(&self.port).id("connection-port"))
                            .debug_selector(|| "connection-port-field".into())
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        row("用户名", Input::new(&self.username).id("connection-user"))
                            .debug_selector(|| "connection-user-field".into())
                            .flex_1()
                            .min_w_0(),
                    ),
            )
            .when(!self.draft, |view| {
                view.child(row(
                    "文件夹",
                    Select::new(&self.folder)
                        .id("connection-folder")
                        .disabled(self.busy),
                ))
            })
            .child(row(
                "认证方式",
                Select::new(&self.authentication).id("connection-auth"),
            ))
            .when(key, |view| {
                view.child(row(
                    "私钥路径",
                    Input::new(&self.key_path).id("connection-key"),
                ))
            })
            .when(!self.draft && (key || password), |view| {
                view.child(row(
                    if key {
                        "私钥口令（可留空）"
                    } else {
                        "密码"
                    },
                    Input::new(&self.secret).id("connection-secret"),
                ))
                .child(
                    Checkbox::new("remember-credentials")
                        .label("记住凭据（系统钥匙串）")
                        .checked(self.remember)
                        .on_change(cx.listener(|this, checked, _, cx| {
                            this.remember = *checked;
                            cx.notify();
                        })),
                )
            })
            .when(
                !self.draft && self.profile.remember_credentials && (key || password),
                |view| {
                    view.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("密码留空时使用系统钥匙串中的凭据；留空不会清除已保存的凭据。"),
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
                    .debug_selector(|| "connection-viewport".into())
                    .w_full()
                    .child(body),
            )
            .child(
                div()
                    .debug_selector(|| "connection-footer".into())
                    .flex()
                    .flex_shrink_0()
                    .justify_end()
                    .gap_2()
                    .pt_3()
                    .child(
                        Button::new("connection-cancel")
                            .ghost()
                            .label(if self.draft { "返回" } else { "取消" })
                            .disabled(self.busy)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ConnectionAction::Cancel))),
                    )
                    .when(self.draft, |view| {
                        view.child(
                            Button::new("connection-apply-draft")
                                .primary()
                                .label("采用配置")
                                .disabled(self.busy)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.submit(false, false, cx)),
                                ),
                        )
                    })
                    .when(!self.draft, |view| {
                        view.child(
                            Button::new("connection-save")
                                .label("保存")
                                .disabled(self.busy)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.submit(true, false, cx)),
                                ),
                        )
                        .child(
                            Button::new("connection-connect")
                                .label("仅连接")
                                .disabled(self.busy)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.submit(false, true, cx)),
                                ),
                        )
                        .child(
                            Button::new("connection-save-connect")
                                .primary()
                                .label(if self.busy {
                                    "处理中…"
                                } else {
                                    "保存并连接"
                                })
                                .disabled(self.busy)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.submit(true, true, cx)),
                                ),
                        )
                    }),
            )
    }
}

pub(crate) fn field<T: 'static>(
    value: &str,
    placeholder: &str,
    masked: bool,
    window: &mut Window,
    cx: &mut Context<T>,
) -> Entity<InputState> {
    cx.new(|cx| {
        InputState::new(window, cx)
            .default_value(value.to_owned())
            .placeholder(placeholder.to_owned())
            .masked(masked)
    })
}

fn same_auth_target(left: &AuthConfig, right: &AuthConfig) -> bool {
    match (left, right) {
        (AuthConfig::Password { .. }, AuthConfig::Password { .. })
        | (AuthConfig::Agent, AuthConfig::Agent) => true,
        (
            AuthConfig::Key { key_path: left, .. },
            AuthConfig::Key {
                key_path: right, ..
            },
        ) => left == right,
        _ => false,
    }
}

pub(crate) fn row(label: &'static str, input: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap_1()
        .child(div().text_sm().child(label))
        .child(input)
}

#[cfg(test)]
mod tests {
    use super::{ConnectionAction, ConnectionForm, SshImportOptions};
    use crate::config::{AuthConfig, Folder, HostLibrary, ImportOrigin, SessionConfig};
    use gpui_kit::{
        AppContext, Context, Entity, Subscription, TestAppContext, Window, WindowHandle,
    };
    use std::{cell::RefCell, rc::Rc};

    struct Fixture {
        window: WindowHandle<gpui_kit::base::Root>,
        form: Entity<ConnectionForm>,
        actions: Rc<RefCell<Vec<ConnectionAction>>>,
        _subscription: Subscription,
    }

    impl Fixture {
        fn new(cx: &mut TestAppContext, profile: SessionConfig) -> Self {
            cx.update(gpui_kit::init);
            let (window, form) = cx.update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| ConnectionForm::new(Some(profile), window, cx))
                })
                .expect("test connection window")
            });
            let actions = Rc::new(RefCell::new(Vec::new()));
            let collected = actions.clone();
            let subscription = cx.update(|cx| {
                cx.subscribe(&form, move |_, action: &ConnectionAction, _| {
                    collected.borrow_mut().push(action.clone());
                })
            });
            Self {
                window: window.downcast().expect("Base Root"),
                form,
                actions,
                _subscription: subscription,
            }
        }

        fn draft(
            cx: &mut TestAppContext,
            profile: SessionConfig,
            options: SshImportOptions,
        ) -> Self {
            cx.update(gpui_kit::init);
            let (window, form) = cx.update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| {
                        ConnectionForm::new_import_draft(
                            profile,
                            &folder_library(),
                            options,
                            window,
                            cx,
                        )
                    })
                })
                .expect("draft connection window")
            });
            let actions = Rc::new(RefCell::new(Vec::new()));
            let collected = actions.clone();
            let subscription = cx.update(|cx| {
                cx.subscribe(&form, move |_, action: &ConnectionAction, _| {
                    collected.borrow_mut().push(action.clone());
                })
            });
            Self {
                window: window.downcast().expect("Base Root"),
                form,
                actions,
                _subscription: subscription,
            }
        }

        fn update<R>(
            &self,
            cx: &mut TestAppContext,
            update: impl FnOnce(&mut ConnectionForm, &mut Window, &mut Context<ConnectionForm>) -> R,
        ) -> R {
            cx.update_window(self.window.into(), |_, window, cx| {
                self.form.update(cx, |form, cx| update(form, window, cx))
            })
            .expect("update connection form")
        }
    }

    fn folder_library() -> HostLibrary {
        HostLibrary {
            folders: vec![
                Folder {
                    id: "development".into(),
                    name: "Development".into(),
                    parent_id: None,
                },
                Folder {
                    id: "production".into(),
                    name: "Production".into(),
                    parent_id: Some("development".into()),
                },
            ],
            sessions: vec![],
        }
    }

    fn password_profile() -> SessionConfig {
        SessionConfig {
            id: "test-password-profile".into(),
            name: "Original host".into(),
            host: "host-a.test.invalid".into(),
            username: "test-user".into(),
            auth: AuthConfig::Password { password: None },
            ..Default::default()
        }
    }

    #[gpui::test]
    fn invalid_connection_fields_show_errors_without_submitting(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx, password_profile());
        for (host, username, port, expected_error) in [
            ("", "test-user", "22", "请填写有效主机名和用户名"),
            ("host-a.test.invalid", "", "22", "请填写有效主机名和用户名"),
            ("  ", "test-user", "22", "请填写有效主机名和用户名"),
            (
                "host-a.test.invalid",
                "  ",
                "22",
                "请填写有效主机名和用户名",
            ),
            (
                "invalid host",
                "test-user",
                "22",
                "请填写有效主机名和用户名",
            ),
            (
                "host-a.test.invalid",
                "test-user",
                "0",
                "端口必须是 1–65535 之间的整数",
            ),
            (
                "host-a.test.invalid",
                "test-user",
                "65536",
                "端口必须是 1–65535 之间的整数",
            ),
            (
                "host-a.test.invalid",
                "test-user",
                "ssh",
                "端口必须是 1–65535 之间的整数",
            ),
        ] {
            fixture.update(cx, |form, window, cx| {
                form.host
                    .update(cx, |input, cx| input.set_value(host, window, cx));
                form.username
                    .update(cx, |input, cx| input.set_value(username, window, cx));
                form.port
                    .update(cx, |input, cx| input.set_value(port, window, cx));
                form.submit(true, true, cx);
                assert!(!form.busy);
                assert_eq!(form.error.as_deref(), Some(expected_error));
            });
            cx.run_until_parked();
            assert!(fixture.actions.borrow().is_empty());
        }
    }

    #[gpui::test]
    fn valid_password_submission_keeps_masked_secret_out_of_profile(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx, password_profile());
        let secret = "test-only-password-not-for-profile";
        fixture.update(cx, |form, window, cx| {
            form.host.update(cx, |input, cx| {
                input.set_value("  host-a.test.invalid  ", window, cx)
            });
            form.username
                .update(cx, |input, cx| input.set_value("  test-user  ", window, cx));
            form.name.update(cx, |input, cx| {
                input.set_value("  Saved host  ", window, cx)
            });
            form.set_folders(&folder_library(), window, cx);
            form.set_folder(Some("development".into()), window, cx);
            form.secret
                .update(cx, |input, cx| input.set_value(secret, window, cx));
            assert!(form.secret.read(cx).presentation().is_masked());
            form.submit(true, true, cx);
            assert!(form.busy);
            assert!(form.error.is_none());
            form.submit(true, true, cx);
        });
        cx.run_until_parked();
        let actions = fixture.actions.borrow();
        assert_eq!(actions.len(), 1);
        let ConnectionAction::Submit {
            profile,
            secret: submitted_secret,
            save,
            connect,
        } = &actions[0]
        else {
            panic!("expected profile submission");
        };
        assert_eq!(profile.id, "test-password-profile");
        assert_eq!(profile.host, "host-a.test.invalid");
        assert_eq!(profile.username, "test-user");
        assert_eq!(profile.name, "Saved host");
        assert_eq!(profile.folder_id.as_deref(), Some("development"));
        assert_eq!(profile.port, 22);
        assert!(matches!(
            profile.auth,
            AuthConfig::Password { password: None }
        ));
        assert_eq!(submitted_secret.as_deref(), Some(secret));
        assert!(*save);
        assert!(*connect);
        let serialized = toml::to_string(profile.as_ref()).expect("serialize submitted profile");
        assert!(!serialized.contains(secret));
        let restored: SessionConfig =
            toml::from_str(&serialized).expect("restore profile without password");
        assert!(matches!(
            restored.auth,
            AuthConfig::Password { password: None }
        ));
    }

    #[gpui::test]
    fn imported_host_requires_an_explicit_authentication_choice(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx, password_profile());
        fixture.update(cx, |form, window, cx| {
            form.configure_ssh_import(
                SshImportOptions {
                    require_auth_choice: true,
                    ..Default::default()
                },
                window,
                cx,
            );
            assert!(form.authentication.read(cx).selected_value().is_none());
            form.submit(false, true, cx);
            assert!(!form.busy);
            assert_eq!(
                form.error.as_deref(),
                Some("请选择认证方式，或选择下方配置中的私钥")
            );
        });
        cx.run_until_parked();
        assert!(fixture.actions.borrow().is_empty());
        fixture.update(cx, |form, window, cx| {
            form.authentication.update(cx, |state, cx| {
                state.set_selected_index(Some(gpui_kit::component::IndexPath::new(2)), window, cx);
            });
            form.submit(false, true, cx);
            assert!(form.busy);
            assert!(form.error.is_none());
        });
        cx.run_until_parked();
        let actions = fixture.actions.borrow();
        let [ConnectionAction::Submit {
            profile,
            save,
            connect,
            ..
        }] = actions.as_slice()
        else {
            panic!("expected one explicit authentication submission");
        };
        assert!(matches!(profile.auth, AuthConfig::Agent));
        assert!(!*save);
        assert!(*connect);
    }

    #[gpui::test]
    fn imported_identity_candidates_are_not_selected_implicitly(cx: &mut TestAppContext) {
        let mut profile = password_profile();
        profile.auth = AuthConfig::Key {
            key_path: "/synthetic/first-key".into(),
            passphrase: None,
        };
        let fixture = Fixture::new(cx, profile);
        fixture.update(cx, |form, window, cx| {
            form.configure_ssh_import(
                SshImportOptions {
                    identity_files: vec![
                        "/synthetic/first-key".into(),
                        "/synthetic/second-key".into(),
                    ],
                    notices: vec!["选择一个密钥；不会尝试其他密钥。".into()],
                    require_auth_choice: true,
                },
                window,
                cx,
            );
            assert!(form.authentication.read(cx).selected_value().is_none());
            assert!(form.key_path.read(cx).value().is_empty());
            form.submit(false, true, cx);
            assert!(!form.busy);
            form.choose_identity("/synthetic/second-key".into(), window, cx);
            assert_eq!(
                form.authentication.read(cx).selected_value().copied(),
                Some("私钥")
            );
            assert_eq!(
                form.key_path.read(cx).value().as_str(),
                "/synthetic/second-key"
            );
            form.submit(true, false, cx);
            assert!(form.busy);
            assert!(form.error.is_none());
        });
        cx.run_until_parked();
        let actions = fixture.actions.borrow();
        let [ConnectionAction::Submit {
            profile,
            secret,
            save,
            connect,
        }] = actions.as_slice()
        else {
            panic!("expected only the explicitly chosen key submission");
        };
        assert!(matches!(
            &profile.auth,
            AuthConfig::Key { key_path, passphrase: None } if key_path == "/synthetic/second-key"
        ));
        assert!(secret.is_none());
        assert!(*save);
        assert!(!*connect);
    }

    #[gpui::test]
    fn imported_single_key_retains_private_key_authentication(cx: &mut TestAppContext) {
        let mut profile = password_profile();
        profile.auth = AuthConfig::Key {
            key_path: "/synthetic/only-key".into(),
            passphrase: None,
        };
        let fixture = Fixture::new(cx, profile);
        fixture.update(cx, |form, window, cx| {
            form.configure_ssh_import(
                SshImportOptions {
                    identity_files: vec!["/synthetic/only-key".into()],
                    ..Default::default()
                },
                window,
                cx,
            );
            assert_eq!(
                form.authentication.read(cx).selected_value().copied(),
                Some("私钥")
            );
            assert_eq!(
                form.key_path.read(cx).value().as_str(),
                "/synthetic/only-key"
            );
            form.submit(false, true, cx);
            assert!(form.busy);
        });
        cx.run_until_parked();
        assert_eq!(fixture.actions.borrow().len(), 1);
    }

    #[gpui::test]
    fn remembered_password_cannot_follow_a_changed_host_without_new_secret(
        cx: &mut TestAppContext,
    ) {
        let mut profile = password_profile();
        profile.remember_credentials = true;
        let fixture = Fixture::new(cx, profile);
        fixture.update(cx, |form, window, cx| {
            assert!(form.secret.read(cx).value().is_empty());
            form.host.update(cx, |input, cx| {
                input.set_value("host-b.test.invalid", window, cx)
            });
            form.submit(true, false, cx);
            assert!(!form.busy);
            assert_eq!(
                form.error.as_deref(),
                Some("主机或认证方式已改变，请输入新凭据，或取消记住凭据。原凭据不会用于新目标。")
            );
            assert_eq!(form.profile.host, "host-a.test.invalid");
        });
        cx.run_until_parked();
        assert!(fixture.actions.borrow().is_empty());
    }

    #[gpui::test]
    fn paired_connection_fields_share_usable_width_without_overflow(cx: &mut TestAppContext) {
        use gpui_kit::{px, size, VisualTestContext};

        let fixture = Fixture::new(cx, password_profile());
        let mut visual = VisualTestContext::from_window(fixture.window.into(), cx);
        for viewport in [size(px(1280.), px(820.)), size(px(860.), px(600.))] {
            visual.simulate_resize(viewport);
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let port = visual.debug_bounds("connection-port-field").unwrap();
            let user = visual.debug_bounds("connection-user-field").unwrap();
            assert!(port.size.width >= px(180.) && user.size.width >= px(180.));
            assert!((port.size.width - user.size.width).abs() <= px(1.));
            assert!(port.origin.x >= px(0.));
            assert!(user.origin.x >= port.origin.x + port.size.width);
            assert!(user.origin.x + user.size.width <= viewport.width);
        }
    }

    #[gpui::test]
    fn folder_selector_preserves_ids_and_rejects_missing_destinations(cx: &mut TestAppContext) {
        let mut profile = password_profile();
        profile.folder_id = Some("production".into());
        let fixture = Fixture::new(cx, profile);
        fixture.update(cx, |form, window, cx| {
            form.set_folders(&folder_library(), window, cx);
            assert_eq!(
                form.folder.read(cx).selected_value(),
                Some(&Some("production".into()))
            );
            assert!(form
                .folders
                .iter()
                .any(|folder| folder.id.as_deref() == Some("production")
                    && folder.label.as_str() == "Development / Production"));
            form.set_folders(&HostLibrary::default(), window, cx);
            assert!(form.folder.read(cx).selected_value().is_none());
            form.submit(true, false, cx);
            assert!(!form.busy);
            assert!(form.error.is_some());
            form.set_folder(None, window, cx);
            form.submit(true, false, cx);
            assert!(form.busy);
        });
        cx.run_until_parked();
        let actions = fixture.actions.borrow();
        let [ConnectionAction::Submit { profile, .. }] = actions.as_slice() else {
            panic!("expected valid root submission");
        };
        assert!(profile.folder_id.is_none());
    }

    #[gpui::test]
    fn import_draft_emits_only_metadata_and_requires_explicit_authentication(
        cx: &mut TestAppContext,
    ) {
        let mut profile = password_profile();
        profile.remember_credentials = true;
        profile.import_origin = Some(ImportOrigin {
            source_path: "/synthetic/config".into(),
            alias: "original".into(),
        });
        let fixture = Fixture::draft(
            cx,
            profile,
            SshImportOptions {
                require_auth_choice: true,
                ..Default::default()
            },
        );
        fixture.update(cx, |form, window, cx| {
            assert!(form.draft);
            assert!(!form.remember);
            form.secret.update(cx, |input, cx| {
                input.set_value("must-not-escape", window, cx)
            });
            form.submit(true, true, cx);
            assert!(!form.busy);
            form.authentication.update(cx, |state, cx| {
                state.set_selected_index(Some(gpui_kit::component::IndexPath::new(0)), window, cx)
            });
            form.submit(true, true, cx);
            assert!(form.busy);
        });
        cx.run_until_parked();
        let actions = fixture.actions.borrow();
        let [ConnectionAction::ApplyDraft(profile)] = actions.as_slice() else {
            panic!("draft must not emit Submit");
        };
        assert!(!profile.remember_credentials);
        assert!(matches!(
            profile.auth,
            AuthConfig::Password { password: None }
        ));
        assert_eq!(profile.import_origin.as_ref().unwrap().alias, "original");
        assert!(!toml::to_string(profile.as_ref())
            .unwrap()
            .contains("must-not-escape"));
    }

    #[gpui::test]
    fn remembered_password_survives_name_and_folder_only_edits(cx: &mut TestAppContext) {
        let mut profile = password_profile();
        profile.remember_credentials = true;
        let fixture = Fixture::new(cx, profile);
        fixture.update(cx, |form, window, cx| {
            form.name
                .update(cx, |input, cx| input.set_value("Renamed host", window, cx));
            form.set_folders(&folder_library(), window, cx);
            form.set_folder(Some("production".into()), window, cx);
            assert!(form.secret.read(cx).value().is_empty());
            form.submit(true, false, cx);
            assert!(form.busy);
            assert!(form.error.is_none());
        });
        cx.run_until_parked();
        let actions = fixture.actions.borrow();
        assert_eq!(actions.len(), 1);
        let ConnectionAction::Submit {
            profile,
            secret,
            save,
            connect,
        } = &actions[0]
        else {
            panic!("expected metadata-only submission");
        };
        assert_eq!(profile.id, "test-password-profile");
        assert_eq!(profile.name, "Renamed host");
        assert_eq!(profile.folder_id.as_deref(), Some("production"));
        assert_eq!(profile.host, "host-a.test.invalid");
        assert_eq!(profile.username, "test-user");
        assert_eq!(profile.port, 22);
        assert!(profile.remember_credentials);
        assert!(matches!(
            profile.auth,
            AuthConfig::Password { password: None }
        ));
        assert!(secret.is_none());
        assert!(*save);
        assert!(!*connect);
    }
}
