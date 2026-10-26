use std::future::pending;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use russh::{Channel, ChannelMsg};
#[cfg(target_os = "macos")]
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{BackendEvent, RequestId, SshTransport};
use crate::inspection::history::{parse_history, MAX_HISTORY_BYTES};
#[cfg(target_os = "macos")]
use crate::inspection::network::parse_lsof;
use crate::inspection::network::parse_ss;
use crate::inspection::system::parse_linux_system;
use crate::inspection::{HistorySource, InspectionData, InspectionError, InspectionKind};

const STDERR_LIMIT: usize = 16 * 1024;
const HEADER_LIMIT: usize = 128;

struct ActiveInspection {
    request_id: RequestId,
    cancel: CancellationToken,
    cleaning: Arc<AtomicBool>,
    task: JoinHandle<()>,
}

pub(super) struct InspectionSlot {
    active: Option<ActiveInspection>,
    completed_tx: mpsc::Sender<RequestId>,
    completed_rx: mpsc::Receiver<RequestId>,
    local_system: LocalSystemState,
}

#[cfg(target_os = "macos")]
type LocalSystemState = Arc<parking_lot::Mutex<Option<(sysinfo::System, std::time::Instant)>>>;
#[cfg(not(target_os = "macos"))]
type LocalSystemState = ();

impl Default for InspectionSlot {
    fn default() -> Self {
        let (completed_tx, completed_rx) = mpsc::channel(1);
        Self {
            active: None,
            completed_tx,
            completed_rx,
            local_system: Default::default(),
        }
    }
}

impl InspectionSlot {
    pub async fn start(
        &mut self,
        session_id: String,
        request_id: RequestId,
        kind: InspectionKind,
        transport: Option<Arc<SshTransport>>,
        events: mpsc::Sender<BackendEvent>,
        session_cancel: CancellationToken,
    ) {
        if let Some(active) = &self.active {
            let error = if active.cleaning.load(Ordering::Acquire) {
                InspectionError::Cleaning
            } else {
                InspectionError::Busy
            };
            tokio::select! {
                biased;
                _ = session_cancel.cancelled() => {},
                _ = events.send(BackendEvent::InspectionResult { session_id, request_id, result: Err(error) }) => {},
            }
            return;
        }
        let request_cancel = session_cancel.child_token();
        let cleaning = Arc::new(AtomicBool::new(false));
        let token = request_cancel.clone();
        let state = cleaning.clone();
        let completed = self.completed_tx.clone();
        let local_system = self.local_system.clone();
        let task = tokio::spawn(async move {
            match transport {
                Some(transport) => {
                    run_remote(
                        transport,
                        session_id,
                        request_id,
                        kind,
                        events,
                        token,
                        session_cancel,
                        state,
                    )
                    .await
                }
                None => {
                    run_local(
                        session_id,
                        request_id,
                        kind,
                        events,
                        token,
                        session_cancel,
                        state,
                        local_system,
                    )
                    .await
                }
            }
            let _ = completed.send(request_id).await;
        });
        self.active = Some(ActiveInspection {
            request_id,
            cancel: request_cancel,
            cleaning,
            task,
        });
    }

    pub fn cancel(&self, request_id: RequestId) {
        if let Some(active) = &self.active {
            if active.request_id == request_id {
                active.cancel.cancel();
            }
        }
    }

    pub async fn completed(&mut self) {
        if self.active.is_none() {
            pending::<()>().await;
        }
        let Some(request_id) = self.completed_rx.recv().await else {
            return;
        };
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.request_id == request_id)
        {
            let active = self.active.take().unwrap();
            let _ = active.task.await;
        }
    }

    pub async fn shutdown(&mut self) {
        if let Some(active) = self.active.take() {
            active.cancel.cancel();
            let _ = active.task.await;
        }
    }
}

fn deadline(kind: InspectionKind) -> Duration {
    match kind {
        InspectionKind::History(_) => Duration::from_secs(10),
        _ => Duration::from_secs(5),
    }
}

