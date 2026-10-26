use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use alacritty_terminal::event::{Event, WindowSize};
use alacritty_terminal::vte::ansi::Rgb;
use gpui_kit::{
    div, fill, outline, point, px, relative, rgb, rgba, size, App, BorderStyle, Bounds,
    ClipboardItem, ContentMask, Context, CursorStyle, DispatchPhase, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, Font, GlobalElementId,
    Hsla, InspectorElementId, InteractiveElement, IntoElement, KeyDownEvent, Keystroke, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point,
    Render, ScrollDelta, ScrollWheelEvent, ShapedLine, StrikethroughStyle, Style, Styled,
    Subscription, Task, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window,
};
use tokio::sync::mpsc;

use crate::backend::AppCommand;
use crate::config::{AppSettings, Theme};
use crate::fonts::terminal_font;
use crate::shortcuts::{TerminalCopy, TerminalPaste, TerminalSelectAll};
use crate::terminal_v2::{
    CellFlags, CursorKind, PackedColor, Terminal, TerminalModes, TerminalSnapshot,
};

const PADDING: f32 = 8.;

pub fn init(cx: &mut App) {
    // Root actions run before key listeners, so terminal Tab must be unbound here.
    cx.bind_keys([
        gpui_kit::KeyBinding::new("tab", gpui_kit::NoAction, Some("Terminal")),
        gpui_kit::KeyBinding::new("shift-tab", gpui_kit::NoAction, Some("Terminal")),
    ]);
}

pub struct TerminalView {
    session_id: String,
    terminal: Terminal,
    snapshot: Arc<TerminalSnapshot>,
    command_tx: mpsc::UnboundedSender<AppCommand>,
    _command_task: Task<()>,
    focus_handle: FocusHandle,
    _focus_subscriptions: Vec<Subscription>,
    connected: bool,
    shell_ready: bool,
    focused: bool,
    font: Font,
    font_family: String,
    font_dirty: bool,
    font_size: Pixels,
    cell_width: Pixels,
    cell_height: Pixels,
    theme: Theme,
    bounds: Option<Bounds<Pixels>>,
    sent_size: Option<(u16, u16)>,
    selecting: bool,
    scroll_remainder: f32,
    copy_on_select: bool,
    right_click_paste: bool,
    marked_text: String,
    marked_selection: Range<usize>,
    cursor_on: bool,
    blink_task: Option<Task<()>>,
}

