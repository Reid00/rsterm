use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::component::{
    button::{Button, ButtonVariants},
    chart::LineChart,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    progress::Progress,
    ActiveTheme, Disableable, IconName, Selectable, Sizable,
};
use gpui_kit::{prelude::*, *};
use tokio::sync::mpsc;

use crate::backend::{AppCommand, BackendEvent};
use crate::inspection::{
    HistoryEntry, HistorySnapshot, HistorySource, InspectionData, InspectionError, InspectionKind,
    NetworkSnapshot, SystemSampler, SystemSnapshot,
};

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InspectorPage {
    #[default]
    System,
    Network,
    History,
}

impl InspectorPage {
    fn label(self) -> &'static str {
        match self {
            Self::System => "系统",
            Self::Network => "网络",
            Self::History => "历史",
        }
    }

    fn interval(self) -> Option<Duration> {
        match self {
            Self::System => Some(Duration::from_secs(2)),
            Self::Network => Some(Duration::from_secs(5)),
            Self::History => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectionTarget {
    pub tab_id: String,
    pub session_id: String,
    pub label: String,
    pub remote: bool,
    pub connected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InspectorAction {
    Close,
    FocusTerminal,
    InsertHistory {
        target: InspectionTarget,
        snapshot_id: u64,
        entry_id: u64,
    },
}

#[derive(Clone)]
struct Intent {
    target: InspectionTarget,
    generation: u64,
    kind: InspectionKind,
}

struct Request {
    intent: Intent,
    id: u64,
}

struct StoredHistory {
    id: u64,
    snapshot: HistorySnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadStatus {
    Idle,
    Loading,
    Waiting,
    Ready,
    Failed(InspectionError),
}

#[derive(Clone)]
struct CpuPoint {
    index: u64,
    value: f64,
}

struct InspectorState {
    commands: mpsc::UnboundedSender<AppCommand>,
    target: Option<InspectionTarget>,
    visible: bool,
    page: InspectorPage,
    source: HistorySource,
    generation: u64,
    active: Option<Request>,
    blocked: HashSet<String>,
    pending: Option<Intent>,
    status: ReadStatus,
    stale: bool,
    halted: bool,
    system: Option<SystemSnapshot>,
    sampler: SystemSampler,
    cpu_points: VecDeque<CpuPoint>,
    sample_index: u64,
    network: Option<NetworkSnapshot>,
    history: Option<StoredHistory>,
    selected: Option<u64>,
    query: String,
}

impl InspectorState {
    fn new(commands: mpsc::UnboundedSender<AppCommand>) -> Self {
        Self {
            commands,
            target: None,
            visible: false,
            page: InspectorPage::System,
            source: HistorySource::default(),
            generation: 0,
            active: None,
            blocked: HashSet::new(),
            pending: None,
            status: ReadStatus::Idle,
            stale: false,
            halted: false,
            system: None,
            sampler: SystemSampler::default(),
            cpu_points: VecDeque::new(),
            sample_index: 0,
            network: None,
            history: None,
            selected: None,
            query: String::new(),
        }
    }

    fn current_kind(&self) -> InspectionKind {
        match self.page {
            InspectorPage::System => InspectionKind::System,
            InspectorPage::Network => InspectionKind::Network,
            InspectorPage::History => InspectionKind::History(self.source),
        }
    }

    fn cancel_request(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
        if let Some(request) = self.active.take() {
            let _ = self.commands.send(AppCommand::CancelInspection {
                session_id: request.intent.target.session_id,
                request_id: request.id,
            });
        }
    }

    fn clear_content(&mut self) {
        self.system = None;
        self.sampler.reset();
        self.cpu_points.clear();
        self.sample_index = 0;
        self.network = None;
        self.history = None;
        self.selected = None;
        self.query.clear();
        self.status = ReadStatus::Idle;
        self.stale = false;
        self.halted = false;
    }

    fn invalidate(&mut self) {
        self.cancel_request();
        self.clear_content();
    }

    fn set_target(&mut self, target: Option<InspectionTarget>) -> bool {
        if self.target == target {
            return false;
        }
        self.invalidate();
        self.target = target;
        self.request_automatic();
        true
    }

    fn set_visible(&mut self, visible: bool) -> bool {
        if self.visible == visible {
            return false;
        }
        self.invalidate();
        self.visible = visible;
        self.request_automatic();
        true
    }

    fn set_page(&mut self, page: InspectorPage) -> bool {
        if self.page == page {
            return false;
        }
        self.invalidate();
        self.page = page;
        self.request_automatic();
        true
    }

    fn set_source(&mut self, source: HistorySource) -> bool {
        if self.source == source {
            return false;
        }
        self.invalidate();
        self.source = source;
        true
    }

    fn request_automatic(&mut self) {
        if self.page != InspectorPage::History && !self.halted {
            self.request();
        }
    }

    fn refresh(&mut self) {
        if !self.visible || !self.target.as_ref().is_some_and(|target| target.connected) {
            return;
        }
        self.cancel_request();
        self.halted = false;
        self.stale = self.system.is_some() || self.network.is_some();
        self.history = None;
        self.selected = None;
        self.request();
    }

    fn request(&mut self) {
        let Some(target) = self
            .target
            .as_ref()
            .filter(|target| self.visible && target.connected)
        else {
            return;
        };
        self.pending = Some(Intent {
            target: target.clone(),
            generation: self.generation,
            kind: self.current_kind(),
        });
        self.dispatch_pending();
    }

    fn intent_is_current(&self, intent: &Intent) -> bool {
        self.visible
            && self.generation == intent.generation
            && self.target.as_ref() == Some(&intent.target)
            && intent.target.connected
            && self.current_kind() == intent.kind
    }

    fn dispatch_pending(&mut self) {
        let Some(intent) = self.pending.as_ref() else {
            return;
        };
        if !self.intent_is_current(intent) {
            self.pending = None;
            return;
        }
        if self.active.is_some() || self.blocked.contains(&intent.target.session_id) {
            self.status = ReadStatus::Waiting;
            return;
        }
        let intent = self.pending.take().unwrap();
        let id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        let session_id = intent.target.session_id.clone();
        self.blocked.insert(session_id.clone());
        self.active = Some(Request { intent, id });
        self.status = ReadStatus::Loading;
        if self
            .commands
            .send(AppCommand::Inspect {
                session_id: session_id.clone(),
                request_id: id,
                kind: self.current_kind(),
            })
            .is_err()
        {
            self.blocked.remove(&session_id);
            self.active = None;
            self.status = ReadStatus::Failed(InspectionError::Unavailable);
            self.halted = true;
        }
    }

    fn expire_wait(&mut self) {
        if self.status == ReadStatus::Waiting {
            self.pending = None;
            self.status = ReadStatus::Failed(InspectionError::Cleaning);
            self.halted = true;
        }
    }

    fn handle_event(&mut self, event: &BackendEvent) -> Option<Duration> {
        match event {
            BackendEvent::InspectionReady { session_id } => {
                self.blocked.remove(session_id);
                self.dispatch_pending();
            }
            BackendEvent::InspectionResult {
                session_id,
                request_id,
                result,
            } => {
                let request = self.active.as_ref().filter(|request| {
                    request.id == *request_id
                        && &request.intent.target.session_id == session_id
                        && self.intent_is_current(&request.intent)
                })?;
                let intent = request.intent.clone();
                self.active = None;
                let result = match result {
                    Ok(data) if data_matches_kind(data, intent.kind) => Ok(data),
                    Ok(_) => Err(InspectionError::InvalidData),
                    Err(error) => Err(*error),
                };
                match result {
                    Ok(InspectionData::System(snapshot)) => {
                        let sample = self.sampler.sample(snapshot.clone());
                        self.sample_index += 1;
                        if let Some(value) =
                            sample.cpu_usage_percent.filter(|value| value.is_finite())
                        {
                            if self.cpu_points.len() == 60 {
                                self.cpu_points.pop_front();
                            }
                            self.cpu_points.push_back(CpuPoint {
                                index: self.sample_index,
                                value: f64::from(value.clamp(0., 100.)),
                            });
                        } else {
                            self.cpu_points.clear();
                        }
                        self.system = Some(sample);
                        self.status = ReadStatus::Ready;
                        self.stale = false;
                    }
                    Ok(InspectionData::Network(snapshot)) => {
                        let mut snapshot = snapshot.clone();
                        if snapshot.connections.len() > 1000 {
                            snapshot.connections.truncate(1000);
                            snapshot.truncated = true;
                        }
                        self.network = Some(snapshot);
                        self.status = ReadStatus::Ready;
                        self.stale = false;
                    }
                    Ok(InspectionData::History(snapshot)) => {
                        self.history = Some(StoredHistory {
                            id: *request_id,
                            snapshot: snapshot.clone(),
                        });
                        self.selected = None;
                        self.status = ReadStatus::Ready;
                        self.stale = false;
                    }
                    Err(InspectionError::Busy) => {
                        self.pending = Some(intent);
                        self.status = ReadStatus::Waiting;
                        return None;
                    }
                    Err(error) => {
                        self.status = ReadStatus::Failed(error);
                        self.stale = self.system.is_some() || self.network.is_some();
                        self.halted = error.stops_polling() || error == InspectionError::Cancelled;
                    }
                }
                if !self.halted {
                    return self.page.interval();
                }
            }
            BackendEvent::SessionClosed { session_id, .. }
            | BackendEvent::SessionFailed { session_id, .. } => {
                self.blocked.remove(session_id);
                if self
                    .target
                    .as_ref()
                    .is_some_and(|target| &target.session_id == session_id)
                {
                    let mut target = self.target.clone().unwrap();
                    target.connected = false;
                    self.set_target(Some(target));
                }
            }
            _ => {}
        }
        None
    }

    fn history_entry(
        &self,
        target: &InspectionTarget,
        snapshot_id: u64,
        entry_id: u64,
    ) -> Option<&HistoryEntry> {
        if !self.visible
            || self.page != InspectorPage::History
            || !target.connected
            || self.target.as_ref() != Some(target)
            || self.status != ReadStatus::Ready
        {
            return None;
        }
        let history = self.history.as_ref()?;
        if history.id != snapshot_id || history.snapshot.source != self.source {
            return None;
        }
        history
            .snapshot
            .entries
            .iter()
            .find(|entry| entry.id == entry_id)
    }

    fn insertion_text(
        &self,
        target: &InspectionTarget,
        snapshot_id: u64,
        entry_id: u64,
    ) -> Option<String> {
        self.history_entry(target, snapshot_id, entry_id)?
            .insertion_text()
            .map(str::to_owned)
    }

    fn filtered_history(&self) -> Vec<usize> {
        self.history
            .as_ref()
            .map(|history| {
                let query = self.query.trim().to_lowercase();
                history
                    .snapshot
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| {
                        query.is_empty() || entry.preview.to_lowercase().contains(&query)
                    })
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn filtered_connections(&self) -> Vec<usize> {
        self.network
            .as_ref()
            .map(|network| {
                let query = self.query.trim().to_lowercase();
                network
                    .connections
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| {
                        query.is_empty()
                            || format!(
                                "{} {} {} {} {} {}",
                                row.protocol,
                                row.state,
                                row.local_address,
                                row.remote_address,
                                row.process.as_deref().unwrap_or(""),
                                row.pid.map(|pid| pid.to_string()).unwrap_or_default()
                            )
                            .to_lowercase()
                            .contains(&query)
                    })
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn data_matches_kind(data: &InspectionData, kind: InspectionKind) -> bool {
    match (data, kind) {
        (InspectionData::System(_), InspectionKind::System)
        | (InspectionData::Network(_), InspectionKind::Network) => true,
        (InspectionData::History(snapshot), InspectionKind::History(source)) => {
            snapshot.source == source
        }
        _ => false,
    }
}

pub struct InspectorView {
    state: InspectorState,
    focus: FocusHandle,
    search: Option<Entity<InputState>>,
    search_subscription: Option<Subscription>,
    history_scroll: UniformListScrollHandle,
    network_scroll: UniformListScrollHandle,
    poll_task: Option<Task<()>>,
    wait_task: Option<Task<()>>,
}

impl InspectorView {
    pub fn new(
        commands: mpsc::UnboundedSender<AppCommand>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            state: InspectorState::new(commands),
            focus: cx.focus_handle().tab_stop(true),
            search: None,
            search_subscription: None,
            history_scroll: UniformListScrollHandle::default(),
            network_scroll: UniformListScrollHandle::default(),
            poll_task: None,
            wait_task: None,
        }
    }

    fn reset_view(&mut self) {
        self.poll_task = None;
        self.wait_task = None;
        self.search_subscription = None;
        self.search = None;
        self.history_scroll = UniformListScrollHandle::default();
        self.network_scroll = UniformListScrollHandle::default();
    }

    pub fn set_target(&mut self, target: Option<InspectionTarget>, cx: &mut Context<Self>) {
        if self.state.set_target(target) {
            self.reset_view();
            self.sync_wait(cx);
            cx.notify();
        }
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.state.set_visible(visible) {
            self.reset_view();
            self.sync_wait(cx);
            cx.notify();
        }
    }

    pub fn show_page(&mut self, page: InspectorPage, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.set_page(page) {
            self.reset_view();
        }
        self.sync_wait(cx);
        if self.state.visible {
            if page == InspectorPage::History {
                self.ensure_search(window, cx)
                    .focus_handle(cx)
                    .focus(window, cx);
            } else {
                self.focus.focus(window, cx);
            }
        }
        cx.notify();
    }

    pub fn page(&self) -> InspectorPage {
        self.state.page
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn handle_event(&mut self, event: &BackendEvent, cx: &mut Context<Self>) {
        if !matches!(
            event,
            BackendEvent::InspectionResult { .. }
                | BackendEvent::InspectionReady { .. }
                | BackendEvent::SessionClosed { .. }
                | BackendEvent::SessionFailed { .. }
        ) {
            return;
        }
        let generation = self.state.generation;
        let interval = self.state.handle_event(event);
        if generation != self.state.generation {
            self.reset_view();
        }
        if let Some(interval) = interval {
            self.schedule_poll(interval, cx);
        }
        self.sync_wait(cx);
        cx.notify();
    }

    pub fn insertion_text(
        &self,
        target: &InspectionTarget,
        snapshot_id: u64,
        entry_id: u64,
    ) -> Option<String> {
        self.state.insertion_text(target, snapshot_id, entry_id)
    }

    #[cfg(test)]
    pub(crate) fn refresh_for_test(&mut self, cx: &mut Context<Self>) {
        self.refresh(cx);
    }

    #[cfg(test)]
    pub(crate) fn system_for_test(&self) -> Option<&SystemSnapshot> {
        self.state.system.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn history_for_test(&self) -> Option<&HistorySnapshot> {
        self.state.history.as_ref().map(|history| &history.snapshot)
    }

    fn ensure_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(search) = &self.search {
            return search.clone();
        }
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(if self.state.page == InspectorPage::History {
                "搜索当前历史"
            } else {
                "筛选连接、地址或进程"
            })
        });
        self.search_subscription = Some(cx.subscribe(
            &search,
            |this, search, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.state.query = search.read(cx).value().to_string();
                    this.history_scroll = UniformListScrollHandle::default();
                    this.network_scroll = UniformListScrollHandle::default();
                    cx.notify();
                }
            },
        ));
        self.search = Some(search.clone());
        search
    }

    fn sync_wait(&mut self, cx: &mut Context<Self>) {
        if self.state.status != ReadStatus::Waiting {
            self.wait_task = None;
            return;
        }
        if self.wait_task.is_some() {
            return;
        }
        let generation = self.state.generation;
        self.wait_task = Some(cx.spawn(async move |view, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(10))
                .await;
            let _ = view.update(cx, |view, cx| {
                if view.state.generation == generation {
                    view.state.expire_wait();
                    cx.notify();
                }
            });
        }));
    }

    fn schedule_poll(&mut self, interval: Duration, cx: &mut Context<Self>) {
        self.poll_task = None;
        let generation = self.state.generation;
        let target = self.state.target.clone();
        self.poll_task = Some(cx.spawn(async move |view, cx| {
            cx.background_executor().timer(interval).await;
            let _ = view.update(cx, |view, cx| {
                if view.state.generation == generation && view.state.target == target {
                    view.state.request_automatic();
                    view.sync_wait(cx);
                    cx.notify();
                }
            });
        }));
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.poll_task = None;
        self.state.refresh();
        self.wait_task = None;
        self.sync_wait(cx);
        self.history_scroll = UniformListScrollHandle::default();
        cx.notify();
    }

    fn set_source(&mut self, source: HistorySource, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.set_source(source) {
            self.reset_view();
            self.ensure_search(window, cx);
            cx.notify();
        }
    }

    fn select_history(
        &mut self,
        target: &InspectionTarget,
        snapshot_id: u64,
        entry_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .state
            .history_entry(target, snapshot_id, entry_id)
            .is_some()
        {
            self.state.selected = Some(entry_id);
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    fn copy_history(
        &self,
        target: &InspectionTarget,
        snapshot_id: u64,
        entry_id: u64,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = self
            .state
            .history_entry(target, snapshot_id, entry_id)
            .and_then(HistoryEntry::copy_text)
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        }
    }

    fn insert_history(
        &self,
        target: &InspectionTarget,
        snapshot_id: u64,
        entry_id: u64,
        cx: &mut Context<Self>,
    ) {
        if self.insertion_text(target, snapshot_id, entry_id).is_some() {
            cx.emit(InspectorAction::InsertHistory {
                target: target.clone(),
                snapshot_id,
                entry_id,
            });
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.page != InspectorPage::History
            || !self.state.visible
            || !self.focus.is_focused(window)
        {
            return;
        }
        let key = &event.keystroke;
        if key.modifiers.control
            || key.modifiers.alt
            || key.modifiers.platform
            || key.modifiers.shift
        {
            return;
        }
        let rows = self.state.filtered_history();
        let Some(history) = &self.state.history else {
            return;
        };
        let position = self.state.selected.and_then(|id| {
            rows.iter()
                .position(|index| history.snapshot.entries[*index].id == id)
        });
        let next = match key.key.as_str() {
            "down" => {
                Some(position.map_or(0, |index| (index + 1).min(rows.len().saturating_sub(1))))
            }
            "up" => Some(position.unwrap_or(0).saturating_sub(1)),
            "enter" => position.or((!rows.is_empty()).then_some(0)),
            _ => None,
        };
        if let Some(position) = next.filter(|position| *position < rows.len()) {
            self.state.selected = Some(history.snapshot.entries[rows[position]].id);
            self.history_scroll
                .scroll_to_item(position, ScrollStrategy::Nearest);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn status_text(&self) -> Option<String> {
        if self.state.target.is_none() {
            return Some("选择一个终端，查看当前连接账户的信息。".into());
        }
        if self
            .state
            .target
            .as_ref()
            .is_some_and(|target| !target.connected)
        {
            return Some("当前会话未连接。连接后可手动打开检查面板。".into());
        }
        match self.state.status {
            ReadStatus::Loading => Some(
                if self.state.stale {
                    "正在刷新 · 上次数据已过期"
                } else {
                    "正在读取…"
                }
                .into(),
            ),
            ReadStatus::Waiting => Some("正在等待上次读取清理完成…".into()),
            ReadStatus::Failed(error) => Some(format!(
                "{}{}",
                if self.state.stale {
                    "数据已过期 · "
                } else {
                    ""
                },
                error_label(error)
            )),
            _ => None,
        }
    }

    fn system_page(&self, cx: &Context<Self>) -> AnyElement {
        let Some(snapshot) = &self.state.system else {
            return empty_content(
                if self.state.status == ReadStatus::Idle {
                    "打开系统页后开始只读采样。"
                } else {
                    "等待有效系统样本。"
                },
                cx,
            );
        };
        let cpu = snapshot.cpu_usage_percent.filter(|value| value.is_finite());
        let memory = memory_percent(snapshot);
        let mut body = div()
            .id("inspector-system-scroll")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .p_3()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(info_row(
                        "系统",
                        snapshot.os.as_deref().unwrap_or("未知"),
                        cx,
                    ))
                    .child(info_row(
                        "架构",
                        snapshot.architecture.as_deref().unwrap_or("未知"),
                        cx,
                    ))
                    .child(info_row(
                        "运行时间",
                        &format_uptime(snapshot.uptime_seconds),
                        cx,
                    ))
                    .child(info_row(
                        "CPU",
                        snapshot.cpu_model.as_deref().unwrap_or("未知"),
                        cx,
                    ))
                    .child(info_row(
                        "逻辑核心",
                        &snapshot
                            .logical_cores
                            .map(|cores| cores.to_string())
                            .unwrap_or_else(|| "未知".into()),
                        cx,
                    )),
            )
            .child(metric(
                "inspector-cpu",
                "CPU 使用率",
                cpu.map(|value| format!("{value:.1}%"))
                    .unwrap_or_else(|| "预热中".into()),
                cpu,
                cx,
            ))
            .when(self.cpu_chart_available(), |body| {
                body.child(
                    div().h(px(68.)).min_w_0().w_full().child(
                        LineChart::new(self.state.cpu_points.iter().cloned())
                            .id("inspector-cpu-chart")
                            .name("CPU 使用率")
                            .x(|point: &CpuPoint| point.index.to_string())
                            .y(|point: &CpuPoint| point.value)
                            .y_domain(0., 100.)
                            .stroke(cx.theme().primary)
                            .linear()
                            .x_axis(false)
                            .grid(false)
                            .appear(false)
                            .tooltip_title(|_| "CPU 使用率".into())
                            .tooltip_value(|_, value| format!("{value:.1}%").into()),
                    ),
                )
            })
            .child(metric(
                "inspector-memory",
                "内存",
                match (snapshot.memory_used_bytes, snapshot.memory_total_bytes) {
                    (Some(used), Some(total)) if total > 0 => format!(
                        "{} / {}",
                        format_bytes(used as f64),
                        format_bytes(total as f64)
                    ),
                    _ => "未知".into(),
                },
                memory,
                cx,
            ));
        let mut interfaces = div().flex().flex_col().gap_2().min_w_0().child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child("网络接口"),
        );
        if snapshot.interfaces.is_empty() {
            interfaces = interfaces.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("没有可用的接口计数。"),
            );
        }
        for interface in &snapshot.interfaces {
            interfaces = interfaces.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .py_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_xs()
                            .font_family(cx.theme().mono_font_family.clone())
                            .truncate()
                            .child(interface.name.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap_2()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "↓ {}",
                                format_rate(interface.receive_bytes_per_second)
                            ))
                            .child(format!(
                                "↑ {}",
                                format_rate(interface.transmit_bytes_per_second)
                            )),
                    ),
            );
        }
        body = body.child(interfaces);
        if !snapshot.notes.is_empty() {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("部分系统信息不可用，其余有效字段仍会显示。"),
            );
        }
        body.into_any_element()
    }

    fn cpu_chart_available(&self) -> bool {
        self.state.cpu_points.len() > 1
    }

    fn network_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let search = self.ensure_search(window, cx);
        let indices = self.state.filtered_connections();
        let count = indices.len();
        let truncated = self
            .state
            .network
            .as_ref()
            .is_some_and(|network| network.truncated);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .child(
                div().p_3().flex_shrink_0().child(
                    Input::new(&search)
                        .id("inspector-network-search")
                        .cleanable(true)
                        .w_full(),
                ),
            )
            .child(
                div()
                    .id("inspector-network-horizontal")
                    .debug_selector(|| "inspector-network-horizontal".into())
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_x_scroll()
                    .overflow_y_hidden()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w(px(830.))
                            .min_w(px(830.))
                            .h_full()
                            .min_h_0()
                            .child(network_header(cx))
                            .child(
                                uniform_list(
                                    "inspector-network-list",
                                    count,
                                    cx.processor(
                                        move |this, range: std::ops::Range<usize>, _, cx| {
                                            let Some(network) = &this.state.network else {
                                                return Vec::new();
                                            };
                                            range
                                                .filter_map(|position| {
                                                    indices.get(position).and_then(|index| {
                                                        network.connections.get(*index)
                                                    })
                                                })
                                                .map(|row| {
                                                    div()
                                                        .h(px(30.))
                                                        .min_h(px(30.))
                                                        .flex()
                                                        .items_center()
                                                        .text_xs()
                                                        .font_family(
                                                            cx.theme().mono_font_family.clone(),
                                                        )
                                                        .child(table_cell(&row.protocol, 64.))
                                                        .child(table_cell(&row.state, 110.))
                                                        .child(table_cell(&row.local_address, 220.))
                                                        .child(table_cell(
                                                            &row.remote_address,
                                                            220.,
                                                        ))
                                                        .child(table_cell(
                                                            row.process.as_deref().unwrap_or("—"),
                                                            140.,
                                                        ))
                                                        .child(table_cell(
                                                            &row.pid
                                                                .map(|pid| pid.to_string())
                                                                .unwrap_or_else(|| "—".into()),
                                                            76.,
                                                        ))
                                                        .into_any_element()
                                                })
                                                .collect()
                                        },
                                    ),
                                )
                                .track_scroll(&self.network_scroll)
                                .flex_1()
                                .min_h_0()
                                .w_full(),
                            )
                            .when(count == 0, |body| {
                                body.child(empty_content(
                                    if self.state.network.is_none() {
                                        "等待网络连接快照。"
                                    } else if self.state.query.is_empty() {
                                        "没有可见的 TCP / UDP 连接。"
                                    } else {
                                        "没有匹配的连接。"
                                    },
                                    cx,
                                ))
                            }),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if truncated {
                        format!("{count} 条匹配 · 最多显示 1000 条，结果已截断")
                    } else {
                        format!("{count} 条匹配 · 仅显示当前账户可见的进程")
                    }),
            )
            .into_any_element()
    }

    fn history_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let search = self.ensure_search(window, cx);
        let indices = self.state.filtered_history();
        let count = indices.len();
        let source = self.state.source;
        let connected = self
            .state
            .target
            .as_ref()
            .is_some_and(|target| target.connected);
        let target = self.state.target.clone();
        let snapshot_id = self.state.history.as_ref().map(|history| history.id);
        let truncated = self
            .state
            .history
            .as_ref()
            .is_some_and(|history| history.snapshot.truncated);
        let mut body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .child(
                div()
                    .p_3()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .min_w_0()
                            .children([HistorySource::Bash, HistorySource::Zsh].map(
                                |source_option| {
                                    Button::new(if source_option == HistorySource::Bash {
                                        "inspector-source-bash"
                                    } else {
                                        "inspector-source-zsh"
                                    })
                                    .ghost()
                                    .small()
                                    .label(if source_option == HistorySource::Bash {
                                        "Bash"
                                    } else {
                                        "Zsh"
                                    })
                                    .selected(source == source_option)
                                    .accessibility_label(format!(
                                        "历史来源：{}",
                                        source_option.label()
                                    ))
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.set_source(source_option, window, cx)
                                        },
                                    ))
                                },
                            ))
                            .child(div().flex_1())
                            .child(
                                Button::new("inspector-read-history")
                                    .small()
                                    .label(if self.state.history.is_some() {
                                        "刷新"
                                    } else {
                                        "读取"
                                    })
                                    .disabled(
                                        !connected
                                            || self.state.status == ReadStatus::Loading
                                            || self.state.status == ReadStatus::Waiting,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(source.label()),
                    )
                    .child(
                        Input::new(&search)
                            .id("inspector-history-search")
                            .cleanable(true)
                            .w_full(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(
                        uniform_list(
                            "inspector-history-list",
                            count,
                            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                let Some(history) = &this.state.history else {
                                    return Vec::new();
                                };
                                if Some(history.id) != snapshot_id {
                                    return Vec::new();
                                }
                                range
                                    .filter_map(|position| {
                                        indices
                                            .get(position)
                                            .and_then(|index| history.snapshot.entries.get(*index))
                                    })
                                    .map(|entry| {
                                        let entry_id = entry.id;
                                        let row_target = target.clone();
                                        let snapshot_id = history.id;
                                        ListItem::new(("inspector-history-row", entry_id))
                                            .h(px(36.))
                                            .min_h(px(36.))
                                            .px_3()
                                            .py_0()
                                            .min_w_0()
                                            .selected(this.state.selected == Some(entry_id))
                                            .accessibility_label(format!(
                                                "历史记录，{}，仅选择并查看详情",
                                                entry.preview
                                            ))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .text_xs()
                                                    .font_family(
                                                        cx.theme().mono_font_family.clone(),
                                                    )
                                                    .truncate()
                                                    .child(entry.preview.clone()),
                                            )
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                if let Some(target) = &row_target {
                                                    this.select_history(
                                                        target,
                                                        snapshot_id,
                                                        entry_id,
                                                        window,
                                                        cx,
                                                    );
                                                }
                                            }))
                                            .into_any_element()
                                    })
                                    .collect()
                            }),
                        )
                        .track_scroll(&self.history_scroll)
                        .flex_1()
                        .min_h_0()
                        .w_full(),
                    )
                    .when(count == 0, |list| {
                        list.child(empty_content(
                            if self.state.history.is_none() {
                                "点击「读取」查看已保存的历史。不会记录输入，也不会执行命令。"
                            } else if self.state.query.is_empty() {
                                "历史文件为空。"
                            } else {
                                "没有匹配的历史记录。"
                            },
                            cx,
                        ))
                    }),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if truncated {
                        format!("{count} 条匹配 · 已截断，仅显示最近的完整记录")
                    } else {
                        format!("{count} 条匹配 · 最新记录在前")
                    }),
            );
        if let Some(history) = &self.state.history {
            if let Some(entry) = self
                .state
                .selected
                .and_then(|id| history.snapshot.entries.iter().find(|entry| entry.id == id))
            {
                let entry_id = entry.id;
                let snapshot_id = history.id;
                let copy_target = self.state.target.clone();
                let insert_target = copy_target.clone();
                let can_copy = entry.copy_text().is_some();
                let can_insert = entry.insertion_text().is_some();
                let preview = entry.preview.clone();
                body = body.child(
                    div()
                        .flex_shrink_0()
                        .min_w_0()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .id("inspector-history-preview")
                                .debug_selector(|| "inspector-history-preview".into())
                                .max_h(px(96.))
                                .min_w_0()
                                .overflow_y_scroll()
                                .text_xs()
                                .font_family(cx.theme().mono_font_family.clone())
                                .whitespace_normal()
                                .child(preview),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(history_safety_label(entry)),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    Button::new("inspector-copy-history")
                                        .small()
                                        .label("复制")
                                        .disabled(!can_copy)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(target) = &copy_target {
                                                this.copy_history(
                                                    target,
                                                    snapshot_id,
                                                    entry_id,
                                                    cx,
                                                );
                                            }
                                        })),
                                )
                                .child(
                                    Button::new("inspector-insert-history")
                                        .small()
                                        .label("填入终端")
                                        .disabled(!can_insert || !connected)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(target) = &insert_target {
                                                this.insert_history(
                                                    target,
                                                    snapshot_id,
                                                    entry_id,
                                                    cx,
                                                );
                                            }
                                        })),
                                ),
                        ),
                );
            }
        }
        body.child(div().px_3().pb_3().flex_shrink_0().text_xs().text_color(cx.theme().muted_foreground).child("仅默认已写盘文件；自定义 HISTFILE 和未保存命令不在范围内。填入前请先回到 shell 提示符，不会追加回车。"))
            .into_any_element()
    }
}