fn stdout_limit(kind: InspectionKind) -> usize {
    match kind {
        InspectionKind::System => 256 * 1024,
        InspectionKind::Network => 2 * 1024 * 1024,
        InspectionKind::History(_) => MAX_HISTORY_BYTES + 1 + HEADER_LIMIT,
    }
}

fn deliver(
    events: &mpsc::Sender<BackendEvent>,
    session_id: &str,
    request_id: RequestId,
    result: Result<InspectionData, InspectionError>,
    session_cancel: &CancellationToken,
) -> JoinHandle<()> {
    let events = events.clone();
    let event = BackendEvent::InspectionResult {
        session_id: session_id.to_owned(),
        request_id,
        result,
    };
    let cancel = session_cancel.clone();
    tokio::spawn(async move {
        tokio::select! { biased; _ = cancel.cancelled() => {}, _ = events.send(event) => {} }
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_remote(
    transport: Arc<SshTransport>,
    session_id: String,
    request_id: RequestId,
    kind: InspectionKind,
    events: mpsc::Sender<BackendEvent>,
    request_cancel: CancellationToken,
    session_cancel: CancellationToken,
    cleaning: Arc<AtomicBool>,
) {
    let timeout = tokio::time::sleep(deadline(kind));
    tokio::pin!(timeout);
    let opening = transport.open_inspection_channel();
    tokio::pin!(opening);
    let mut delivery = None;
    let mut channel = loop {
        tokio::select! {
            biased;
            _ = session_cancel.cancelled() => {
                if let Some(delivery) = delivery { let _ = delivery.await; }
                return;
            },
            _ = request_cancel.cancelled(), if delivery.is_none() => {
                cleaning.store(true, Ordering::Release);
                delivery = Some(deliver(&events, &session_id, request_id, Err(InspectionError::Cancelled), &session_cancel));
            }
            _ = &mut timeout, if delivery.is_none() => {
                cleaning.store(true, Ordering::Release);
                delivery = Some(deliver(&events, &session_id, request_id, Err(InspectionError::Timeout), &session_cancel));
            }
            result = &mut opening => match result {
                Ok(channel) => break channel,
                Err(_) => {
                    if delivery.is_none() { delivery = Some(deliver(&events, &session_id, request_id, Err(InspectionError::Unavailable), &session_cancel)); }
                    if let Some(delivery) = delivery { let _ = delivery.await; }
                    return;
                }
            }
        }
    };
    let mut closed = false;
    if delivery.is_none() {
        if let Err((error, peer_closed)) = enqueue_exec(
            &mut channel,
            remote_command(kind),
            &request_cancel,
            &session_cancel,
        )
        .await
        {
            closed = peer_closed;
            cleaning.store(true, Ordering::Release);
            delivery = Some(deliver(
                &events,
                &session_id,
                request_id,
                Err(error),
                &session_cancel,
            ));
        }
    }
    let mut stdout = Vec::new();
    let mut stderr_bytes = 0_usize;
    let mut accepted = false;
    let mut status = None;
    let mut eof = false;
    while delivery.is_none() {
        let result = tokio::select! {
            biased;
            _ = session_cancel.cancelled() => return,
            _ = request_cancel.cancelled() => Some(Err(InspectionError::Cancelled)),
            _ = &mut timeout => Some(Err(InspectionError::Timeout)),
            message = channel.wait() => match message {
                Some(ChannelMsg::Success) => { accepted = true; None }
                Some(ChannelMsg::Failure | ChannelMsg::OpenFailure(_)) => Some(Err(InspectionError::Rejected)),
                Some(ChannelMsg::Data { data }) => {
                    if stdout.len().saturating_add(data.len()) > stdout_limit(kind) { Some(Err(InspectionError::OutputLimit)) }
                    else { stdout.extend_from_slice(&data); None }
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    stderr_bytes = stderr_bytes.saturating_add(data.len());
                    (stderr_bytes > STDERR_LIMIT).then_some(Err(InspectionError::OutputLimit))
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => { status = Some(exit_status); None }
                Some(ChannelMsg::ExitSignal { .. }) => Some(Err(InspectionError::Failed)),
                Some(ChannelMsg::Eof) => { eof = true; None }
                Some(ChannelMsg::Close) | None => { closed = true; Some(if accepted { decode_remote(kind, &stdout, status) } else { Err(InspectionError::Rejected) }) }
                _ => None,
            }
        };
        let result = result.or_else(|| {
            (accepted && eof && status.is_some()).then(|| decode_remote(kind, &stdout, status))
        });
        if let Some(result) = result {
            cleaning.store(true, Ordering::Release);
            delivery = Some(deliver(
                &events,
                &session_id,
                request_id,
                result,
                &session_cancel,
            ));
        }
    }
    stdout.clear();
    if !closed {
        closed = enqueue_close(&mut channel, &session_cancel).await;
    }
    if !closed {
        loop {
            tokio::select! {
                biased;
                _ = session_cancel.cancelled() => break,
                message = channel.wait() => if matches!(message, Some(ChannelMsg::Close) | None) { break; },
            }
        }
    }
    if let Some(delivery) = delivery {
        let _ = delivery.await;
    }
}

async fn enqueue_exec(
    channel: &mut Channel<russh::client::Msg>,
    command: String,
    request_cancel: &CancellationToken,
    session_cancel: &CancellationToken,
) -> Result<(), (InspectionError, bool)> {
    for _ in 0..8 {
        if request_cancel.is_cancelled() || session_cancel.is_cancelled() {
            return Err((InspectionError::Cancelled, false));
        }
        // Pending is not enqueued: each attempt releases the receiver so incoming data can be drained.
        match channel.exec(true, command.clone()).now_or_never() {
            Some(Ok(())) => return Ok(()),
            Some(Err(_)) => return Err((InspectionError::Unavailable, true)),
            None => {}
        }
        let pause = tokio::time::sleep(Duration::from_millis(10));
        tokio::pin!(pause);
        loop {
            tokio::select! {
                biased;
                _ = session_cancel.cancelled() => return Err((InspectionError::Cancelled, false)),
                _ = request_cancel.cancelled() => return Err((InspectionError::Cancelled, false)),
                _ = &mut pause => break,
                message = channel.wait() => match message {
                    Some(ChannelMsg::Close) | None => return Err((InspectionError::Unavailable, true)),
                    Some(ChannelMsg::Data { .. } | ChannelMsg::ExtendedData { .. }) => return Err((InspectionError::InvalidData, false)),
                    _ => {},
                }
            }
        }
    }
    Err((InspectionError::Cleaning, false))
}

async fn enqueue_close(
    channel: &mut Channel<russh::client::Msg>,
    session_cancel: &CancellationToken,
) -> bool {
    for _ in 0..8 {
        if session_cancel.is_cancelled() {
            return false;
        }
        match channel.close().now_or_never() {
            Some(Ok(())) => return false,
            Some(Err(_)) => return true,
            None => {}
        }
        let pause = tokio::time::sleep(Duration::from_millis(10));
        tokio::pin!(pause);
        loop {
            tokio::select! {
                biased;
                _ = session_cancel.cancelled() => return false,
                _ = &mut pause => break,
                message = channel.wait() => if matches!(message, Some(ChannelMsg::Close) | None) { return true; },
            }
        }
    }
    false
}

fn decode_remote(
    kind: InspectionKind,
    bytes: &[u8],
    status: Option<u32>,
) -> Result<InspectionData, InspectionError> {
    match status {
        Some(0) => {}
        Some(66) => return Err(InspectionError::NotFound),
        Some(69 | 127) => return Err(InspectionError::ToolUnavailable),
        Some(73) => return Err(InspectionError::UnsafeFile),
        Some(77) => return Err(InspectionError::PermissionDenied),
        Some(78) => return Err(InspectionError::Unsupported),
        _ => return Err(InspectionError::Failed),
    }
    match kind {
        InspectionKind::System => parse_linux_system(bytes).map(InspectionData::System),
        InspectionKind::Network => parse_ss(bytes).map(InspectionData::Network),
        InspectionKind::History(source) => {
            let newline = bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .filter(|index| *index < HEADER_LIMIT)
                .ok_or(InspectionError::InvalidData)?;
            if &bytes[..newline] != b"RSTERM_HISTORY_1" {
                return Err(InspectionError::InvalidData);
            }
            let body = &bytes[newline + 1..];
            Ok(InspectionData::History(parse_history(
                source,
                body,
                body.len() > MAX_HISTORY_BYTES,
            )))
        }
    }
}

fn remote_command(kind: InspectionKind) -> String {
    let script = match kind {
        InspectionKind::System => "[ \"$(uname -s)\" = Linux ] || exit 78; printf \"RSTERM_SYSTEM_1\\nLinux\\n\"; uname -m; printf \"@@os\\n\"; head -c 16384 /etc/os-release 2>/dev/null; printf \"\\n@@stat\\n\"; head -c 65536 /proc/stat 2>/dev/null; printf \"\\n@@mem\\n\"; head -c 16384 /proc/meminfo 2>/dev/null; printf \"\\n@@uptime\\n\"; head -c 256 /proc/uptime 2>/dev/null; printf \"\\n@@cpu\\n\"; head -c 65536 /proc/cpuinfo 2>/dev/null; printf \"\\n@@net\\n\"; head -c 65536 /proc/net/dev 2>/dev/null; printf \"\\n\"".to_owned(),
        InspectionKind::Network => "[ \"$(uname -s)\" = Linux ] || exit 78; command -v ss >/dev/null 2>&1 || exit 69; printf \"RSTERM_NETWORK_1\\n\"; ss -H -n -t -u -a -p".to_owned(),
        InspectionKind::History(source) => format!(r#"[ "$(uname -s)" = Linux ] || exit 78; command -v python3 >/dev/null 2>&1 || exit 69; python3 -I -S - "{}" << "RSTERM_PY"
import errno, os, stat, sys
try:
    fd = os.open(os.path.join(os.environ["HOME"], sys.argv[1]), os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    info = os.fstat(fd)
    if not stat.S_ISREG(info.st_mode):
        sys.exit(73)
    os.lseek(fd, max(0, info.st_size - 2097153), os.SEEK_SET)
    body = bytearray()
    expected = min(info.st_size, 2097153)
    while len(body) < expected:
        block = os.read(fd, min(8192, expected - len(body)))
        if not block:
            break
        body.extend(block)
    os.close(fd)
    if len(body) != expected:
        sys.exit(74)
    sys.stdout.buffer.write(b"RSTERM_HISTORY_1\n")
    sys.stdout.buffer.write(body)
except OSError as error:
    sys.exit(66 if error.errno == errno.ENOENT else 73 if error.errno == errno.ELOOP else 77)
RSTERM_PY"#, source.filename()),
    };
    format!("/bin/sh -c 'PATH=/usr/bin:/bin:/usr/sbin:/sbin; export PATH; LC_ALL=C; export LC_ALL; {script}'")
}

#[allow(clippy::too_many_arguments)]
async fn run_local(
    session_id: String,
    request_id: RequestId,
    kind: InspectionKind,
    events: mpsc::Sender<BackendEvent>,
    request_cancel: CancellationToken,
    session_cancel: CancellationToken,
    cleaning: Arc<AtomicBool>,
    system: LocalSystemState,
) {
    let operation = local_inspection(kind, &request_cancel, system);
    tokio::pin!(operation);
    let timeout = tokio::time::sleep(deadline(kind));
    tokio::pin!(timeout);
    let (result, finished) = tokio::select! {
        biased;
        _ = session_cancel.cancelled() => { request_cancel.cancel(); let _ = operation.await; return; }
        _ = request_cancel.cancelled() => (Err(InspectionError::Cancelled), false),
        _ = &mut timeout => { request_cancel.cancel(); (Err(InspectionError::Timeout), false) }
        result = &mut operation => (result, true),
    };
    cleaning.store(true, Ordering::Release);
    let delivery = deliver(&events, &session_id, request_id, result, &session_cancel);
    if !finished {
        let _ = operation.await;
    }
    let _ = delivery.await;
}

async fn local_inspection(
    kind: InspectionKind,
    cancel: &CancellationToken,
    system: LocalSystemState,
) -> Result<InspectionData, InspectionError> {
    if cancel.is_cancelled() {
        return Err(InspectionError::Cancelled);
    }
    match kind {
        InspectionKind::History(source) => {
            let home = dirs::home_dir().ok_or(InspectionError::Unavailable)?;
            let cancel = cancel.clone();
            tokio::task::spawn_blocking(move || {
                read_history_file(&home.join(source.filename()), source, &cancel)
            })
            .await
            .map_err(|_| InspectionError::Failed)?
            .map(InspectionData::History)
        }
        InspectionKind::Network => {
            #[cfg(target_os = "macos")]
            {
                let (bytes, status) = read_process(
                    "/usr/sbin/lsof",
                    &["-nP", "-iTCP", "-iUDP", "-FpcfPnT"],
                    stdout_limit(kind),
                    cancel,
                )
                .await?;
                if !status.success() && status.code() != Some(1) {
                    return Err(InspectionError::PermissionDenied);
                }
                parse_lsof(&bytes).map(InspectionData::Network)
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(InspectionError::Unsupported)
            }
        }
        InspectionKind::System => local_system(cancel, system)
            .await
            .map(InspectionData::System),
    }
}

#[cfg(unix)]
fn read_history_file(
    path: &Path,
    source: HistorySource,
    cancel: &CancellationToken,
) -> Result<crate::inspection::HistorySnapshot, InspectionError> {
    use std::io::{Read, Seek, SeekFrom};
    let file = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map(std::fs::File::from)
    .map_err(|error| match error {
        rustix::io::Errno::NOENT => InspectionError::NotFound,
        rustix::io::Errno::ACCESS | rustix::io::Errno::PERM => InspectionError::PermissionDenied,
        rustix::io::Errno::LOOP => InspectionError::UnsafeFile,
        _ => InspectionError::Failed,
    })?;
    let mut file: std::fs::File = file;
    if cancel.is_cancelled() {
        return Err(InspectionError::Cancelled);
    }
    let metadata = file.metadata().map_err(|_| InspectionError::Failed)?;
    if !metadata.is_file() {
        return Err(InspectionError::UnsafeFile);
    }
    let start = metadata.len().saturating_sub(MAX_HISTORY_BYTES as u64 + 1);
    file.seek(SeekFrom::Start(start))
        .map_err(|_| InspectionError::Failed)?;
    let expected = (metadata.len() - start) as usize;
    let mut bytes = Vec::with_capacity(expected);
    let mut buffer = [0_u8; 8192];
    while bytes.len() < expected {
        if cancel.is_cancelled() {
            return Err(InspectionError::Cancelled);
        }
        let limit = buffer.len().min(expected - bytes.len());
        if limit == 0 {
            break;
        }
        let count = file
            .read(&mut buffer[..limit])
            .map_err(|_| InspectionError::Failed)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    if bytes.len() != expected {
        return Err(InspectionError::InvalidData);
    }
    Ok(parse_history(
        source,
        &bytes,
        start > 0 || bytes.len() > MAX_HISTORY_BYTES,
    ))
}

#[cfg(not(unix))]
fn read_history_file(
    _: &Path,
    _: HistorySource,
    _: &CancellationToken,
) -> Result<crate::inspection::HistorySnapshot, InspectionError> {
    Err(InspectionError::Unsupported)
}

#[cfg(target_os = "macos")]
async fn local_system(
    cancel: &CancellationToken,
    state: LocalSystemState,
) -> Result<crate::inspection::SystemSnapshot, InspectionError> {
    let cancel = cancel.clone();
    tokio::task::spawn_blocking(move || {
        use sysinfo::{CpuRefreshKind, MemoryRefreshKind, Networks, RefreshKind, System};
        if cancel.is_cancelled() {
            return Err(InspectionError::Cancelled);
        }
        let now = std::time::Instant::now();
        let mut state = state.lock();
        let initialized = state.is_some();
        let (system, previous) = state.get_or_insert_with(|| {
            (
                System::new_with_specifics(
                    RefreshKind::new()
                        .with_cpu(CpuRefreshKind::new().with_cpu_usage())
                        .with_memory(MemoryRefreshKind::new().with_ram()),
                ),
                now,
            )
        });
        let valid_cpu_delta =
            initialized && now.duration_since(*previous) >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL;
        if initialized {
            system.refresh_cpu_usage();
            system.refresh_memory_specifics(MemoryRefreshKind::new().with_ram());
        }
        *previous = now;
        let networks = Networks::new_with_refreshed_list();
        let mut interfaces: Vec<_> = networks
            .iter()
            .map(|(name, data)| crate::inspection::InterfaceSample {
                name: name.clone(),
                received_bytes: data.total_received(),
                transmitted_bytes: data.total_transmitted(),
                ..Default::default()
            })
            .collect();
        interfaces.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(crate::inspection::SystemSnapshot {
            os: System::long_os_version().or_else(System::name),
            architecture: System::cpu_arch(),
            hostname: System::host_name(),
            cpu_model: system.cpus().first().map(|cpu| cpu.brand().to_owned()),
            logical_cores: Some(system.cpus().len()).filter(|count| *count > 0),
            uptime_seconds: Some(System::uptime() as f64),
            sample_seconds: Some({
                static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
                ORIGIN
                    .get_or_init(std::time::Instant::now)
                    .elapsed()
                    .as_secs_f64()
            }),
            cpu_usage_percent: valid_cpu_delta.then(|| system.global_cpu_usage()),
            memory_total_bytes: Some(system.total_memory()).filter(|total| *total > 0),
            memory_used_bytes: Some(system.used_memory()),
            interfaces,
            ..Default::default()
        })
    })
    .await
    .map_err(|_| InspectionError::Failed)?
}

#[cfg(not(target_os = "macos"))]
async fn local_system(
    _: &CancellationToken,
    _: LocalSystemState,
) -> Result<crate::inspection::SystemSnapshot, InspectionError> {
    Err(InspectionError::Unsupported)
}

#[cfg(target_os = "macos")]
async fn read_process(
    program: &str,
    args: &[&str],
    limit: usize,
    cancel: &CancellationToken,
) -> Result<(Vec<u8>, std::process::ExitStatus), InspectionError> {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| InspectionError::ToolUnavailable)?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let read = async {
        let mut output = Vec::new();
        let mut stderr_count = 0_usize;
        let mut out_done = false;
        let mut err_done = false;
        let mut out = [0_u8; 8192];
        let mut err = [0_u8; 4096];
        while !out_done || !err_done {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(InspectionError::Cancelled),
                count = stdout.read(&mut out), if !out_done => {
                    let count = count.map_err(|_| InspectionError::Failed)?;
                    out_done = count == 0;
                    if output.len().saturating_add(count) > limit { return Err(InspectionError::OutputLimit); }
                    output.extend_from_slice(&out[..count]);
                }
                count = stderr.read(&mut err), if !err_done => {
                    let count = count.map_err(|_| InspectionError::Failed)?;
                    err_done = count == 0;
                    stderr_count = stderr_count.saturating_add(count);
                    if stderr_count > STDERR_LIMIT { return Err(InspectionError::OutputLimit); }
                }
            }
        }
        Ok(output)
    }.await;
    match read {
        Ok(bytes) => {
            let status = tokio::select! {
                biased;
                _ = cancel.cancelled() => { let _ = child.start_kill(); let _ = child.wait().await; return Err(InspectionError::Cancelled); }
                result = child.wait() => result.map_err(|_| InspectionError::Failed)?,
            };
            Ok((bytes, status))
        }
        Err(error) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(unix))]
    #[test]
    fn local_history_is_explicitly_unsupported() {
        assert_eq!(
            read_history_file(
                Path::new("history-fixture"),
                HistorySource::Zsh,
                &CancellationToken::new()
            )
            .unwrap_err(),
            InspectionError::Unsupported
        );
    }

    #[cfg(unix)]
    #[test]
    fn history_file_reader_uses_only_fixture_and_rejects_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("history");
        std::fs::write(&path, b": 1:0;echo fixture\n").unwrap();
        let snapshot =
            read_history_file(&path, HistorySource::Zsh, &CancellationToken::new()).unwrap();
        assert_eq!(snapshot.entries[0].insertion_text(), Some("echo fixture"));
        std::fs::write(&path, b"").unwrap();
        assert!(
            read_history_file(&path, HistorySource::Zsh, &CancellationToken::new())
                .unwrap()
                .entries
                .is_empty()
        );
        #[cfg(unix)]
        {
            let link = root.path().join("link");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert_eq!(
                read_history_file(&link, HistorySource::Zsh, &CancellationToken::new())
                    .unwrap_err(),
                InspectionError::UnsafeFile
            );
        }
        assert_eq!(
            read_history_file(root.path(), HistorySource::Bash, &CancellationToken::new())
                .unwrap_err(),
            InspectionError::UnsafeFile
        );
    }

    #[cfg(unix)]
    #[test]
    fn history_fifo_and_large_fixture_are_bounded() {
        let root = tempfile::tempdir().unwrap();
        let fifo = root.path().join("fifo");
        assert!(std::process::Command::new("/usr/bin/mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        assert_eq!(
            read_history_file(&fifo, HistorySource::Zsh, &CancellationToken::new()).unwrap_err(),
            InspectionError::UnsafeFile
        );
        let path = root.path().join("large");
        let body = b": 123:0;echo fixture\n".repeat(MAX_HISTORY_BYTES / 10);
        std::fs::write(&path, body).unwrap();
        let snapshot =
            read_history_file(&path, HistorySource::Zsh, &CancellationToken::new()).unwrap();
        assert!(snapshot.truncated);
        assert_eq!(snapshot.entries.len(), 1000);
        assert!(snapshot
            .entries
            .iter()
            .all(|entry| entry.insertion_text() == Some("echo fixture")));
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            read_history_file(&path, HistorySource::Zsh, &cancel).unwrap_err(),
            InspectionError::Cancelled
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn local_process_cancel_and_output_limits_are_reaped() {
        let cancel = CancellationToken::new();
        let cancelled = read_process("/bin/sleep", &["30"], 1024, &cancel);
        let trigger = async {
            tokio::task::yield_now().await;
            cancel.cancel();
        };
        let (result, ()) = tokio::join!(cancelled, trigger);
        assert_eq!(result.unwrap_err(), InspectionError::Cancelled);
        let result = read_process("/usr/bin/yes", &[], 1024, &CancellationToken::new()).await;
        assert_eq!(result.unwrap_err(), InspectionError::OutputLimit);
    }

    #[test]
    fn fixed_commands_have_no_user_text_and_history_uses_opaque_eof_body() {
        for kind in [
            InspectionKind::System,
            InspectionKind::Network,
            InspectionKind::History(HistorySource::Bash),
            InspectionKind::History(HistorySource::Zsh),
        ] {
            let command = remote_command(kind);
            assert!(command.starts_with("/bin/sh -c 'PATH="));
            assert!(!command.contains("sudo"));
            assert_eq!(command.matches('\'').count(), 2);
        }
        let parsed = decode_remote(
            InspectionKind::History(HistorySource::Zsh),
            b"RSTERM_HISTORY_1\nRSTERM_HISTORY_1\necho fixture\n",
            Some(0),
        )
        .unwrap();
        let InspectionData::History(history) = parsed else {
            panic!()
        };
        assert_eq!(history.entries.len(), 2);
        assert_eq!(
            decode_remote(InspectionKind::Network, b"", Some(69)).unwrap_err(),
            InspectionError::ToolUnavailable
        );
    }
}