impl TerminalView {
    pub fn new(
        session_id: String,
        command_tx: mpsc::Sender<AppCommand>,
        settings: &AppSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (queue_tx, mut queue_rx) = mpsc::unbounded_channel();
        let command_task = cx.spawn(async move |this, cx| {
            while let Some(command) = queue_rx.recv().await {
                if command_tx.send(command).await.is_err() {
                    let _ = this.update(cx, |this, cx| this.set_connected(false, cx));
                    break;
                }
            }
        });
        let focus_handle = cx.focus_handle();
        let focus_subscriptions = vec![
            cx.on_focus(&focus_handle, window, |this, _, cx| {
                this.focused = true;
                if this.terminal.modes().focus_reporting {
                    this.send_input(b"\x1b[I".to_vec(), false, cx);
                }
                this.reset_blink(cx);
                cx.notify();
            }),
            cx.on_blur(&focus_handle, window, |this, _, cx| {
                if this.terminal.modes().focus_reporting {
                    this.send_input(b"\x1b[O".to_vec(), false, cx);
                }
                this.focused = false;
                this.selecting = false;
                this.marked_text.clear();
                this.marked_selection = 0..0;
                this.blink_task = None;
                this.cursor_on = true;
                cx.notify();
            }),
        ];
        let mut terminal = Terminal::with_history(80, 24, settings.terminal.scroll_buffer_lines);
        terminal.configure(
            settings.terminal.scroll_buffer_lines,
            cursor_kind(&settings.terminal.cursor_style),
            settings.terminal.cursor_blink,
        );
        let font = terminal_font(&settings.app.font_family, window);
        let font_size = px(settings.app.font_size.clamp(8., 48.));
        let cell_width = measure_cell_width(&font, font_size, window);
        let snapshot = Arc::new(terminal.snapshot());
        Self {
            session_id,
            terminal,
            snapshot,
            command_tx: queue_tx,
            _command_task: command_task,
            focus_handle,
            _focus_subscriptions: focus_subscriptions,
            connected: false,
            shell_ready: false,
            focused: false,
            font,
            font_family: settings.app.font_family.clone(),
            font_dirty: false,
            font_size,
            cell_width,
            cell_height: px((f32::from(font_size) * 1.45).ceil()),
            theme: Theme::for_name(&settings.app.theme),
            bounds: None,
            sent_size: None,
            selecting: false,
            scroll_remainder: 0.,
            copy_on_select: settings.terminal.copy_on_select,
            right_click_paste: settings.terminal.right_click_paste,
            marked_text: String::new(),
            marked_selection: 0..0,
            cursor_on: true,
            blink_task: None,
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub fn title(&self) -> &str {
        self.terminal.title()
    }

    pub fn has_marked_text(&self) -> bool {
        !self.marked_text.is_empty()
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    pub fn set_shell_ready(&mut self, ready: bool, cx: &mut Context<Self>) {
        self.shell_ready = ready && self.connected;
        cx.notify();
    }

    pub fn insert_history(
        &mut self,
        text: &str,
        expected_session_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        validate_history_text(text)?;
        if expected_session_id != self.session_id {
            return Err("终端会话已改变，请重新读取历史".into());
        }
        if !self.connected || !self.shell_ready {
            return Err("终端 shell 尚未就绪".into());
        }
        if self.has_marked_text() {
            return Err("请先完成输入法组合输入".into());
        }
        if self.snapshot.modes.alternate_screen {
            return Err("请先退出全屏程序，返回 shell 提示符".into());
        }
        if self.command_tx.is_closed() {
            self.set_connected(false, cx);
            return Err("终端输入通道已关闭".into());
        }
        let data = self.terminal.paste_bytes(text);
        self.send_input(data, true, cx);
        if !self.connected {
            return Err("终端输入通道已关闭".into());
        }
        self.focus_handle.focus(window, cx);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn theme_for_test(&self) -> &Theme {
        &self.theme
    }

    #[cfg(test)]
    pub(crate) fn snapshot_for_test(&self) -> &TerminalSnapshot {
        &self.snapshot
    }

    pub fn process_output(&mut self, data: &[u8], cx: &mut Context<Self>) {
        self.terminal.process_output(data);
        self.refresh_snapshot();
        for event in self.terminal.drain_events() {
            let reply = match event {
                Event::PtyWrite(text) => Some(text),
                Event::ColorRequest(index, format) => {
                    let [r, g, b] = palette_color(index, &self.snapshot, &self.theme);
                    Some(format(Rgb { r, g, b }))
                }
                Event::TextAreaSizeRequest(format) => Some(format(WindowSize {
                    num_lines: self.snapshot.rows as u16,
                    num_cols: self.snapshot.cols as u16,
                    cell_width: f32::from(self.cell_width).round() as u16,
                    cell_height: f32::from(self.cell_height).round() as u16,
                })),
                _ => None,
            };
            if let Some(reply) = reply {
                self.enqueue(
                    AppCommand::WriteInput {
                        session_id: self.session_id.clone(),
                        data: reply.into_bytes(),
                    },
                    cx,
                );
            }
        }
        self.reset_blink(cx);
        cx.notify();
    }

    pub fn set_connected(&mut self, connected: bool, cx: &mut Context<Self>) {
        if !connected {
            self.shell_ready = false;
        }
        if self.connected == connected {
            return;
        }
        self.connected = connected;
        self.sent_size = None;
        if connected {
            self.send_resize(cx);
            self.reset_blink(cx);
        } else {
            self.marked_text.clear();
            self.marked_selection = 0..0;
            self.blink_task = None;
            self.cursor_on = true;
        }
        cx.notify();
    }

    pub fn apply_settings(&mut self, settings: &AppSettings, cx: &mut Context<Self>) {
        self.theme = Theme::for_name(&settings.app.theme);
        self.font_family = settings.app.font_family.clone();
        self.font_dirty = true;
        self.font_size = px(settings.app.font_size.clamp(8., 48.));
        self.cell_height = px((f32::from(self.font_size) * 1.45).ceil());
        self.copy_on_select = settings.terminal.copy_on_select;
        self.right_click_paste = settings.terminal.right_click_paste;
        self.terminal.configure(
            settings.terminal.scroll_buffer_lines,
            cursor_kind(&settings.terminal.cursor_style),
            settings.terminal.cursor_blink,
        );
        self.refresh_snapshot();
        self.reset_blink(cx);
        cx.notify();
    }

    fn enqueue(&mut self, command: AppCommand, cx: &mut Context<Self>) {
        if self.command_tx.send(command).is_err() {
            self.set_connected(false, cx);
        }
    }

    fn send_resize(&mut self, cx: &mut Context<Self>) {
        let size = (self.snapshot.cols as u16, self.snapshot.rows as u16);
        if self.connected && self.sent_size != Some(size) {
            self.sent_size = Some(size);
            self.enqueue(
                AppCommand::ResizeSession {
                    session_id: self.session_id.clone(),
                    cols: size.0,
                    rows: size.1,
                },
                cx,
            );
        }
    }

    fn sync_bounds(&mut self, bounds: Bounds<Pixels>, window: &Window, cx: &mut Context<Self>) {
        self.bounds = Some(bounds);
        if self.font_dirty {
            self.font = terminal_font(&self.font_family, window);
            self.font_dirty = false;
        }
        self.cell_width = measure_cell_width(&self.font, self.font_size, window);
        let cols = ((f32::from(bounds.size.width) - 2. * PADDING) / f32::from(self.cell_width))
            .floor()
            .clamp(2., u16::MAX as f32) as usize;
        let rows = ((f32::from(bounds.size.height) - 2. * PADDING) / f32::from(self.cell_height))
            .floor()
            .clamp(1., u16::MAX as f32) as usize;
        if (cols, rows) != self.terminal.size() {
            self.terminal.resize(cols, rows);
            self.refresh_snapshot();
        }
        self.send_resize(cx);
    }

    fn refresh_snapshot(&mut self) {
        self.snapshot = Arc::new(self.terminal.snapshot());
    }

    fn send_input(&mut self, data: Vec<u8>, user_input: bool, cx: &mut Context<Self>) {
        if !self.connected || data.is_empty() {
            return;
        }
        if user_input {
            self.marked_text.clear();
            self.marked_selection = 0..0;
            self.terminal.clear_selection();
            self.terminal.scroll_to_bottom();
            self.refresh_snapshot();
        }
        self.enqueue(
            AppCommand::WriteInput {
                session_id: self.session_id.clone(),
                data,
            },
            cx,
        );
        self.reset_blink(cx);
        cx.notify();
    }

    fn reset_blink(&mut self, cx: &mut Context<Self>) {
        self.cursor_on = true;
        self.blink_task = None;
        if self.focused && self.connected && self.snapshot.cursor.blinking {
            self.blink_task = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(600))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.cursor_on = !this.cursor_on;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }));
        }
    }

    fn copy_selection(&self, cx: &mut Context<Self>) {
        if let Some(text) = self.terminal.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        if self.has_marked_text() {
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.send_input(self.terminal.paste_bytes(&text), true, cx);
        }
    }

