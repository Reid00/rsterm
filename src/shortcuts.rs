use gpui_kit::{Action, App, Global};

use crate::config::{EffectiveKeymap, ShortcutConfig};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutScope {
    Workspace,
    Terminal,
    HostsTree,
    Global,
}

impl ShortcutScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::Workspace => "工作区",
            Self::Terminal => "仅终端",
            Self::HostsTree => "仅主机树",
            Self::Global => "应用",
        }
    }

    pub fn overlaps(self, other: Self) -> bool {
        self == other
            || matches!(self, Self::Workspace | Self::Global)
            || matches!(other, Self::Workspace | Self::Global)
    }
}

pub struct ShortcutDefinition {
    pub id: &'static str,
    pub label: &'static str,
    pub scope: ShortcutScope,
    pub non_input: bool,
    pub macos: &'static str,
    pub other: &'static str,
    pub action: fn() -> Box<dyn Action>,
}

impl ShortcutDefinition {
    pub fn predicate(&self) -> Option<&'static str> {
        match self.scope {
            ShortcutScope::Workspace if self.non_input => {
                Some("RSTerm && !Input && !Dialog && !RstermModal")
            }
            ShortcutScope::Workspace => Some("RSTerm && !Dialog && !RstermModal"),
            ShortcutScope::Terminal => Some("Terminal && !Dialog && !RstermModal"),
            ShortcutScope::HostsTree => Some("HostsTree && !Input && !Dialog && !RstermModal"),
            ShortcutScope::Global => None,
        }
    }

    pub fn scope_label(&self) -> &'static str {
        if self.non_input {
            "工作区（输入框除外）"
        } else {
            self.scope.label()
        }
    }
}

macro_rules! shortcut_catalog {
    ($(($action:ident, $id:literal, $label:literal, $scope:ident, $non_input:literal, $mac:literal, $other:literal)),+ $(,)?) => {
        gpui_kit::actions!(rsterm, [$($action),+]);

        pub static CATALOG: &[ShortcutDefinition] = &[
            $(ShortcutDefinition {
                id: $id,
                label: $label,
                scope: ShortcutScope::$scope,
                non_input: $non_input,
                macos: $mac,
                other: $other,
                action: || Box::new($action),
            }),+
        ];
    };
}