impl EventEmitter<InspectorAction> for InspectorView {}

impl Focusable for InspectorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for InspectorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.state.page.label();
        let target = self
            .state
            .target
            .as_ref()
            .map(|target| target.label.clone())
            .unwrap_or_else(|| "没有活动终端".into());
        let status = self.status_text();
        let connected = self
            .state
            .target
            .as_ref()
            .is_some_and(|target| target.connected);
        let content = match self.state.page {
            InspectorPage::System => self.system_page(cx),
            InspectorPage::Network => self.network_page(window, cx),
            InspectorPage::History => self.history_page(window, cx),
        };
        div()
            .id("inspector")
            .debug_selector(|| "app-inspector".into())
            .accessibility_id("inspector")
            .role(accesskit::Role::Group)
            .aria_label(format!("当前主机检查，{title}，{target}"))
            .key_context("Inspector")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .px_3()
                    .pt_3()
                    .pb_2()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                Button::new("inspector-back-terminal")
                                    .ghost()
                                    .xsmall()
                                    .label("返回终端")
                                    .accessibility_label("返回并聚焦当前终端")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(InspectorAction::FocusTerminal)
                                    })),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("inspector-close")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Close)
                                    .accessibility_label("关闭检查面板")
                                    .tooltip("关闭检查面板")
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(InspectorAction::Close)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("当前主机 · {title}")),
                            )
                            .child(div().flex_1())
                            .when(self.state.page != InspectorPage::History, |header| {
                                header.child(
                                    Button::new("inspector-refresh")
                                        .ghost()
                                        .xsmall()
                                        .label("刷新")
                                        .disabled(
                                            !connected
                                                || self.state.status == ReadStatus::Loading
                                                || self.state.status == ReadStatus::Waiting,
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                                )
                            }),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(target),
                    )
                    .child(
                        div().flex().gap_1().children(
                            [
                                InspectorPage::System,
                                InspectorPage::Network,
                                InspectorPage::History,
                            ]
                            .map(|page| {
                                Button::new(("inspector-page", page as usize))
                                    .ghost()
                                    .small()
                                    .label(page.label())
                                    .selected(self.state.page == page)
                                    .accessibility_label(format!("查看{}", page.label()))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.show_page(page, window, cx)
                                    }))
                            }),
                        ),
                    ),
            )
            .when_some(status, |view, status| {
                view.child(
                    div()
                        .px_3()
                        .pb_2()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(status),
                )
            })
            .child(content)
    }
}