    fn on_copy(&mut self, _: &TerminalCopy, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_focused(window) && !self.has_marked_text() {
            self.copy_selection(cx);
        }
    }

    fn on_paste(&mut self, _: &TerminalPaste, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_focused(window) && !self.has_marked_text() {
            self.paste_clipboard(cx);
        }
    }

    fn on_select_all(
        &mut self,
        _: &TerminalSelectAll,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_focused(window) && !self.has_marked_text() {
            self.terminal.select_all();
            self.refresh_snapshot();
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_handle.is_focused(window) {
            return;
        }
        let key = &event.keystroke;
        if !self.marked_text.is_empty() && !key.modifiers.control {
            return;
        }
        if let Some(data) = encode_keystroke(key, self.snapshot.modes) {
            self.send_input(data, true, cx);
            cx.stop_propagation();
        }
    }

    fn position_to_cell(&self, position: Point<Pixels>) -> Option<(usize, usize, bool)> {
        let bounds = self.bounds?;
        let x = f32::from(position.x - bounds.left()) - PADDING;
        let y = f32::from(position.y - bounds.top()) - PADDING;
        let col = (x / f32::from(self.cell_width)).floor().max(0.) as usize;
        let row = (y / f32::from(self.cell_height)).floor().max(0.) as usize;
        let right_half = (x / f32::from(self.cell_width)).fract() >= 0.5;
        Some((
            row.min(self.snapshot.rows - 1),
            col.min(self.snapshot.cols - 1),
            right_half,
        ))
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window, cx);
        if event.button == MouseButton::Right {
            if self.right_click_paste {
                self.paste_clipboard(cx);
            }
            cx.stop_propagation();
            return;
        }
        if let Some((row, col, right_half)) = self.position_to_cell(event.position) {
            self.terminal
                .start_selection(row, col, right_half, event.click_count);
            self.selecting = true;
            self.refresh_snapshot();
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting && event.dragging() {
            if let Some(bounds) = self.bounds {
                if event.position.y < bounds.top() {
                    self.terminal.scroll(1);
                } else if event.position.y > bounds.bottom() {
                    self.terminal.scroll(-1);
                }
            }
            if let Some((row, col, right_half)) = self.position_to_cell(event.position) {
                self.terminal.update_selection(row, col, right_half);
                self.refresh_snapshot();
                cx.notify();
            }
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting && self.copy_on_select {
            self.copy_selection(cx);
        }
        self.selecting = false;
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = match event.delta {
            ScrollDelta::Pixels(delta) => f32::from(delta.y) / f32::from(self.cell_height),
            ScrollDelta::Lines(delta) => delta.y,
        };
        self.scroll_remainder += delta;
        let lines = self.scroll_remainder.trunc() as i32;
        self.scroll_remainder -= lines as f32;
        if lines != 0 {
            if self.snapshot.modes.alternate_screen && self.snapshot.modes.application_cursor {
                if !self.focus_handle.is_focused(window) {
                    return;
                }
                let key = if lines > 0 { b"\x1bOA" } else { b"\x1bOB" };
                self.send_input(
                    key.repeat(lines.unsigned_abs().min(100) as usize),
                    false,
                    cx,
                );
            } else {
                self.terminal.scroll(lines);
                self.refresh_snapshot();
                cx.notify();
            }
        }
        cx.stop_propagation();
    }

    fn cursor_bounds(&self) -> Option<Bounds<Pixels>> {
        let bounds = self.bounds?;
        Some(Bounds::new(
            point(
                bounds.left() + px(PADDING) + self.cell_width * self.snapshot.cursor.col as f32,
                bounds.top() + px(PADDING) + self.cell_height * self.snapshot.cursor.row as f32,
            ),
            size(self.cell_width, self.cell_height),
        ))
    }

    fn composition_line(&self, window: &Window) -> ShapedLine {
        let text = self.marked_text.clone();
        window.text_system().shape_line(
            text.clone().into(),
            self.font_size,
            &[TextRun {
                len: text.len(),
                font: self.font.clone(),
                color: color(self.theme.terminal.fg),
                underline: Some(UnderlineStyle {
                    thickness: px(1.),
                    color: None,
                    wavy: false,
                }),
                ..Default::default()
            }],
            None,
        )
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::on_copy))
            .on_action(cx.listener(Self::on_paste))
            .on_action(cx.listener(Self::on_select_all))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(TerminalSurface { view: cx.entity() })
    }
}

struct TerminalSurface {
    view: Entity<TerminalView>,
}

struct PositionedText {
    line: ShapedLine,
    origin: Point<Pixels>,
    bounds: Bounds<Pixels>,
}

struct TerminalPaint {
    background: Hsla,
    backgrounds: Vec<gpui_kit::PaintQuad>,
    text: Vec<PositionedText>,
    cursor: Option<gpui_kit::PaintQuad>,
    composition: Option<PositionedText>,
    line_height: Pixels,
}