shortcut_catalog!(
    (
        NewConnection,
        "new_connection",
        "新建 SSH 连接",
        Workspace,
        false,
        "cmd-n",
        "ctrl-shift-n"
    ),
    (
        NewLocalTerminal,
        "new_local_terminal",
        "新建本地终端",
        Workspace,
        false,
        "cmd-t",
        "ctrl-shift-t"
    ),
    (
        CloseTab,
        "close_tab",
        "关闭当前标签",
        Workspace,
        false,
        "cmd-w",
        "ctrl-shift-w"
    ),
    (
        OpenSettings,
        "open_settings",
        "设置",
        Workspace,
        false,
        "cmd-,",
        "ctrl-shift-,"
    ),
    (Quit, "quit", "退出", Global, false, "cmd-q", "ctrl-shift-q"),
    (
        NextTab,
        "next_tab",
        "下一个标签",
        Workspace,
        false,
        "ctrl-tab",
        "ctrl-tab"
    ),
    (
        PreviousTab,
        "previous_tab",
        "上一个标签",
        Workspace,
        false,
        "ctrl-shift-tab",
        "ctrl-shift-tab"
    ),
    (
        SelectTab1,
        "select_tab_1",
        "切换到标签 1",
        Workspace,
        false,
        "cmd-1",
        "alt-1"
    ),
    (
        SelectTab2,
        "select_tab_2",
        "切换到标签 2",
        Workspace,
        false,
        "cmd-2",
        "alt-2"
    ),
    (
        SelectTab3,
        "select_tab_3",
        "切换到标签 3",
        Workspace,
        false,
        "cmd-3",
        "alt-3"
    ),
    (
        SelectTab4,
        "select_tab_4",
        "切换到标签 4",
        Workspace,
        false,
        "cmd-4",
        "alt-4"
    ),
    (
        SelectTab5,
        "select_tab_5",
        "切换到标签 5",
        Workspace,
        false,
        "cmd-5",
        "alt-5"
    ),
    (
        SelectTab6,
        "select_tab_6",
        "切换到标签 6",
        Workspace,
        false,
        "cmd-6",
        "alt-6"
    ),
    (
        SelectTab7,
        "select_tab_7",
        "切换到标签 7",
        Workspace,
        false,
        "cmd-7",
        "alt-7"
    ),
    (
        SelectTab8,
        "select_tab_8",
        "切换到标签 8",
        Workspace,
        false,
        "cmd-8",
        "alt-8"
    ),
    (
        SelectLastTab,
        "select_last_tab",
        "切换到最后一个标签",
        Workspace,
        false,
        "cmd-9",
        "alt-9"
    ),
    (
        ToggleFiles,
        "toggle_files",
        "SFTP 文件",
        Workspace,
        true,
        "cmd-shift-f",
        "ctrl-alt-shift-f"
    ),
    (
        ShowSystem,
        "show_system",
        "系统面板",
        Workspace,
        false,
        "cmd-shift-i",
        "ctrl-alt-shift-i"
    ),
    (
        ShowNetwork,
        "show_network",
        "网络面板",
        Workspace,
        false,
        "cmd-shift-n",
        "ctrl-alt-shift-n"
    ),
    (
        ShowHistory,
        "show_history",
        "历史命令",
        Workspace,
        false,
        "cmd-shift-h",
        "ctrl-alt-shift-h"
    ),
    (
        FocusTerminal,
        "focus_terminal",
        "聚焦终端",
        Workspace,
        false,
        "cmd-shift-t",
        "ctrl-alt-shift-t"
    ),
    (
        FocusHostSearch,
        "focus_host_search",
        "聚焦主机搜索",
        Workspace,
        false,
        "cmd-shift-l",
        "ctrl-alt-shift-l"
    ),
    (
        ReconnectSession,
        "reconnect_session",
        "重连已断开的会话",
        Workspace,
        false,
        "cmd-shift-r",
        "ctrl-alt-shift-r"
    ),
    (
        ShowShortcuts,
        "show_shortcuts",
        "快捷键帮助",
        Workspace,
        false,
        "cmd-/",
        "ctrl-alt-shift-/"
    ),
    (
        TerminalCopy,
        "terminal_copy",
        "复制终端选区",
        Terminal,
        false,
        "cmd-c",
        "ctrl-shift-c"
    ),
    (
        TerminalPaste,
        "terminal_paste",
        "粘贴到终端",
        Terminal,
        false,
        "cmd-v",
        "ctrl-shift-v"
    ),
    (
        TerminalSelectAll,
        "terminal_select_all",
        "全选终端",
        Terminal,
        false,
        "cmd-a",
        "ctrl-shift-a"
    ),
    (
        MoveHostNode,
        "move_host_node",
        "移动主机或文件夹",
        HostsTree,
        false,
        "cmd-shift-m",
        "ctrl-alt-shift-m"
    ),
);

pub fn definition(id: &str) -> Option<&'static ShortcutDefinition> {
    CATALOG.iter().find(|definition| definition.id == id)
}

impl Global for ShortcutConfig {}

pub fn init(config: ShortcutConfig, cx: &mut App) {
    if cx.has_global::<ShortcutConfig>() {
        return;
    }
    cx.bind_keys(config.keymap.gpui_bindings());
    cx.set_global(config);
}

pub fn effective(cx: &App) -> Option<&EffectiveKeymap> {
    cx.try_global::<ShortcutConfig>()
        .map(|config| &config.keymap)
}

pub fn warning(cx: &App) -> Option<&str> {
    cx.try_global::<ShortcutConfig>()
        .and_then(|config| config.warning.as_deref())
}

pub fn hint(id: &str, cx: &App) -> String {
    if let Some(keymap) = effective(cx) {
        return keymap.hint(id);
    }
    definition(id)
        .map(|definition| {
            crate::config::display_shortcut(if cfg!(target_os = "macos") {
                definition.macos
            } else {
                definition.other
            })
        })
        .unwrap_or_default()
}