fn empty_content(message: &str, cx: &Context<InspectorView>) -> AnyElement {
    div()
        .p_3()
        .min_w_0()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(message.to_owned())
        .into_any_element()
}

fn info_row(label: &str, value: &str, cx: &Context<InspectorView>) -> AnyElement {
    div()
        .flex()
        .items_start()
        .gap_3()
        .min_w_0()
        .child(
            div()
                .w(px(58.))
                .flex_shrink_0()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_owned()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .whitespace_normal()
                .child(value.to_owned()),
        )
        .into_any_element()
}

fn metric(
    id: &'static str,
    label: &str,
    value: String,
    percentage: Option<f32>,
    cx: &Context<InspectorView>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .min_w_0()
        .child(
            div()
                .flex()
                .justify_between()
                .gap_2()
                .text_xs()
                .child(label.to_owned())
                .child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(value),
                ),
        )
        .when_some(percentage, |metric, percentage| {
            metric.child(
                Progress::new(id)
                    .xsmall()
                    .value(percentage)
                    .color(cx.theme().primary)
                    .accessibility_label(label.to_owned()),
            )
        })
        .into_any_element()
}

fn network_header(cx: &Context<InspectorView>) -> AnyElement {
    div()
        .h(px(30.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .border_b_1()
        .border_color(cx.theme().border)
        .child(table_cell("协议", 64.))
        .child(table_cell("状态", 110.))
        .child(table_cell("本地数字地址", 220.))
        .child(table_cell("远端数字地址", 220.))
        .child(table_cell("进程", 140.))
        .child(table_cell("PID", 76.))
        .into_any_element()
}

fn table_cell(value: &str, width: f32) -> AnyElement {
    div()
        .w(px(width))
        .min_w(px(width))
        .flex_shrink_0()
        .px_2()
        .truncate()
        .child(value.to_owned())
        .into_any_element()
}

fn memory_percent(snapshot: &SystemSnapshot) -> Option<f32> {
    let (used, total) = snapshot
        .memory_used_bytes
        .zip(snapshot.memory_total_bytes)?;
    (total > 0 && used <= total).then(|| (100. * used as f64 / total as f64) as f32)
}

fn format_bytes(bytes: f64) -> String {
    if !bytes.is_finite() || bytes < 0. {
        return "未知".into();
    }
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024. && unit < units.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} {}", units[unit])
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

fn format_rate(rate: Option<f64>) -> String {
    rate.filter(|rate| rate.is_finite() && *rate >= 0.)
        .map(|rate| format!("{}/s", format_bytes(rate)))
        .unwrap_or_else(|| "预热中".into())
}

fn format_uptime(seconds: Option<f64>) -> String {
    let Some(seconds) = seconds.filter(|value| value.is_finite() && *value >= 0.) else {
        return "未知".into();
    };
    let seconds = seconds as u64;
    let days = seconds / 86_400;
    let hours = seconds % 86_400 / 3600;
    let minutes = seconds % 3600 / 60;
    if days > 0 {
        format!("{days} 天 {hours} 小时")
    } else {
        format!("{hours} 小时 {minutes} 分钟")
    }
}

fn history_safety_label(entry: &HistoryEntry) -> &'static str {
    if entry.copy_text().is_none() {
        "记录不完整或编码无效，不能复制或填入。"
    } else if entry.insertion_text().is_some() {
        "仅填入文本，不会执行。请先确认终端位于 shell 提示符。"
    } else if !entry.is_boundary_known() {
        "普通 Bash 历史无法确定多行边界，仅可复制。"
    } else {
        "多行或含控制字符，仅可复制；预览中的异常字符已转义。"
    }
}

fn error_label(error: InspectionError) -> &'static str {
    match error {
        InspectionError::Unavailable => "此会话暂不可检查，可在连接可用后手动刷新。",
        InspectionError::Unsupported => "此系统不受支持；首版支持本地 macOS 与远程 Linux。",
        InspectionError::ToolUnavailable => "所需只读工具不可用，请检查目标系统的工具配置。",
        InspectionError::NotFound => "默认历史文件不存在；请确认所选 shell 已保存历史。",
        InspectionError::PermissionDenied => "当前连接账户没有读取权限，不会尝试提权。",
        InspectionError::UnsafeFile => "历史来源不是安全的普通文件，已停止读取。",
        InspectionError::InvalidData => "返回格式无法识别，已停止自动刷新。",
        InspectionError::OutputLimit => "读取超出数据上限，请稍后手动刷新。",
        InspectionError::Rejected => "目标拒绝了只读检查，交互终端不受影响。",
        InspectionError::Failed => "读取失败，可刷新重试。",
        InspectionError::Timeout => "读取超时，正在等待后台清理。",
        InspectionError::Cancelled => "读取已取消，可手动刷新。",
        InspectionError::Busy => "上次读取仍在处理，等待后台就绪。",
        InspectionError::Cleaning => "上次读取尚未清理完成，自动刷新已停止。",
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::{
        format_bytes, format_rate, format_uptime, memory_percent, InspectionTarget,
        InspectorAction, InspectorPage, InspectorState, InspectorView, ReadStatus,
    };
    use crate::backend::{AppCommand, BackendEvent};
    use crate::inspection::history::parse_history;
    use crate::inspection::system::parse_linux_system;
    use crate::inspection::{
        HistorySource, InspectionData, InspectionError, InspectionKind, SystemSnapshot,
    };
    use gpui_kit::{AppContext, Focusable, KeyDownEvent, Keystroke, TestAppContext};
    use std::time::Duration;
    use tokio::sync::mpsc;

    fn target(name: &str) -> InspectionTarget {
        InspectionTarget {
            tab_id: format!("tab-{name}"),
            session_id: format!("session-{name}"),
            label: format!("Synthetic {name}"),
            remote: true,
            connected: true,
        }
    }

    fn fixture() -> (InspectorState, mpsc::UnboundedReceiver<AppCommand>) {
        let (commands, received) = mpsc::unbounded_channel();
        (InspectorState::new(commands), received)
    }

    fn next_request(
        commands: &mut mpsc::UnboundedReceiver<AppCommand>,
    ) -> (String, u64, InspectionKind) {
        match commands.try_recv().expect("inspection command") {
            AppCommand::Inspect {
                session_id,
                request_id,
                kind,
            } => (session_id, request_id, kind),
            command => panic!("expected Inspect, got {command:?}"),
        }
    }

    fn expect_cancel(commands: &mut mpsc::UnboundedReceiver<AppCommand>, session: &str, id: u64) {
        assert!(
            matches!(commands.try_recv(), Ok(AppCommand::CancelInspection { session_id, request_id }) if session_id == session && request_id == id)
        );
    }

    fn assert_empty(commands: &mut mpsc::UnboundedReceiver<AppCommand>) {
        assert!(commands.try_recv().is_err());
    }

    fn result(
        session: &str,
        id: u64,
        data: Result<InspectionData, InspectionError>,
    ) -> BackendEvent {
        BackendEvent::InspectionResult {
            session_id: session.into(),
            request_id: id,
            result: data,
        }
    }

    fn ready(state: &mut InspectorState, session: &str) {
        state.handle_event(&BackendEvent::InspectionReady {
            session_id: session.into(),
        });
    }

    fn linux(time: u64, total: u64) -> InspectionData {
        let data = format!("RSTERM_SYSTEM_1\nLinux\naarch64\n@@stat\ncpu {total} 0 0 20\ncpu0 0\n@@uptime\n{time} 0\n@@mem\nMemTotal: 100 kB\nMemAvailable: 40 kB\n@@net\neth0: {total} 0 0 0 0 0 0 0 {total} 0 0 0 0 0 0 0\n");
        InspectionData::System(parse_linux_system(data.as_bytes()).unwrap())
    }

    #[test]
    fn construction_hidden_targets_and_history_opening_never_collect() {
        let (mut state, mut commands) = fixture();
        assert_empty(&mut commands);
        state.set_target(Some(target("a")));
        state.set_page(InspectorPage::Network);
        state.set_page(InspectorPage::History);
        assert_empty(&mut commands);
        state.set_visible(true);
        assert_empty(&mut commands);
        state.set_target(Some(target("b")));
        state.set_source(HistorySource::Bash);
        assert_empty(&mut commands);
        state.refresh();
        let (session, id, kind) = next_request(&mut commands);
        assert_eq!(session, "session-b");
        assert_eq!(kind, InspectionKind::History(HistorySource::Bash));
        state.set_visible(false);
        expect_cancel(&mut commands, &session, id);
        state.refresh();
        ready(&mut state, &session);
        assert_empty(&mut commands);
    }

    #[test]
    fn same_session_page_switch_waits_for_readiness_and_keeps_only_latest_intent() {
        let (mut state, mut commands) = fixture();
        state.set_target(Some(target("a")));
        state.set_visible(true);
        let (session, id, _) = next_request(&mut commands);
        state.set_page(InspectorPage::Network);
        expect_cancel(&mut commands, &session, id);
        assert_eq!(state.status, ReadStatus::Waiting);
        state.set_page(InspectorPage::System);
        state.set_page(InspectorPage::Network);
        assert_empty(&mut commands);
        assert!(state
            .handle_event(&result(&session, id, Ok(linux(1, 80))))
            .is_none());
        assert!(state.system.is_none());
        ready(&mut state, &session);
        let (_, latest, kind) = next_request(&mut commands);
        assert_ne!(latest, id);
        assert_eq!(kind, InspectionKind::Network);
        assert_empty(&mut commands);
        ready(&mut state, &session);
        assert_empty(&mut commands);
    }

    #[test]
    fn finished_requests_poll_only_after_completion_and_backend_readiness() {
        let (mut state, mut commands) = fixture();
        state.set_target(Some(target("a")));
        state.set_visible(true);
        let (session, first, _) = next_request(&mut commands);
        assert_eq!(
            state.handle_event(&result(&session, first, Ok(linux(1, 80)))),
            Some(Duration::from_secs(2))
        );
        assert_eq!(state.system.as_ref().unwrap().cpu_usage_percent, None);
        state.request_automatic();
        assert_empty(&mut commands);
        assert_eq!(state.status, ReadStatus::Waiting);
        ready(&mut state, &session);
        let (_, second, _) = next_request(&mut commands);
        assert_eq!(
            state.handle_event(&result(&session, second, Ok(linux(3, 180)))),
            Some(Duration::from_secs(2))
        );
        assert_eq!(state.system.as_ref().unwrap().cpu_usage_percent, Some(100.));
        assert_eq!(state.cpu_points.len(), 1);
        assert_empty(&mut commands);
    }

    #[test]
    fn target_disconnect_hide_and_reconnect_drop_data_and_reject_late_results() {
        let (mut state, mut commands) = fixture();
        state.set_target(Some(target("a")));
        state.set_visible(true);
        let (session_a, first, _) = next_request(&mut commands);
        state.set_target(Some(target("b")));
        expect_cancel(&mut commands, &session_a, first);
        let (session_b, second, _) = next_request(&mut commands);
        state.handle_event(&result(&session_a, first, Ok(linux(1, 80))));
        assert!(state.system.is_none());
        assert!(state.blocked.contains(&session_a));
        state.handle_event(&BackendEvent::SessionFailed {
            session_id: session_a.clone(),
            error: "synthetic closed session".into(),
            retryable: false,
        });
        assert!(!state.blocked.contains(&session_a));
        assert!(state.target.as_ref().unwrap().connected);
        state.handle_event(&result(&session_b, second, Ok(linux(1, 80))));
        assert!(state.system.is_some());
        state.handle_event(&BackendEvent::SessionClosed {
            session_id: session_b.clone(),
            reason: None,
            retryable: false,
        });
        assert!(state.system.is_none());
        assert!(!state.target.as_ref().unwrap().connected);
        state.refresh();
        assert_empty(&mut commands);
        let mut reconnected = target("b");
        reconnected.session_id = "new-session-b".into();
        state.set_target(Some(reconnected));
        let (new_session, third, _) = next_request(&mut commands);
        state.set_visible(false);
        expect_cancel(&mut commands, &new_session, third);
        state.handle_event(&result(&new_session, third, Ok(linux(3, 180))));
        assert!(state.system.is_none());
    }

    #[test]
    fn busy_never_retries_until_ready_and_fatal_errors_stop_automatic_requests() {
        let (mut state, mut commands) = fixture();
        state.set_target(Some(target("a")));
        state.set_visible(true);
        let (session, first, _) = next_request(&mut commands);
        assert!(state
            .handle_event(&result(&session, first, Err(InspectionError::Busy)))
            .is_none());
        state.request_automatic();
        state.request_automatic();
        assert_empty(&mut commands);
        ready(&mut state, &session);
        let (_, second, _) = next_request(&mut commands);
        assert!(state
            .handle_event(&result(&session, second, Err(InspectionError::Unsupported)))
            .is_none());
        ready(&mut state, &session);
        state.request_automatic();
        assert_empty(&mut commands);
        assert!(state.halted);
        state.refresh();
        let (_, third, _) = next_request(&mut commands);
        assert_ne!(second, third);
    }

    #[test]
    fn cleanup_watchdog_stops_without_sending_retry_and_ready_does_not_restart_it() {
        let (mut state, mut commands) = fixture();
        state.set_target(Some(target("a")));
        state.set_visible(true);
        let (session, id, _) = next_request(&mut commands);
        state.handle_event(&result(&session, id, Err(InspectionError::Busy)));
        state.expire_wait();
        assert_eq!(state.status, ReadStatus::Failed(InspectionError::Cleaning));
        assert!(state.pending.is_none());
        assert!(state.halted);
        ready(&mut state, &session);
        state.request_automatic();
        assert_empty(&mut commands);
        state.refresh();
        next_request(&mut commands);
    }

    #[test]
    fn temporary_failure_marks_existing_data_stale_and_preserves_manual_refresh() {
        let (mut state, mut commands) = fixture();
        state.set_target(Some(target("a")));
        state.set_visible(true);
        let (session, first, _) = next_request(&mut commands);
        state.handle_event(&result(&session, first, Ok(linux(1, 80))));
        ready(&mut state, &session);
        state.request_automatic();
        let (_, second, _) = next_request(&mut commands);
        assert_eq!(
            state.handle_event(&result(&session, second, Err(InspectionError::Timeout))),
            Some(Duration::from_secs(2))
        );
        assert!(state.stale);
        assert!(state.system.is_some());
        assert!(!state.halted);
        state.refresh();
        assert_empty(&mut commands);
        ready(&mut state, &session);
        next_request(&mut commands);
    }

    #[test]
    fn source_target_and_snapshot_identity_guard_history_actions() {
        let (mut state, mut commands) = fixture();
        let target_a = target("a");
        state.set_target(Some(target_a.clone()));
        state.set_page(InspectorPage::History);
        state.set_visible(true);
        state.refresh();
        let (session, first, _) = next_request(&mut commands);
        let history = parse_history(
            HistorySource::Zsh,
            b": 100:0;echo safe\n: 200:0;echo multi\\\nline\n",
            false,
        );
        let safe_id = history
            .entries
            .iter()
            .find(|entry| entry.insertion_text().is_some())
            .unwrap()
            .id;
        let multi_id = history
            .entries
            .iter()
            .find(|entry| entry.insertion_text().is_none())
            .unwrap()
            .id;
        assert!(state
            .handle_event(&result(
                &session,
                first,
                Ok(InspectionData::History(history.clone()))
            ))
            .is_none());
        assert_eq!(
            state.insertion_text(&target_a, first, safe_id),
            Some("echo safe".into())
        );
        assert_eq!(state.insertion_text(&target_a, first, multi_id), None);
        assert_eq!(state.insertion_text(&target("b"), first, safe_id), None);
        assert_eq!(state.insertion_text(&target_a, first + 1, safe_id), None);
        state.query = "safe".into();
        assert_eq!(state.filtered_history().len(), 1);
        state.refresh();
        assert!(state.history.is_none());
        assert_eq!(state.insertion_text(&target_a, first, safe_id), None);
        ready(&mut state, &session);
        let (_, second, _) = next_request(&mut commands);
        state.set_source(HistorySource::Bash);
        expect_cancel(&mut commands, &session, second);
        state.handle_event(&result(
            &session,
            second,
            Ok(InspectionData::History(history)),
        ));
        assert!(state.history.is_none());
        assert!(state.query.is_empty());
        ready(&mut state, &session);
        assert_empty(&mut commands);
        state.set_target(Some(target("b")));
        assert_empty(&mut commands);
    }

    #[test]
    fn history_release_and_wrong_source_results_never_retain_text() {
        let (mut state, mut commands) = fixture();
        state.set_target(Some(target("a")));
        state.set_page(InspectorPage::History);
        state.set_visible(true);
        state.refresh();
        let (session, id, _) = next_request(&mut commands);
        state.handle_event(&result(
            &session,
            id,
            Ok(InspectionData::History(parse_history(
                HistorySource::Bash,
                b"echo wrong\n",
                false,
            ))),
        ));
        assert_eq!(
            state.status,
            ReadStatus::Failed(InspectionError::InvalidData)
        );
        assert!(state.history.is_none());
        ready(&mut state, &session);
        state.refresh();
        let (_, id, _) = next_request(&mut commands);
        state.handle_event(&result(
            &session,
            id,
            Ok(InspectionData::History(parse_history(
                HistorySource::Zsh,
                b"echo fixture\n",
                false,
            ))),
        ));
        state.query = "fixture".into();
        state.set_page(InspectorPage::Network);
        assert!(state.history.is_none());
        assert!(state.query.is_empty());
    }

    #[test]
    fn numeric_fields_preserve_unknown_and_first_sample_never_looks_idle() {
        assert_eq!(memory_percent(&SystemSnapshot::default()), None);
        assert_eq!(
            memory_percent(&SystemSnapshot {
                memory_used_bytes: Some(20),
                memory_total_bytes: Some(0),
                ..Default::default()
            }),
            None
        );
        assert_eq!(format_rate(None), "预热中");
        assert_eq!(format_rate(Some(0.)), "0 B/s");
        assert_eq!(format_bytes(f64::NAN), "未知");
        assert_eq!(format_uptime(None), "未知");
        assert_eq!(format_uptime(Some(90_000.)), "1 天 1 小时");
    }

    #[gpui::test]
    fn polling_timers_stop_on_hide_and_do_not_queue_during_inflight(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (commands, mut received) = mpsc::unbounded_channel();
        let (window, inspector) = cx.update(|cx| {
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| InspectorView::new(commands, window, cx))
            })
            .unwrap()
        });
        cx.update_window(window, |_, _, cx| {
            inspector.update(cx, |view, cx| {
                view.set_target(Some(target("timer")), cx);
                view.set_visible(true, cx);
            })
        })
        .unwrap();
        let (session, first, _) = next_request(&mut received);
        cx.executor().advance_clock(Duration::from_secs(30));
        cx.run_until_parked();
        assert_empty(&mut received);
        cx.update_window(window, |_, _, cx| {
            inspector.update(cx, |view, cx| {
                view.handle_event(&result(&session, first, Ok(linux(1, 80))), cx);
                view.handle_event(
                    &BackendEvent::InspectionReady {
                        session_id: session.clone(),
                    },
                    cx,
                );
            })
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(1999));
        cx.run_until_parked();
        assert_empty(&mut received);
        cx.executor().advance_clock(Duration::from_millis(1));
        cx.run_until_parked();
        let (_, second, _) = next_request(&mut received);
        cx.executor().advance_clock(Duration::from_secs(20));
        cx.run_until_parked();
        assert_empty(&mut received);
        cx.update_window(window, |_, _, cx| {
            inspector.update(cx, |view, cx| {
                view.handle_event(&result(&session, second, Ok(linux(3, 180))), cx);
                view.handle_event(
                    &BackendEvent::InspectionReady {
                        session_id: session.clone(),
                    },
                    cx,
                );
                view.set_visible(false, cx);
            })
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(30));
        cx.run_until_parked();
        assert_empty(&mut received);
        cx.update_window(window, |_, _, cx| {
            inspector.update(cx, |view, _| {
                assert!(view.poll_task.is_none());
                assert!(view.wait_task.is_none());
                assert!(view.state.system.is_none());
            })
        })
        .unwrap();
    }

    #[gpui::test]
    fn bounded_network_table_and_history_updates_keep_focus(cx: &mut TestAppContext) {
        use crate::inspection::{ConnectionRow, NetworkSnapshot};
        use gpui_kit::{px, size};

        cx.update(gpui_kit::init);
        let (commands, mut received) = mpsc::unbounded_channel();
        let (inspector, cx) =
            cx.add_window_view(move |window, cx| InspectorView::new(commands, window, cx));
        cx.simulate_resize(size(px(280.), px(600.)));
        cx.update(|window, cx| {
            inspector.update(cx, |view, cx| {
                view.set_target(Some(target("layout")), cx);
                view.show_page(InspectorPage::Network, window, cx);
                view.set_visible(true, cx);
                let (session, id, _) = next_request(&mut received);
                let snapshot = NetworkSnapshot {
                    connections: (0..1000)
                        .map(|index| ConnectionRow {
                            protocol: "TCP".into(),
                            state: "ESTABLISHED".into(),
                            local_address: format!("[2001:db8::1]:{index}"),
                            remote_address: "[2001:db8::2]:443".into(),
                            process: Some("synthetic-process".into()),
                            pid: Some(42),
                        })
                        .collect(),
                    ..Default::default()
                };
                view.handle_event(
                    &result(&session, id, Ok(InspectionData::Network(snapshot))),
                    cx,
                );
            });
            window.draw(cx).clear(cx);
        });
        let panel = cx.debug_bounds("app-inspector").expect("panel bounds");
        let table = cx
            .debug_bounds("inspector-network-horizontal")
            .expect("table viewport bounds");
        assert!(panel.size.width <= px(280.));
        assert!(table.size.width <= panel.size.width);
        assert!(table.size.height > px(100.));
        assert!(table.right() <= panel.right());
        cx.update(|window, cx| {
            inspector.update(cx, |view, cx| {
                view.show_page(InspectorPage::History, window, cx);
                let search = view.search.as_ref().unwrap().clone();
                let search_focus = search.focus_handle(cx);
                assert!(search_focus.is_focused(window));
                view.refresh(cx);
                view.handle_event(
                    &BackendEvent::InspectionReady {
                        session_id: "session-layout".into(),
                    },
                    cx,
                );
                let (session, id, _) = next_request(&mut received);
                view.handle_event(
                    &result(
                        &session,
                        id,
                        Ok(InspectionData::History(parse_history(
                            HistorySource::Zsh,
                            b"echo synthetic\n",
                            false,
                        ))),
                    ),
                    cx,
                );
                assert!(
                    search_focus.is_focused(window),
                    "data updates must not steal search focus"
                );
                view.on_key_down(
                    &KeyDownEvent {
                        keystroke: Keystroke::parse("enter").unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
                assert!(
                    view.state.selected.is_none(),
                    "input Enter must not select history rows"
                );
                search.update(cx, |search, cx| search.set_value("echo", window, cx));
                view.state.query = "echo".into();
                view.set_visible(false, cx);
                assert!(view.state.query.is_empty());
                assert!(view.search.is_none());
                assert!(view.poll_task.is_none());
                assert!(view.wait_task.is_none());
            });
            window.draw(cx).clear(cx);
        });
        assert_empty(&mut received);
    }

    #[gpui::test]
    fn history_selection_copy_and_explicit_insert_are_separate_actions(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (commands, mut received) = mpsc::unbounded_channel();
        let (window, inspector) = cx.update(|cx| {
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| InspectorView::new(commands, window, cx))
            })
            .unwrap()
        });
        let actions = Rc::new(RefCell::new(Vec::new()));
        let collected = actions.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&inspector, move |_, action: &InspectorAction, _| {
                collected.borrow_mut().push(action.clone())
            })
        });
        let target = target("ui");
        let mut snapshot_id = 0;
        let mut entry_id = 0;
        cx.update_window(window, |_, window, cx| {
            inspector.update(cx, |view, cx| {
                view.set_target(Some(target.clone()), cx);
                view.show_page(InspectorPage::History, window, cx);
                view.set_visible(true, cx);
                view.show_page(InspectorPage::History, window, cx);
                assert_empty(&mut received);
                view.refresh(cx);
                let (session, id, _) = next_request(&mut received);
                snapshot_id = id;
                let history = parse_history(HistorySource::Zsh, b": 100:0;echo fixture\n", false);
                entry_id = history.entries[0].id;
                view.handle_event(
                    &result(&session, id, Ok(InspectionData::History(history))),
                    cx,
                );
                view.select_history(&target, id, entry_id, window, cx);
                view.on_key_down(
                    &KeyDownEvent {
                        keystroke: Keystroke::parse("enter").unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
                view.select_history(&target, id, entry_id, window, cx);
                view.copy_history(&target, id, entry_id, cx);
                assert_eq!(
                    cx.read_from_clipboard().and_then(|item| item.text()),
                    Some("echo fixture".into())
                );
                assert_empty(&mut received);
                assert!(actions.borrow().is_empty());
            })
        })
        .unwrap();
        cx.run_until_parked();
        assert!(actions.borrow().is_empty());
        cx.update_window(window, |_, _, cx| {
            inspector.update(cx, |view, cx| {
                view.insert_history(&target, snapshot_id, entry_id, cx);
                assert_empty(&mut received);
            })
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(
            *actions.borrow(),
            vec![InspectorAction::InsertHistory {
                target: target.clone(),
                snapshot_id,
                entry_id
            }]
        );
        cx.update_window(window, |_, _, cx| {
            inspector.update(cx, |view, cx| {
                view.refresh(cx);
                view.insert_history(&target, snapshot_id, entry_id, cx);
                view.set_visible(false, cx);
                assert!(view.search.is_none());
                assert!(view.poll_task.is_none());
                assert!(view.state.history.is_none());
            })
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(actions.borrow().len(), 1);
    }
}