impl IntoElement for TerminalSurface {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for TerminalSurface {
    type RequestLayoutState = ();
    type PrepaintState = TerminalPaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> TerminalPaint {
        self.view.update(cx, |this, cx| {
            let focused = this.focus_handle.is_focused(window);
            if focused != this.focused {
                this.focused = focused;
                this.reset_blink(cx);
            }
            this.sync_bounds(bounds, window, cx);
        });
        let view = self.view.read(cx);
        let snapshot = &view.snapshot;
        let origin = point(bounds.left() + px(PADDING), bounds.top() + px(PADDING));
        let default_bg = color(palette_color(257, snapshot, &view.theme));
        let cursor_color = color(palette_color(258, snapshot, &view.theme));
        let selection_color: Hsla = rgba(u32::from_be_bytes(view.theme.ui.selection_bg)).into();
        let cursor_visible = view.connected
            && snapshot.cursor.visible
            && view.marked_text.is_empty()
            && (!view.focused || view.cursor_on);
        let mut backgrounds = Vec::new();
        let mut text = Vec::new();
        for row in 0..snapshot.rows {
            for col in 0..snapshot.cols {
                let cell = snapshot.cell(row, col);
                let cell_bounds = Bounds::new(
                    point(
                        origin.x + view.cell_width * col as f32,
                        origin.y + view.cell_height * row as f32,
                    ),
                    size(view.cell_width, view.cell_height),
                );
                let bold =
                    cell.flags.contains(CellFlags::BOLD) && !cell.flags.contains(CellFlags::DIM);
                let mut foreground = resolve_color(cell.fg, bold, snapshot, &view.theme);
                let mut background = resolve_color(cell.bg, false, snapshot, &view.theme);
                if cell.flags.contains(CellFlags::REVERSE) {
                    std::mem::swap(&mut foreground, &mut background);
                }
                if background != default_bg {
                    backgrounds.push(fill(cell_bounds, background));
                }
                if cell.selected {
                    backgrounds.push(fill(cell_bounds, selection_color));
                }
                let block_cursor = cursor_visible
                    && view.focused
                    && snapshot.cursor.kind == CursorKind::Block
                    && snapshot.cursor.row == row
                    && snapshot.cursor.col == col;
                if block_cursor {
                    foreground = default_bg;
                } else if cell.flags.contains(CellFlags::DIM) {
                    foreground = foreground.opacity(0.6);
                }
                if cell.is_wide_spacer || cell.flags.contains(CellFlags::HIDDEN) {
                    continue;
                }
                if cell.text == " "
                    && !cell
                        .flags
                        .intersects(CellFlags::UNDERLINE | CellFlags::STRIKETHROUGH)
                {
                    continue;
                }
                let mut font = view.font.clone();
                if cell.flags.contains(CellFlags::BOLD) {
                    font = font.bold();
                }
                if cell.flags.contains(CellFlags::ITALIC) {
                    font = font.italic();
                }
                let run = TextRun {
                    len: cell.text.len(),
                    font,
                    color: foreground,
                    background_color: None,
                    underline: cell.flags.contains(CellFlags::UNDERLINE).then_some(
                        UnderlineStyle {
                            thickness: px(1.),
                            color: Some(foreground),
                            wavy: cell.flags.contains(CellFlags::UNDERCURL),
                        },
                    ),
                    strikethrough: cell.flags.contains(CellFlags::STRIKETHROUGH).then_some(
                        StrikethroughStyle {
                            thickness: px(1.),
                            color: Some(foreground),
                        },
                    ),
                };
                let line = window.text_system().shape_line(
                    cell.text.clone().into(),
                    view.font_size,
                    &[run],
                    None,
                );
                text.push(PositionedText {
                    line,
                    origin: cell_bounds.origin,
                    bounds: Bounds::new(
                        cell_bounds.origin,
                        size(view.cell_width * cell.width as f32, view.cell_height),
                    ),
                });
            }
        }
        let cursor = if cursor_visible {
            let cell = snapshot.cell(snapshot.cursor.row, snapshot.cursor.col);
            let cursor_bounds = Bounds::new(
                point(
                    origin.x + view.cell_width * snapshot.cursor.col as f32,
                    origin.y + view.cell_height * snapshot.cursor.row as f32,
                ),
                size(view.cell_width * cell.width as f32, view.cell_height),
            );
            Some(if !view.focused {
                outline(cursor_bounds, cursor_color, BorderStyle::Solid)
            } else {
                match snapshot.cursor.kind {
                    CursorKind::Block => fill(cursor_bounds, cursor_color.opacity(0.88)),
                    CursorKind::Beam => fill(
                        Bounds::new(cursor_bounds.origin, size(px(2.), view.cell_height)),
                        cursor_color,
                    ),
                    CursorKind::Underline => fill(
                        Bounds::new(
                            point(cursor_bounds.left(), cursor_bounds.bottom() - px(2.)),
                            size(cursor_bounds.size.width, px(2.)),
                        ),
                        cursor_color,
                    ),
                }
            })
        } else {
            None
        };
        let composition = if !view.marked_text.is_empty() {
            view.cursor_bounds().map(|cursor| {
                let line = view.composition_line(window);
                let bounds = Bounds::new(
                    cursor.origin,
                    size(line.width().max(view.cell_width), view.cell_height),
                );
                PositionedText {
                    line,
                    origin: bounds.origin,
                    bounds,
                }
            })
        } else {
            None
        };
        TerminalPaint {
            background: default_bg,
            backgrounds,
            text,
            cursor,
            composition,
            line_height: view.cell_height,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        paint: &mut TerminalPaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let pointer_view = self.view.downgrade();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Capture {
                let _ = pointer_view.update(cx, |view, cx| view.on_mouse_move(event, window, cx));
            }
        });
        let (focus_handle, selection, selection_color) = {
            let view = self.view.read(cx);
            (
                view.focus_handle.clone(),
                utf16_range(&view.marked_text, view.marked_selection.clone()),
                rgba(u32::from_be_bytes(view.theme.ui.selection_bg)),
            )
        };
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(fill(bounds, paint.background));
            for quad in paint.backgrounds.drain(..) {
                window.paint_quad(quad);
            }
            if let Some(cursor) = paint.cursor.take() {
                window.paint_quad(cursor);
            }
            for (index, text) in paint
                .text
                .iter()
                .chain(paint.composition.iter())
                .enumerate()
            {
                if index == paint.text.len() {
                    window.paint_quad(fill(text.bounds, paint.background));
                    if !selection.is_empty() {
                        let left = text.line.x_for_index(selection.start);
                        let right = text.line.x_for_index(selection.end);
                        window.paint_quad(fill(
                            Bounds::new(
                                point(text.origin.x + left, text.origin.y),
                                size(right - left, paint.line_height),
                            ),
                            selection_color,
                        ));
                    }
                }
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: text.bounds,
                    }),
                    |window| {
                        if let Err(error) = text.line.paint(
                            text.origin,
                            paint.line_height,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        ) {
                            tracing::warn!(%error, "Terminal glyph painting failed");
                        }
                    },
                );
            }
        });
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = utf16_range(&self.marked_text, range);
        *adjusted = Some(
            utf16_offset(&self.marked_text, range.start)
                ..utf16_offset(&self.marked_text, range.end),
        );
        Some(self.marked_text[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.marked_selection.clone(),
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.marked_text.is_empty()).then(|| 0..self.marked_text.encode_utf16().count())
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = std::mem::take(&mut self.marked_text);
        self.marked_selection = 0..0;
        if self.focus_handle.is_focused(window) {
            self.send_input(text.into_bytes(), true, cx);
        }
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus_handle.is_focused(window) {
            return;
        }
        let range = range
            .map(|range| utf16_range(&self.marked_text, range))
            .unwrap_or(0..self.marked_text.len());
        self.marked_text.replace_range(range, text);
        let committed = std::mem::take(&mut self.marked_text);
        self.marked_selection = 0..0;
        self.send_input(committed.into_bytes(), true, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.connected || !self.focus_handle.is_focused(window) {
            return;
        }
        let range = range
            .map(|range| utf16_range(&self.marked_text, range))
            .unwrap_or(0..self.marked_text.len());
        let start = utf16_offset(&self.marked_text, range.start);
        self.marked_text.replace_range(range, text);
        let len = self.marked_text.encode_utf16().count();
        let selected = selection
            .map(|range| (start + range.start).min(len)..(start + range.end).min(len))
            .unwrap_or(len..len);
        let selected = utf16_range(&self.marked_text, selected);
        self.marked_selection = utf16_offset(&self.marked_text, selected.start)
            ..utf16_offset(&self.marked_text, selected.end);
        self.reset_blink(cx);
        cx.notify();
    }

    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus_handle.is_focused(window) && !self.has_marked_text() {
            if let Some(text) = item.text() {
                self.send_input(self.terminal.paste_bytes(&text), true, cx);
            }
        }
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let cursor = self.cursor_bounds()?;
        let range = utf16_range(&self.marked_text, range);
        let line = self.composition_line(window);
        Some(Bounds::new(
            point(cursor.left() + line.x_for_index(range.start), cursor.top()),
            size(
                (line.x_for_index(range.end) - line.x_for_index(range.start)).max(px(1.)),
                self.cell_height,
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let cursor = self.cursor_bounds()?;
        let line = self.composition_line(window);
        let index = line.closest_index_for_x(position.x - cursor.left());
        Some(utf16_offset(&self.marked_text, index))
    }

    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = utf16_range(&self.marked_text, range);
        self.marked_selection = utf16_offset(&self.marked_text, range.start)
            ..utf16_offset(&self.marked_text, range.end);
        cx.notify();
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.marked_text.encode_utf16().count())
    }

    fn accepts_text_input(&self, window: &mut Window, _: &mut Context<Self>) -> bool {
        self.connected && self.focus_handle.is_focused(window)
    }
}

