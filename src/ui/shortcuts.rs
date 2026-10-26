use std::time::Duration;

use gpui_kit::component::{
    button::{Button, ButtonVariants},
    scroll::ScrollableElement,
    ActiveTheme, Sizable,
};
use gpui_kit::{prelude::*, *};

use crate::shortcuts::{self, CATALOG};

pub struct ShortcutsView {
    focus: FocusHandle,
}

impl ShortcutsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
        }
    }
}

impl Focusable for ShortcutsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[derive(Default)]
struct CopyPathState {
    copied: bool,
    reset: Option<Task<()>>,
}

pub fn configuration_info(window: &mut Window, cx: &mut App) -> impl IntoElement {
    let path = shortcuts::effective(cx).map(|keymap| keymap.path.display().to_string());
    let state = window.use_keyed_state("shortcuts-copy-path-state", cx, |_, _| {
        CopyPathState::default()
    });
    let copied = state.read(cx).copied;
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded_md()
        .bg(cx.theme().muted.opacity(0.4))
        .min_w_0()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child("快捷键配置"),
                )
                .when_some(path.clone(), |view, path| {
                    view.child(
                        Button::new("shortcuts-copy-path")
                            .debug_selector(move || {
                                if copied {
                                    "shortcuts-copy-path-copied"
                                } else {
                                    "shortcuts-copy-path-button"
                                }
                                .into()
                            })
                            .ghost()
                            .xsmall()
                            .w(px(88.))
                            .flex_shrink_0()
                            .label(if copied { "已复制" } else { "复制路径" })
                            .accessibility_label(if copied {
                                "已复制快捷键配置文件的绝对路径，点击再次复制"
                            } else {
                                "复制快捷键配置文件的绝对路径"
                            })
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
                                state.update(cx, |state, cx| {
                                    state.copied = true;
                                    state.reset = Some(cx.spawn(async move |state, cx| {
                                        cx.background_executor()
                                            .timer(Duration::from_secs(2))
                                            .await;
                                        let _ = state.update(cx, |state, cx| {
                                            state.copied = false;
                                            state.reset = None;
                                            cx.notify();
                                        });
                                    }));
                                    cx.notify();
                                });
                            }),
                    )
                }),
        )
        .when_some(path, |view, path| {
            view.child(
                div()
                    .min_w_0()
                    .whitespace_normal()
                    .text_xs()
                    .font_family("Menlo")
                    .child(path),
            )
        })
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("编辑 keybindings.toml 后重启生效；[] 禁用该动作，省略则使用默认值。"),
        )
}

impl Render for ShortcutsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("ShortcutsHelp")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap_3()
            .min_h_0()
            .w_full()
            .child(configuration_info(window, cx))
            .when_some(shortcuts::warning(cx).map(str::to_owned), |view, warning| {
                view.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(warning),
                )
            })
            .child(
                div()
                    .id("shortcuts-list")
                    .debug_selector(|| "shortcuts-list".into())
                    .flex()
                    .flex_col()
                    .h((window.viewport_size().height * 0.6 - px(100.)).max(px(160.)))
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .children(CATALOG.iter().map(|action| {
                        let hint = shortcuts::hint(action.id, cx);
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .py_2()
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.5))
                            .text_sm()
                            .child(div().flex_1().min_w_0().child(action.label))
                            .child(
                                div()
                                    .w(px(154.))
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(action.scope_label()),
                            )
                            .child(
                                div()
                                    .w(px(160.))
                                    .flex_shrink_0()
                                    .min_w_0()
                                    .font_family("Menlo")
                                    .text_xs()
                                    .whitespace_normal()
                                    .child(if hint.is_empty() { "已禁用".into() } else { hint }),
                            )
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("此列表是本次运行的有效按键。弹窗中暂停应用动作；终端保留 Tab、Shift-Tab 和 Ctrl-C。"),
            )
    }
}
