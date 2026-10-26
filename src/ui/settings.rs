use gpui_kit::component::{
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, InputState},
    radio::Radio,
    scroll::ScrollableElement,
    ActiveTheme, Disableable,
};
use gpui_kit::{prelude::*, *};

use crate::config::{AppSettings, Theme, BUILTIN_THEMES};
use crate::ui::connection::{field, row};

#[derive(Clone)]
pub enum SettingsAction {
    Save(AppSettings),
    Cancel,
}

pub struct SettingsView {
    settings: AppSettings,
    font: Entity<InputState>,
    font_size: Entity<InputState>,
    timeout: Entity<InputState>,
    keepalive: Entity<InputState>,
    keepalive_max: Entity<InputState>,
    reconnect: Entity<InputState>,
    pub error: Option<String>,
    pub busy: bool,
}

impl SettingsView {
    pub fn new(settings: AppSettings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            font: field(&settings.app.font_family, "Menlo", false, window, cx),
            font_size: field(&settings.app.font_size.to_string(), "14", false, window, cx),
            timeout: field(
                &settings.ssh.connection_timeout_secs.to_string(),
                "30",
                false,
                window,
                cx,
            ),
            keepalive: field(
                &settings.ssh.keepalive_interval_secs.to_string(),
                "30",
                false,
                window,
                cx,
            ),
            keepalive_max: field(
                &settings.ssh.keepalive_max_count.to_string(),
                "3",
                false,
                window,
                cx,
            ),
            reconnect: field(
                &settings.ssh.reconnect_max_attempts.to_string(),
                "3",
                false,
                window,
                cx,
            ),
            settings,
            error: None,
            busy: false,
        }
    }

    fn select_theme(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.settings.app.theme = id.into();
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let result = (|| -> Result<AppSettings, &'static str> {
            let mut settings = self.settings.clone();
            let font = self.font.read(cx).value().trim().to_owned();
            if font.is_empty() {
                return Err("请填写终端字体名称");
            }
            let size = self
                .font_size
                .read(cx)
                .value()
                .parse::<f32>()
                .map_err(|_| "字号必须是数字")?;
            if !size.is_finite() || !(8.0..=36.0).contains(&size) {
                return Err("字号范围为 8–36");
            }
            let timeout = self
                .timeout
                .read(cx)
                .value()
                .parse::<u64>()
                .map_err(|_| "连接超时必须是整数")?;
            if !(1..=300).contains(&timeout) {
                return Err("连接超时范围为 1–300 秒");
            }
            let interval = self
                .keepalive
                .read(cx)
                .value()
                .parse::<u64>()
                .map_err(|_| "保活间隔必须是整数")?;
            if !(1..=3600).contains(&interval) {
                return Err("保活间隔范围为 1–3600 秒");
            }
            let max = self
                .keepalive_max
                .read(cx)
                .value()
                .parse::<u32>()
                .map_err(|_| "保活次数必须是整数")?;
            if !(1..=30).contains(&max) {
                return Err("保活失败次数范围为 1–30");
            }
            let retries = self
                .reconnect
                .read(cx)
                .value()
                .parse::<u32>()
                .map_err(|_| "重连次数必须是整数")?;
            if retries > 10 {
                return Err("自动重连次数范围为 0–10，0 表示关闭");
            }
            settings.app.font_family = font;
            settings.app.font_size = size;
            settings.ssh.connection_timeout_secs = timeout;
            settings.ssh.keepalive_interval_secs = interval;
            settings.ssh.keepalive_max_count = max;
            settings.ssh.reconnect_max_attempts = retries;
            Ok(settings)
        })();
        match result {
            Ok(settings) => {
                self.busy = true;
                self.error = None;
                cx.emit(SettingsAction::Save(settings));
            }
            Err(error) => self.error = Some(error.into()),
        }
        cx.notify();
    }

    fn theme_selector(&self, cx: &Context<Self>) -> impl IntoElement {
        let selected = Theme::for_name(&self.settings.app.theme).name;
        div()
            .debug_selector(|| "settings-theme-selector".into())
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child("外观主题"),
            )
            .children(BUILTIN_THEMES.chunks(2).map(|presets| {
                div().flex().gap_2().children(presets.iter().map(|preset| {
                    let id = preset.id;
                    let theme = Theme::for_name(id);
                    let checked = selected == id;
                    let foreground = rgb_color(theme.terminal.fg);
                    let muted = rgb_color(theme.ui.muted_foreground);
                    Radio::new(SharedString::from(format!("settings-theme-{id}")))
                        .accessibility_label(format!(
                            "{}，{}主题",
                            preset.label,
                            if preset.is_light { "浅色" } else { "深色" }
                        ))
                        .checked(checked)
                        .disabled(self.busy)
                        .flex_1()
                        .min_w_0()
                        .p_2()
                        .border_1()
                        .border_color(if checked {
                            rgb_color(theme.ui.accent)
                        } else {
                            rgb_color(theme.ui.border)
                        })
                        .bg(rgb_color(theme.terminal.bg))
                        .text_color(foreground)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(preset.label),
                                )
                                .when(checked, |this| {
                                    this.child(div().text_xs().text_color(muted).child("已选"))
                                }),
                        )
                        .child(div().text_xs().text_color(muted).child(if preset.is_light {
                            "浅色"
                        } else {
                            "深色"
                        }))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .mt_1()
                                .text_xs()
                                .font_family("Menlo")
                                .child(
                                    div()
                                        .text_color(rgb_color(theme.terminal.ansi[2]))
                                        .child("❯"),
                                )
                                .child(
                                    div()
                                        .text_color(rgb_color(theme.terminal.ansi[4]))
                                        .child("src/"),
                                )
                                .child(div().text_color(foreground).child("README")),
                        )
                        .child(div().flex().gap_1().mt_1().children(
                            [1, 2, 3, 4, 5, 6].into_iter().map(|index| {
                                div()
                                    .w_4()
                                    .h_1()
                                    .rounded_sm()
                                    .bg(rgb_color(theme.terminal.ansi[index]))
                            }),
                        ))
                        .on_change(cx.listener(move |this, _, _, cx| this.select_theme(id, cx)))
                }))
            }))
    }
}