fn validate_history_text(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Err("不能填入空历史记录".into());
    }
    if text.len() > 64 * 1024 {
        return Err("历史记录超过单条长度上限".into());
    }
    if text.chars().any(|ch| {
        matches!(
            ch,
            '\u{0}'..='\u{1f}'
                | '\u{7f}'..='\u{9f}'
                | '\u{061c}'
                | '\u{200e}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
    }) {
        return Err("仅允许无控制字符的单行历史记录".into());
    }
    Ok(())
}

fn cursor_kind(style: &str) -> CursorKind {
    match style {
        "beam" | "bar" => CursorKind::Beam,
        "underline" => CursorKind::Underline,
        _ => CursorKind::Block,
    }
}

fn measure_cell_width(font: &Font, font_size: Pixels, window: &Window) -> Pixels {
    let system = window.text_system();
    let id = system.resolve_font(font);
    system
        .advance(id, font_size, 'M')
        .map(|size| size.width)
        .unwrap_or(font_size * 0.6)
        .max(px(1.))
}

fn color([r, g, b]: [u8; 3]) -> Hsla {
    rgb(u32::from_be_bytes([0, r, g, b])).into()
}

fn palette_color(index: usize, snapshot: &TerminalSnapshot, theme: &Theme) -> [u8; 3] {
    if let Some(Some(value)) = snapshot.color_overrides.get(index) {
        return *value;
    }
    match index {
        0..=255 => theme.indexed_color(index as u8),
        257 => theme.terminal.bg,
        258 => theme.terminal.cursor,
        259..=266 => theme
            .indexed_color((index - 259) as u8)
            .map(|component| (f32::from(component) * 0.66) as u8),
        268 => theme
            .terminal
            .fg
            .map(|component| (f32::from(component) * 0.66) as u8),
        _ => theme.terminal.fg,
    }
}

fn resolve_color(
    value: PackedColor,
    bold: bool,
    snapshot: &TerminalSnapshot,
    theme: &Theme,
) -> Hsla {
    let index = match value {
        PackedColor::Rgb(value) => return color(value),
        PackedColor::Indexed(index) => index as usize,
        PackedColor::Named(name) => name as usize,
    };
    let index = if bold && index < 8 {
        index + 8
    } else if bold && index == 256 {
        267
    } else {
        index
    };
    color(palette_color(index, snapshot, theme))
}