pub fn accessibility_hint(id: &str, cx: &App) -> String {
    effective(cx)
        .map(|keymap| keymap.accessibility_hint(id))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{backend::AppCommand, config::AppSettings, ui::terminal::TerminalView};
    use gpui_kit::component::input::{Input, InputState};
    use gpui_kit::{
        div, AppContext, ClipboardItem, Context, Entity, Focusable, InteractiveElement,
        IntoElement, KeyDownEvent, Keystroke, ParentElement, PlatformInput, Render, Styled,
        TestAppContext, Window, WindowHandle,
    };
    use tokio::sync::mpsc;

    struct DispatchView {
        terminal: Entity<TerminalView>,
        input: Entity<InputState>,
        next: usize,
        files: usize,
        modal: bool,
    }

    impl Render for DispatchView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .key_context(if self.modal {
                    "RSTerm RstermModal"
                } else {
                    "RSTerm"
                })
                .size_full()
                .flex()
                .flex_col()
                .on_action(cx.listener(|this, _: &NextTab, _, cx| {
                    this.next += 1;
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &ToggleFiles, _, cx| {
                    this.files += 1;
                    cx.notify();
                }))
                .child(Input::new(&self.input))
                .child(div().flex_1().min_h_0().child(self.terminal.clone()))
        }
    }

    struct Fixture {
        window: WindowHandle<gpui_kit::base::Root>,
        view: Entity<DispatchView>,
        commands: mpsc::Receiver<AppCommand>,
    }

    impl Fixture {
        fn new(content: Option<&str>, cx: &mut TestAppContext) -> Self {
            let path = std::path::PathBuf::from("/fixture/keybindings.toml");
            let config = ShortcutConfig {
                keymap: content
                    .map(|content| EffectiveKeymap::parse(content, path.clone()).unwrap())
                    .unwrap_or_else(|| EffectiveKeymap::defaults(path)),
                warning: None,
            };
            cx.update(|cx| {
                gpui_kit::init(cx);
                crate::ui::terminal::init(cx);
                init(config, cx);
            });
            let (sender, commands) = mpsc::channel(32);
            let (window, view) = cx.update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| {
                        let terminal = cx.new(|cx| {
                            TerminalView::new(
                                "shortcut-session".into(),
                                sender,
                                &AppSettings::default(),
                                window,
                                cx,
                            )
                        });
                        terminal.update(cx, |terminal, cx| {
                            terminal.set_connected(true, cx);
                            terminal.focus_handle().focus(window, cx);
                        });
                        DispatchView {
                            terminal,
                            input: cx.new(|cx| InputState::new(window, cx)),
                            next: 0,
                            files: 0,
                            modal: false,
                        }
                    })
                })
                .unwrap()
            });
            Self {
                window: window.downcast().unwrap(),
                view,
                commands,
            }
        }

        fn update<R>(
            &self,
            cx: &mut TestAppContext,
            update: impl FnOnce(&mut DispatchView, &mut Window, &mut Context<DispatchView>) -> R,
        ) -> R {
            cx.update_window(self.window.into(), |_, window, cx| {
                self.view.update(cx, |view, cx| update(view, window, cx))
            })
            .unwrap()
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

        fn bytes(&mut self, cx: &mut TestAppContext) -> Vec<Vec<u8>> {
            cx.run_until_parked();
            let mut result = Vec::new();
            while let Ok(command) = self.commands.try_recv() {
                if let AppCommand::WriteInput { data, .. } = command {
                    result.push(data);
                }
            }
            result
        }
    }

    #[gpui::test]
    fn root_dispatches_defaults_without_stealing_input_or_terminal_control_keys(
        cx: &mut TestAppContext,
    ) {
        let mut fixture = Fixture::new(None, cx);
        fixture.press("ctrl-tab", cx);
        fixture.press(
            if cfg!(target_os = "macos") {
                "cmd-shift-f"
            } else {
                "ctrl-alt-shift-f"
            },
            cx,
        );
        fixture.update(cx, |view, _, _| {
            assert_eq!(view.next, 1);
            assert_eq!(view.files, 1);
        });
        fixture.press("ctrl-c", cx);
        fixture.press("tab", cx);
        fixture.press("shift-tab", cx);
        assert_eq!(fixture.bytes(cx), [vec![3], vec![9], b"\x1b[Z".to_vec()]);
        fixture.update(cx, |view, window, cx| {
            view.input.update(cx, |input, cx| {
                input.set_value("normal input text", window, cx)
            });
            view.input.focus_handle(cx).focus(window, cx);
        });
        fixture.press("secondary-a", cx);
        fixture.press("secondary-c", cx);
        fixture.update(cx, |view, _, cx| {
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some("normal input text")
            );
            assert_eq!(view.input.read(cx).value().as_str(), "normal input text");
        });
        fixture.press(
            if cfg!(target_os = "macos") {
                "cmd-shift-f"
            } else {
                "ctrl-alt-shift-f"
            },
            cx,
        );
        fixture.update(cx, |view, _, _| assert_eq!(view.files, 1));
        fixture.press("ctrl-tab", cx);
        fixture.update(cx, |view, _, _| assert_eq!(view.next, 2));
        assert!(fixture.bytes(cx).is_empty());
    }

    #[gpui::test]
    fn root_custom_bindings_remove_old_defaults_and_paste_only_at_new_key(cx: &mut TestAppContext) {
        let mut fixture = Fixture::new(
            Some("version = 1\n[bindings]\nnext_tab = ['f13']\nterminal_paste = ['f14']\n"),
            cx,
        );
        fixture.press("ctrl-tab", cx);
        fixture.update(cx, |view, _, _| assert_eq!(view.next, 0));
        fixture.press("f13", cx);
        fixture.update(cx, |view, _, cx| {
            assert_eq!(view.next, 1);
            cx.write_to_clipboard(ClipboardItem::new_string("safe clipboard text".into()));
        });
        fixture.bytes(cx);
        fixture.press(
            if cfg!(target_os = "macos") {
                "cmd-v"
            } else {
                "ctrl-shift-v"
            },
            cx,
        );
        let old_key_bytes = fixture.bytes(cx);
        assert!(old_key_bytes
            .iter()
            .all(|bytes| bytes != b"safe clipboard text"));
        fixture.press("f14", cx);
        assert_eq!(fixture.bytes(cx), [b"safe clipboard text".to_vec()]);
        fixture.update(cx, |view, window, cx| {
            view.input.focus_handle(cx).focus(window, cx)
        });
        fixture.press("f13", cx);
        fixture.update(cx, |view, _, _| assert_eq!(view.next, 2));
    }

    #[gpui::test]
    fn root_shifted_punctuation_dispatches_native_shaped_key_events(cx: &mut TestAppContext) {
        let source = if cfg!(target_os = "macos") {
            "cmd-shift-]"
        } else {
            "ctrl-shift-]"
        };
        let native = if cfg!(target_os = "macos") {
            "cmd-}"
        } else {
            "ctrl-}"
        };
        let mut fixture = Fixture::new(
            Some(&format!("version = 1\n[bindings]\nnext_tab = [{source:?}]")),
            cx,
        );
        fixture.press(native, cx);
        fixture.update(cx, |view, _, _| assert_eq!(view.next, 1));
        assert!(fixture.bytes(cx).is_empty());
        fixture.press("ctrl-tab", cx);
        fixture.update(cx, |view, _, _| assert_eq!(view.next, 1));
    }

    #[gpui::test]
    fn root_modal_marker_protects_workspace_and_terminal_actions_without_dialog_context(
        cx: &mut TestAppContext,
    ) {
        let mut fixture = Fixture::new(None, cx);
        fixture.update(cx, |view, _, cx| {
            view.modal = true;
            cx.write_to_clipboard(ClipboardItem::new_string("must not paste".into()));
            cx.notify();
        });
        fixture.press("ctrl-tab", cx);
        fixture.press(
            if cfg!(target_os = "macos") {
                "cmd-v"
            } else {
                "ctrl-shift-v"
            },
            cx,
        );
        fixture.update(cx, |view, _, _| assert_eq!(view.next, 0));
        assert!(fixture
            .bytes(cx)
            .iter()
            .all(|bytes| bytes != b"must not paste"));
    }

    #[gpui::test]
    fn root_disabled_bindings_have_no_raw_key_fallback(cx: &mut TestAppContext) {
        let mut fixture = Fixture::new(Some("version = 1\n[bindings]\nnext_tab = []\nterminal_copy = []\nterminal_paste = []\nterminal_select_all = []\n"), cx);
        fixture.update(cx, |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string("must not paste".into()))
        });
        fixture.press("ctrl-tab", cx);
        fixture.press(
            if cfg!(target_os = "macos") {
                "cmd-v"
            } else {
                "ctrl-shift-v"
            },
            cx,
        );
        fixture.update(cx, |view, _, cx| {
            assert_eq!(view.next, 0);
            assert!(hint("terminal_paste", cx).is_empty());
        });
        assert!(fixture
            .bytes(cx)
            .iter()
            .all(|bytes| bytes != b"must not paste"));
    }
}