fn rgb_color([r, g, b]: [u8; 3]) -> Hsla {
    rgb(u32::from_be_bytes([0, r, g, b])).into()
}

impl EventEmitter<SettingsAction> for SettingsView {}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = div()
            .id("settings-scroll")
            .debug_selector(|| "settings-scroll-content".into())
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .min_h_0()
            .h((window.viewport_size().height * 0.70 - px(110.)).max(px(120.)))
            .pr_1()
            .child(self.theme_selector(cx))
            .child(crate::ui::shortcuts::configuration_info(window, cx))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "快捷键帮助 {} 可查看本次运行的有效按键。",
                        crate::shortcuts::hint("show_shortcuts", cx)
                    )),
            )
            .child(row("终端字体", Input::new(&self.font).id("settings-font")))
            .child(row(
                "终端字号",
                Input::new(&self.font_size).id("settings-font-size"),
            ))
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(
                        row(
                            "连接超时（秒）",
                            Input::new(&self.timeout).id("settings-timeout"),
                        )
                        .debug_selector(|| "settings-timeout-field".into())
                        .flex_1()
                        .min_w_0(),
                    )
                    .child(
                        row(
                            "保活间隔（秒）",
                            Input::new(&self.keepalive).id("settings-keepalive"),
                        )
                        .debug_selector(|| "settings-keepalive-field".into())
                        .flex_1()
                        .min_w_0(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(
                        row(
                            "保活失败次数",
                            Input::new(&self.keepalive_max).id("settings-keepalive-max"),
                        )
                        .debug_selector(|| "settings-keepalive-max-field".into())
                        .flex_1()
                        .min_w_0(),
                    )
                    .child(
                        row(
                            "自动重连次数",
                            Input::new(&self.reconnect).id("settings-reconnect"),
                        )
                        .debug_selector(|| "settings-reconnect-field".into())
                        .flex_1()
                        .min_w_0(),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("字体和主题保存后立即应用；连接参数用于新连接。"),
            )
            .child(
                Checkbox::new("settings-hidden")
                    .label("SFTP 显示隐藏文件")
                    .checked(self.settings.sftp.show_hidden_files)
                    .disabled(self.busy)
                    .on_change(cx.listener(|this, checked, _, cx| {
                        this.settings.sftp.show_hidden_files = *checked;
                        cx.notify();
                    })),
            )
            .child(
                Checkbox::new("settings-overwrite")
                    .label("覆盖文件前确认")
                    .checked(self.settings.sftp.confirm_overwrite)
                    .disabled(self.busy)
                    .on_change(cx.listener(|this, checked, _, cx| {
                        this.settings.sftp.confirm_overwrite = *checked;
                        cx.notify();
                    })),
            )
            .when_some(self.error.clone(), |view, error| {
                view.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .overflow_y_scrollbar();
        div().flex().flex_col().gap_3().w_full().child(body).child(
            div()
                .debug_selector(|| "settings-footer".into())
                .flex()
                .flex_shrink_0()
                .justify_end()
                .gap_2()
                .pt_2()
                .border_t_1()
                .border_color(cx.theme().border)
                .child(
                    Button::new("settings-cancel")
                        .ghost()
                        .label("取消")
                        .disabled(self.busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsAction::Cancel))),
                )
                .child(
                    Button::new("settings-save")
                        .primary()
                        .label(if self.busy {
                            "保存中…"
                        } else {
                            "保存设置"
                        })
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{SettingsAction, SettingsView};
    use crate::config::AppSettings;
    use gpui_kit::{
        AppContext, Context, Entity, Subscription, TestAppContext, Window, WindowHandle,
    };
    use std::{cell::RefCell, rc::Rc};

    struct Fixture {
        window: WindowHandle<gpui_kit::base::Root>,
        form: Entity<SettingsView>,
        actions: Rc<RefCell<Vec<SettingsAction>>>,
        _subscription: Subscription,
    }

    impl Fixture {
        fn new(cx: &mut TestAppContext) -> Self {
            cx.update(gpui_kit::init);
            let (window, form) = cx.update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| SettingsView::new(AppSettings::default(), window, cx))
                })
                .expect("settings window")
            });
            let actions = Rc::new(RefCell::new(Vec::new()));
            let collected = actions.clone();
            let subscription = cx.update(|cx| {
                cx.subscribe(&form, move |_, action: &SettingsAction, _| {
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
            update: impl FnOnce(&mut SettingsView, &mut Window, &mut Context<SettingsView>) -> R,
        ) -> R {
            cx.update_window(self.window.into(), |_, window, cx| {
                self.form.update(cx, |form, cx| update(form, window, cx))
            })
            .expect("update settings")
        }
    }

    #[gpui::test]
    fn paired_numeric_fields_share_usable_width_without_overflow(cx: &mut TestAppContext) {
        use gpui_kit::{px, size, VisualTestContext};

        let fixture = Fixture::new(cx);
        let mut visual = VisualTestContext::from_window(fixture.window.into(), cx);
        for viewport in [size(px(1280.), px(820.)), size(px(860.), px(600.))] {
            visual.simulate_resize(viewport);
            visual.update(|window, cx| window.draw(cx).clear(cx));
            for (left, right) in [
                ("settings-timeout-field", "settings-keepalive-field"),
                ("settings-keepalive-max-field", "settings-reconnect-field"),
            ] {
                let left = visual.debug_bounds(left).unwrap();
                let right = visual.debug_bounds(right).unwrap();
                assert!(left.size.width >= px(180.) && right.size.width >= px(180.));
                assert!((left.size.width - right.size.width).abs() <= px(1.));
                assert!(left.origin.x >= px(0.));
                assert!(right.origin.x >= left.origin.x + left.size.width);
                assert!(right.origin.x + right.size.width <= viewport.width);
            }
        }
    }

    #[gpui::test]
    fn theme_selection_is_a_draft_until_save(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx);
        let original = cx.update(|cx| gpui_kit::component::Theme::global(cx).colors.background);
        fixture.update(cx, |form, _, cx| {
            form.select_theme("catppuccin-mocha", cx);
            assert_eq!(form.settings.app.theme, "catppuccin-mocha");
            assert!(!form.busy);
            assert_eq!(
                gpui_kit::component::Theme::global(cx).colors.background,
                original
            );
        });
        cx.run_until_parked();
        assert!(fixture.actions.borrow().is_empty());
        fixture.update(cx, |_, _, cx| cx.emit(SettingsAction::Cancel));
        cx.run_until_parked();
        assert!(matches!(
            fixture.actions.borrow().as_slice(),
            [SettingsAction::Cancel]
        ));
        assert_eq!(
            cx.update(|cx| gpui_kit::component::Theme::global(cx).colors.background),
            original
        );
    }

    #[gpui::test]
    fn save_emits_selected_theme_without_applying_it(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx);
        let original = cx.update(|cx| gpui_kit::component::Theme::global(cx).colors.background);
        fixture.update(cx, |form, _, cx| {
            form.select_theme("nord", cx);
            form.save(cx);
            assert!(form.busy);
            form.select_theme("light", cx);
            form.save(cx);
            assert_eq!(form.settings.app.theme, "nord");
            assert_eq!(
                gpui_kit::component::Theme::global(cx).colors.background,
                original
            );
        });
        cx.run_until_parked();
        let actions = fixture.actions.borrow();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            SettingsAction::Save(settings) => assert_eq!(settings.app.theme, "nord"),
            SettingsAction::Cancel => panic!("expected save"),
        }
    }

    #[gpui::test]
    fn invalid_settings_keep_theme_draft_without_save(cx: &mut TestAppContext) {
        let fixture = Fixture::new(cx);
        fixture.update(cx, |form, window, cx| {
            form.select_theme("light", cx);
            form.font_size
                .update(cx, |input, cx| input.set_value("invalid", window, cx));
            form.save(cx);
            assert!(!form.busy);
            assert_eq!(form.error.as_deref(), Some("字号必须是数字"));
            assert_eq!(form.settings.app.theme, "light");
        });
        cx.run_until_parked();
        assert!(fixture.actions.borrow().is_empty());
    }
}