fn utf16_range(text: &str, range: Range<usize>) -> Range<usize> {
    let offset = |target| {
        let mut units = 0;
        for (index, ch) in text.char_indices() {
            if units >= target {
                return index;
            }
            units += ch.len_utf16();
        }
        text.len()
    };
    offset(range.start)..offset(range.end.max(range.start))
}

fn utf16_offset(text: &str, byte: usize) -> usize {
    text[..byte].encode_utf16().count()
}

fn encode_keystroke(key: &Keystroke, modes: TerminalModes) -> Option<Vec<u8>> {
    let modifiers = key.modifiers;
    if modifiers.platform {
        return None;
    }
    let name = key.key.as_str();
    let parameter = 1
        + usize::from(modifiers.shift)
        + 2 * usize::from(modifiers.alt)
        + 4 * usize::from(modifiers.control);
    let modified = parameter != 1;
    if modifiers.control {
        let control = match name {
            "space" | "@" | "2" => Some(0),
            "[" | "3" => Some(27),
            "\\" | "4" => Some(28),
            "]" | "5" => Some(29),
            "^" | "6" => Some(30),
            "_" | "-" | "7" => Some(31),
            "?" | "8" => Some(127),
            _ if name.len() == 1 && name.as_bytes()[0].is_ascii_alphabetic() => {
                Some(name.as_bytes()[0].to_ascii_lowercase() - b'a' + 1)
            }
            _ => None,
        };
        if let Some(control) = control {
            return Some(if modifiers.alt {
                vec![27, control]
            } else {
                vec![control]
            });
        }
    }
    let final_char = match name {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        _ => None,
    };
    if let Some(final_char) = final_char {
        return Some(if modified {
            format!("\x1b[1;{parameter}{final_char}").into_bytes()
        } else if modes.application_cursor {
            format!("\x1bO{final_char}").into_bytes()
        } else {
            format!("\x1b[{final_char}").into_bytes()
        });
    }
    let tilde = match name {
        "insert" => Some(2),
        "delete" => Some(3),
        "pageup" => Some(5),
        "pagedown" => Some(6),
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        _ => None,
    };
    if let Some(number) = tilde {
        return Some(
            if modified {
                format!("\x1b[{number};{parameter}~")
            } else {
                format!("\x1b[{number}~")
            }
            .into_bytes(),
        );
    }
    if let Some(final_char) = match name {
        "f1" => Some('P'),
        "f2" => Some('Q'),
        "f3" => Some('R'),
        "f4" => Some('S'),
        _ => None,
    } {
        return Some(
            if modified {
                format!("\x1b[1;{parameter}{final_char}")
            } else {
                format!("\x1bO{final_char}")
            }
            .into_bytes(),
        );
    }
    let bytes: Option<&[u8]> = match name {
        "enter" if modes.line_feed_new_line => Some(b"\r\n"),
        "enter" => Some(b"\r"),
        "tab" if modifiers.shift => Some(b"\x1b[Z"),
        "tab" => Some(b"\t"),
        "backspace" if modifiers.control => Some(b"\x08"),
        "backspace" => Some(b"\x7f"),
        "escape" => Some(b"\x1b"),
        _ => None,
    };
    if let Some(bytes) = bytes {
        let mut result = Vec::new();
        if modifiers.alt {
            result.push(27);
        }
        result.extend_from_slice(bytes);
        return Some(result);
    }
    if modes.application_keypad {
        if let Some(final_char) = match name {
            "numpad0" => Some('p'),
            "numpad1" => Some('q'),
            "numpad2" => Some('r'),
            "numpad3" => Some('s'),
            "numpad4" => Some('t'),
            "numpad5" => Some('u'),
            "numpad6" => Some('v'),
            "numpad7" => Some('w'),
            "numpad8" => Some('x'),
            "numpad9" => Some('y'),
            "numpaddecimal" => Some('n'),
            "numpadenter" => Some('M'),
            "numpadadd" => Some('k'),
            "numpadsubtract" => Some('m'),
            "numpadmultiply" => Some('j'),
            "numpaddivide" => Some('o'),
            _ => None,
        } {
            return Some(format!("\x1bO{final_char}").into_bytes());
        }
    }
    if modifiers.alt {
        if let Some(text) = &key.key_char {
            let mut bytes = vec![27];
            bytes.extend_from_slice(text.as_bytes());
            return Some(bytes);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::input::{Input, InputState};
    use gpui_kit::{AppContext, PlatformInput, TestAppContext, WindowHandle};

    struct KeyboardTestView {
        terminal: Entity<TerminalView>,
        first_input: Entity<InputState>,
        second_input: Entity<InputState>,
    }

    impl Render for KeyboardTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(Input::new(&self.first_input))
                .child(Input::new(&self.second_input))
                .child(div().flex_1().min_h_0().child(self.terminal.clone()))
        }
    }

    struct KeyboardFixture {
        window: WindowHandle<gpui_kit::base::Root>,
        view: Entity<KeyboardTestView>,
        commands: mpsc::Receiver<AppCommand>,
    }

    impl KeyboardFixture {
        fn new(connected: bool, cx: &mut TestAppContext) -> Self {
            cx.update(|cx| {
                gpui_kit::init(cx);
                init(cx);
                crate::shortcuts::init(
                    crate::config::ShortcutConfig::defaults("/fixture/keybindings.toml".into()),
                    cx,
                );
            });
            let (command_tx, commands) = mpsc::channel(32);
            let (window, view) = cx.update(|cx| {
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| {
                        let terminal = cx.new(|cx| {
                            TerminalView::new(
                                "terminal-session".into(),
                                command_tx,
                                &AppSettings::default(),
                                window,
                                cx,
                            )
                        });
                        terminal.update(cx, |terminal, cx| {
                            terminal.set_connected(connected, cx);
                            terminal.focus_handle.focus(window, cx);
                        });
                        KeyboardTestView {
                            terminal,
                            first_input: cx.new(|cx| InputState::new(window, cx)),
                            second_input: cx.new(|cx| InputState::new(window, cx)),
                        }
                    })
                })
                .expect("keyboard test window")
            });
            Self {
                window: window.downcast().expect("Base Root"),
                view,
                commands,
            }
        }

        fn update<R>(
            &self,
            cx: &mut TestAppContext,
            update: impl FnOnce(&mut KeyboardTestView, &mut Window, &mut Context<KeyboardTestView>) -> R,
        ) -> R {
            cx.update_window(self.window.into(), |_, window, cx| {
                self.view.update(cx, |view, cx| update(view, window, cx))
            })
            .expect("update keyboard test")
        }

        fn press(&self, key: &str, cx: &mut TestAppContext) {
            cx.update_window(self.window.into(), |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::KeyDown(KeyDownEvent {
                        keystroke: Keystroke::parse(key).expect("test key"),
                        is_held: false,
                        prefer_character_input: false,
                    }),
                    cx,
                );
            })
            .expect("dispatch test key");
        }

        fn input(&mut self, cx: &mut TestAppContext) -> Vec<(String, Vec<u8>)> {
            cx.run_until_parked();
            let mut input = Vec::new();
            while let Ok(command) = self.commands.try_recv() {
                if let AppCommand::WriteInput { session_id, data } = command {
                    input.push((session_id, data));
                }
            }
            input
        }

        fn assert_terminal_focused(&self, cx: &mut TestAppContext) {
            self.update(cx, |view, window, cx| {
                assert!(view.terminal.read(cx).focus_handle.is_focused(window));
            });
        }
    }

    #[gpui::test]
    fn tab_keys_send_terminal_bytes_without_moving_focus(cx: &mut TestAppContext) {
        let mut fixture = KeyboardFixture::new(true, cx);
        for (key, expected) in [
            ("tab", b"\t".as_slice()),
            ("shift-tab", b"\x1b[Z".as_slice()),
        ] {
            fixture.press(key, cx);
            fixture.assert_terminal_focused(cx);
            assert_eq!(
                fixture.input(cx),
                vec![("terminal-session".into(), expected.to_vec())]
            );
        }
    }

    #[gpui::test]
    fn disconnected_terminal_tab_keys_do_not_send_input(cx: &mut TestAppContext) {
        let mut fixture = KeyboardFixture::new(false, cx);
        for key in ["tab", "shift-tab"] {
            fixture.press(key, cx);
            fixture.assert_terminal_focused(cx);
        }
        assert!(fixture.input(cx).is_empty());
    }

    #[gpui::test]
    fn tab_keys_keep_normal_input_focus_navigation(cx: &mut TestAppContext) {
        let mut fixture = KeyboardFixture::new(true, cx);
        fixture.update(cx, |view, window, cx| {
            view.first_input.focus_handle(cx).focus(window, cx);
        });
        fixture.press("tab", cx);
        fixture.update(cx, |view, window, cx| {
            assert!(view.second_input.focus_handle(cx).is_focused(window));
        });
        fixture.press("shift-tab", cx);
        fixture.update(cx, |view, window, cx| {
            assert!(view.first_input.focus_handle(cx).is_focused(window));
            assert!(view.first_input.read(cx).value().is_empty());
            assert!(view.second_input.read(cx).value().is_empty());
        });
        assert!(fixture.input(cx).is_empty());
    }

    #[gpui::test]
    fn tab_keys_preserve_active_ime_composition(cx: &mut TestAppContext) {
        let mut fixture = KeyboardFixture::new(true, cx);
        fixture.update(cx, |view, window, cx| {
            view.terminal.update(cx, |terminal, cx| {
                terminal.replace_and_mark_text_in_range(None, "中", Some(0..1), window, cx);
            });
        });
        for key in ["tab", "shift-tab"] {
            fixture.press(key, cx);
            fixture.assert_terminal_focused(cx);
            fixture.update(cx, |view, _, cx| {
                let terminal = view.terminal.read(cx);
                assert_eq!(terminal.marked_text, "中");
                assert_eq!(terminal.marked_selection, 0..1);
            });
        }
        assert!(fixture.input(cx).is_empty());
        fixture.update(cx, |view, window, cx| {
            view.terminal.update(cx, |terminal, cx| {
                terminal.replace_text_in_range(None, "中文", window, cx);
            });
        });
        fixture.press("tab", cx);
        fixture.assert_terminal_focused(cx);
        assert_eq!(
            fixture.input(cx),
            vec![
                ("terminal-session".into(), "中文".as_bytes().to_vec()),
                ("terminal-session".into(), b"\t".to_vec()),
            ]
        );
    }

    #[gpui::test]
    fn history_insert_requires_matching_ready_shell_and_sends_no_enter(cx: &mut TestAppContext) {
        let mut fixture = KeyboardFixture::new(true, cx);
        fixture.update(cx, |view, window, cx| {
            view.terminal.update(cx, |terminal, cx| {
                assert!(terminal
                    .insert_history("printf safe", "terminal-session", window, cx)
                    .is_err());
                terminal.set_shell_ready(true, cx);
                assert!(terminal
                    .insert_history("printf safe", "another-session", window, cx)
                    .is_err());
                terminal
                    .insert_history("printf safe", "terminal-session", window, cx)
                    .unwrap();
            });
        });
        assert_eq!(
            fixture.input(cx),
            vec![("terminal-session".into(), b"printf safe".to_vec())]
        );
        fixture.update(cx, |view, window, cx| {
            view.terminal.update(cx, |terminal, cx| {
                terminal.set_connected(false, cx);
                terminal.set_shell_ready(true, cx);
                assert!(!terminal.shell_ready);
                terminal.set_connected(true, cx);
                assert!(terminal
                    .insert_history("printf safe", "terminal-session", window, cx)
                    .is_err());
            });
        });
        assert!(fixture.input(cx).is_empty());
    }

    #[gpui::test]
    fn history_insert_rejects_controls_composition_and_alternate_screen(cx: &mut TestAppContext) {
        let mut fixture = KeyboardFixture::new(true, cx);
        fixture.update(cx, |view, window, cx| {
            view.terminal.update(cx, |terminal, cx| {
                terminal.set_shell_ready(true, cx);
                for text in [
                    "",
                    "a\nb",
                    "a\rb",
                    "a\tb",
                    "a\u{0}b",
                    "a\u{7f}b",
                    "a\u{85}b",
                    "a\u{061c}b",
                    "a\u{200e}b",
                    "a\u{2028}b",
                    "a\u{2029}b",
                    "a\u{202e}b",
                    "a\u{2066}b",
                ] {
                    assert!(terminal
                        .insert_history(text, "terminal-session", window, cx)
                        .is_err());
                }
                terminal.replace_and_mark_text_in_range(None, "中", Some(0..1), window, cx);
                assert!(terminal
                    .insert_history("safe", "terminal-session", window, cx)
                    .is_err());
                assert_eq!(terminal.marked_text, "中");
                terminal.marked_text.clear();
                terminal.process_output(b"\x1b[?1049h", cx);
                assert!(terminal
                    .insert_history("safe", "terminal-session", window, cx)
                    .is_err());
            });
        });
        assert!(fixture.input(cx).is_empty());
        fixture.update(cx, |view, window, cx| {
            view.terminal.update(cx, |terminal, cx| {
                terminal.process_output(b"\x1b[?1049l\x1b[?2004h", cx);
                terminal
                    .insert_history("printf '中文'", "terminal-session", window, cx)
                    .unwrap();
            });
        });
        assert_eq!(
            fixture.input(cx),
            vec![(
                "terminal-session".into(),
                "\u{1b}[200~printf '中文'\u{1b}[201~".as_bytes().to_vec()
            )]
        );
    }

    #[gpui::test]
    fn terminal_actions_preserve_ctrl_c_and_active_composition(cx: &mut TestAppContext) {
        let mut fixture = KeyboardFixture::new(true, cx);
        fixture.press("ctrl-c", cx);
        assert_eq!(
            fixture.input(cx),
            vec![("terminal-session".into(), vec![3])]
        );
        fixture.update(cx, |view, window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string("must not paste".into()));
            view.terminal.update(cx, |terminal, cx| {
                terminal.replace_and_mark_text_in_range(None, "中", Some(0..1), window, cx);
            });
        });
        fixture.press(
            if cfg!(target_os = "macos") {
                "cmd-v"
            } else {
                "ctrl-shift-v"
            },
            cx,
        );
        fixture.update(cx, |view, _, cx| {
            assert!(view.terminal.read(cx).has_marked_text())
        });
        assert!(fixture.input(cx).is_empty());
    }

    #[test]
    fn keys_preserve_terminal_modes_and_control_characters() {
        let modes = TerminalModes::default();
        assert_eq!(
            encode_keystroke(&Keystroke::parse("ctrl-c").unwrap(), modes),
            Some(vec![3])
        );
        assert_eq!(
            encode_keystroke(&Keystroke::parse("up").unwrap(), modes),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_keystroke(&Keystroke::parse("ctrl-shift-right").unwrap(), modes),
            Some(b"\x1b[1;6C".to_vec())
        );
        assert_eq!(
            encode_keystroke(&Keystroke::parse("tab").unwrap(), modes),
            Some(b"\t".to_vec())
        );
        assert_eq!(
            encode_keystroke(&Keystroke::parse("shift-tab").unwrap(), modes),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            encode_keystroke(&Keystroke::parse("f1").unwrap(), modes),
            Some(b"\x1bOP".to_vec())
        );
        assert_eq!(
            encode_keystroke(&Keystroke::parse("cmd-c").unwrap(), modes),
            None
        );
        assert_eq!(
            encode_keystroke(&Keystroke::parse("a").unwrap(), modes),
            None
        );
        assert_eq!(
            encode_keystroke(
                &Keystroke::parse("up").unwrap(),
                TerminalModes {
                    application_cursor: true,
                    ..modes
                }
            ),
            Some(b"\x1bOA".to_vec())
        );
    }

    #[test]
    fn theme_changes_preserve_truecolor_extended_colors_and_osc_overrides() {
        let mut terminal = Terminal::with_history(20, 4, 100);
        terminal.process_output(b"\x1b]4;4;rgb:12/34/56\x07\x1b]10;rgb:ab/cd/ef\x07");
        let snapshot = terminal.snapshot();
        for preset in crate::config::BUILTIN_THEMES {
            let theme = Theme::for_name(preset.id);
            assert_eq!(palette_color(4, &snapshot, &theme), [0x12, 0x34, 0x56]);
            assert_eq!(palette_color(256, &snapshot, &theme), [0xab, 0xcd, 0xef]);
            assert_eq!(palette_color(21, &snapshot, &theme), [0, 0, 255]);
            assert_eq!(palette_color(240, &snapshot, &theme), [88; 3]);
            assert_eq!(
                resolve_color(
                    PackedColor::Rgb([0x19, 0x28, 0x37]),
                    false,
                    &snapshot,
                    &theme
                ),
                color([0x19, 0x28, 0x37])
            );
        }
    }

    #[test]
    fn ime_ranges_are_utf16_not_utf8_offsets() {
        let text = "中😀e\u{301}";
        assert_eq!(&text[utf16_range(text, 1..3)], "😀");
        assert_eq!(utf16_offset(text, "中😀".len()), 3);
        assert_eq!(utf16_range(text, 30..40), text.len()..text.len());
    }
}
