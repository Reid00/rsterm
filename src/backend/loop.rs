use std::collections::HashMap;
use std::sync::Arc;

use russh::{Channel, ChannelMsg};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::inspection::InspectionSlot;
use crate::backend::{
    AppCommand, BackendEvent, FileEntryInfo, LocalPty, PtyBackend, SshConnectionConfig,
    SshTransport, TransferId, TransferOutcome,
};
use crate::config::SessionConfig;
use crate::error::{SftpError, SshError};
use crate::inspection::InspectionError;
use crate::sftp::{remote_parent, SftpClient};

struct SessionTask {
    commands: mpsc::UnboundedSender<AppCommand>,
    cancel: CancellationToken,
}

pub async fn backend_loop(
    mut commands: mpsc::Receiver<AppCommand>,
    events: mpsc::Sender<BackendEvent>,
) {
    let mut sessions: HashMap<String, SessionTask> = HashMap::new();
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break; };
                match command {
                    AppCommand::Shutdown => break,
                    AppCommand::CreateSession { session_id, config } => {
                        if sessions.contains_key(&session_id) {
                            send_detached(&events, BackendEvent::SessionFailed { session_id, error: "Session ID already exists".into(), retryable: false });
                            continue;
                        }
                        let (tx, rx) = mpsc::unbounded_channel();
                        let cancel = CancellationToken::new();
                        sessions.insert(session_id.clone(), SessionTask { commands: tx, cancel: cancel.clone() });
                        let events = events.clone();
                        tasks.spawn(async move {
                            run_local_session(session_id.clone(), config, rx, events, cancel).await;
                            session_id
                        });
                    }
                    AppCommand::CreateSshSession { session_id, ssh_config } => {
                        if sessions.contains_key(&session_id) {
                            send_detached(&events, BackendEvent::SessionFailed { session_id, error: "Session ID already exists".into(), retryable: false });
                            continue;
                        }
                        let (tx, rx) = mpsc::unbounded_channel();
                        let cancel = CancellationToken::new();
                        sessions.insert(session_id.clone(), SessionTask { commands: tx, cancel: cancel.clone() });
                        let events = events.clone();
                        tasks.spawn(async move {
                            run_ssh_session(session_id.clone(), ssh_config, rx, events, cancel).await;
                            session_id
                        });
                    }
                    AppCommand::CloseSession { session_id } => {
                        if let Some(session) = sessions.get(&session_id) { session.cancel.cancel(); }
                    }
                    command => {
                        let session_id = command_session(&command).to_string();
                        match sessions.get(&session_id) {
                            Some(session) => {
                                if let Err(error) = session.commands.send(command) {
                                    reject_command(error.0, &events, "Session is closed");
                                }
                            }
                            None => reject_command(command, &events, "Session not found"),
                        }
                    }
                }
            }
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                match result {
                    Ok(session_id) => { sessions.remove(&session_id); }
                    Err(error) => tracing::error!("Session supervisor failed: {error}"),
                }
            }
        }
    }
    for session in sessions.values() {
        session.cancel.cancel();
    }
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            tracing::error!("Session shutdown failed: {error}");
        }
    }
}

fn command_session(command: &AppCommand) -> &str {
    match command {
        AppCommand::CreateSession { session_id, .. }
        | AppCommand::CreateSshSession { session_id, .. }
        | AppCommand::WriteInput { session_id, .. }
        | AppCommand::ResizeSession { session_id, .. }
        | AppCommand::CloseSession { session_id }
        | AppCommand::SftpInit { session_id }
        | AppCommand::SftpListDir { session_id, .. }
        | AppCommand::SftpMkdir { session_id, .. }
        | AppCommand::SftpDelete { session_id, .. }
        | AppCommand::SftpRename { session_id, .. }
        | AppCommand::SftpChmod { session_id, .. }
        | AppCommand::SftpUpload { session_id, .. }
        | AppCommand::SftpDownload { session_id, .. }
        | AppCommand::CancelTransfer { session_id, .. }
        | AppCommand::Inspect { session_id, .. }
        | AppCommand::CancelInspection { session_id, .. } => session_id,
        AppCommand::Shutdown => "",
    }
}

fn send_detached(events: &mpsc::Sender<BackendEvent>, event: BackendEvent) {
    let events = events.clone();
    tokio::spawn(async move {
        let _ = events.send(event).await;
    });
}

fn reject_command(command: AppCommand, events: &mpsc::Sender<BackendEvent>, error: &str) {
    let error = error.to_string();
    let event = match command {
        AppCommand::Inspect {
            session_id,
            request_id,
            ..
        } => {
            let events = events.clone();
            tokio::spawn(async move {
                if events
                    .send(BackendEvent::InspectionResult {
                        session_id: session_id.clone(),
                        request_id,
                        result: Err(InspectionError::Unavailable),
                    })
                    .await
                    .is_ok()
                {
                    let _ = events
                        .send(BackendEvent::InspectionReady { session_id })
                        .await;
                }
            });
            return;
        }
        AppCommand::CancelInspection { .. } => return,
        AppCommand::SftpListDir {
            session_id,
            request_id,
            path,
        } => BackendEvent::SftpListResult {
            session_id,
            request_id,
            path,
            result: Err(error),
        },
        AppCommand::SftpMkdir {
            session_id,
            request_id,
            path,
        }
        | AppCommand::SftpDelete {
            session_id,
            request_id,
            path,
        }
        | AppCommand::SftpChmod {
            session_id,
            request_id,
            path,
            ..
        } => BackendEvent::SftpOperationResult {
            session_id,
            request_id,
            path: remote_parent(&path),
            result: Err(error),
        },
        AppCommand::SftpRename {
            session_id,
            request_id,
            to,
            ..
        } => BackendEvent::SftpOperationResult {
            session_id,
            request_id,
            path: remote_parent(&to),
            result: Err(error),
        },
        AppCommand::SftpUpload {
            session_id,
            transfer_id,
            ..
        }
        | AppCommand::SftpDownload {
            session_id,
            transfer_id,
            ..
        } => BackendEvent::SftpTransferComplete {
            session_id,
            transfer_id,
            outcome: TransferOutcome::Failed(error),
        },
        AppCommand::CancelTransfer { session_id, .. } | AppCommand::SftpInit { session_id } => {
            BackendEvent::SftpError { session_id, error }
        }
        AppCommand::WriteInput { session_id, .. }
        | AppCommand::ResizeSession { session_id, .. } => {
            BackendEvent::ShellUnavailable { session_id, error }
        }
        _ => return,
    };
    send_detached(events, event);
}

async fn run_local_session(
    session_id: String,
    config: SessionConfig,
    mut commands: mpsc::UnboundedReceiver<AppCommand>,
    events: mpsc::Sender<BackendEvent>,
    cancel: CancellationToken,
) {
    let (output_tx, mut output) = mpsc::channel(256);
    let shell = config.shell.unwrap_or_else(crate::platform::default_shell);
    let creation = tokio::task::spawn_blocking(move || LocalPty::new(&shell, 80, 24, output_tx));
    let backend = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            commands.close();
            while let Ok(command) = commands.try_recv() { reject_command(command, &events, "Session is closed"); }
            let _ = events.send(BackendEvent::SessionClosed { session_id, reason: None, retryable: false }).await;
            return;
        }
        result = creation => match result {
            Ok(Ok(backend)) => Arc::new(backend),
            result => {
                let error = match result { Ok(Err(error)) => error.to_string(), Err(error) => error.to_string(), _ => unreachable!() };
                commands.close();
                while let Ok(command) = commands.try_recv() { reject_command(command, &events, &error); }
                let _ = events.send(BackendEvent::SessionFailed { session_id, error, retryable: false }).await;
                return;
            }
        }
    };
    if events
        .send(BackendEvent::SessionCreated {
            session_id: session_id.clone(),
        })
        .await
        .is_err()
    {
        backend.kill();
        return;
    }
    let _ = events
        .send(BackendEvent::ShellReady {
            session_id: session_id.clone(),
        })
        .await;
    let mut inspection = InspectionSlot::default();
    let mut reason = None;
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = inspection.completed() => {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => break,
                    _ = events.send(BackendEvent::InspectionReady { session_id: session_id.clone() }) => {},
                }
            }
            data = output.recv() => {
                match data {
                    Some(data) => {
                        let result = tokio::select! {
                            _ = cancel.cancelled() => break,
                            result = events.send(BackendEvent::SessionOutput { session_id: session_id.clone(), data }) => result,
                        };
                        if result.is_err() { break; }
                    }
                    None => break,
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { break; };
                let result = match command {
                    AppCommand::WriteInput { data, .. } => {
                        tokio::select! { _ = cancel.cancelled() => break, result = backend.write(&data) => result }
                    }
                    AppCommand::ResizeSession { cols, rows, .. } => {
                        tokio::select! { _ = cancel.cancelled() => break, result = backend.resize(cols, rows) => result }
                    }
                    AppCommand::Inspect { request_id, kind, .. } => {
                        inspection.start(session_id.clone(), request_id, kind, None, events.clone(), cancel.clone()).await;
                        continue;
                    }
                    AppCommand::CancelInspection { request_id, .. } => {
                        inspection.cancel(request_id);
                        continue;
                    }
                    command => { reject_command(command, &events, "SFTP is unavailable for local sessions"); continue; }
                };
                if let Err(error) = result { reason = Some(error.to_string()); break; }
            }
        }
    }
    cancel.cancel();
    backend.kill();
    inspection.shutdown().await;
    commands.close();
    while let Ok(command) = commands.try_recv() {
        reject_command(command, &events, "Session is closed");
    }
    let _ = events
        .send(BackendEvent::SessionClosed {
            session_id,
            reason,
            retryable: false,
        })
        .await;
}

enum SessionResult {
    SftpInitialized(Result<SftpClient, String>),
    TransferFinished(TransferId),
}

enum ShellCommand {
    Write(Vec<u8>),
    Resize(u16, u16),
}

async fn run_ssh_session(
    session_id: String,
    config: SshConnectionConfig,
    mut commands: mpsc::UnboundedReceiver<AppCommand>,
    events: mpsc::Sender<BackendEvent>,
    cancel: CancellationToken,
) {
    let transport =
        match SshTransport::connect(&config, session_id.clone(), events.clone(), cancel.clone())
            .await
        {
            Ok(transport) => Arc::new(transport),
            Err(error) => {
                cancel.cancel();
                commands.close();
                while let Ok(command) = commands.try_recv() {
                    reject_command(command, &events, &error.to_string());
                }
                let event = if matches!(error, SshError::Cancelled) {
                    BackendEvent::SessionClosed {
                        session_id,
                        reason: None,
                        retryable: false,
                    }
                } else {
                    let retryable = matches!(
                        error,
                        SshError::Io(_)
                            | SshError::ConnectionTimeout { .. }
                            | SshError::ConnectionRefused { .. }
                            | SshError::NetworkLost
                            | SshError::Russh(russh::Error::IO(_) | russh::Error::Disconnect)
                    );
                    BackendEvent::SessionFailed {
                        session_id,
                        error: error.to_string(),
                        retryable,
                    }
                };
                let _ = events.send(event).await;
                return;
            }
        };
    if events
        .send(BackendEvent::SessionCreated {
            session_id: session_id.clone(),
        })
        .await
        .is_err()
    {
        cancel.cancel();
        transport.disconnect().await;
        return;
    }
    let mut disconnected = transport.disconnection();
    let (shell_tx, shell_rx) = mpsc::unbounded_channel();
    let mut tasks = JoinSet::new();
    {
        let transport = transport.clone();
        let session_id = session_id.clone();
        let events = events.clone();
        let cancel = cancel.clone();
        tasks.spawn(async move {
            run_shell(transport, session_id, shell_rx, events, cancel).await;
        });
    }
    let (result_tx, mut results) = mpsc::unbounded_channel();
    let mut inspection = InspectionSlot::default();
    let mut sftp: Option<SftpClient> = None;
    let mut initializing = false;
    let mut transfers: HashMap<TransferId, CancellationToken> = HashMap::new();
    let mut reason = None;
    let mut retryable = false;
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = inspection.completed() => {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => break,
                    _ = events.send(BackendEvent::InspectionReady { session_id: session_id.clone() }) => {},
                }
            }
            changed = disconnected.changed() => {
                reason = disconnected.borrow().clone().or_else(|| Some("SSH transport closed".into()));
                retryable = changed.is_ok();
                break;
            }
            Some(result) = results.recv() => match result {
                SessionResult::SftpInitialized(result) => {
                    initializing = false;
                    match result {
                        Ok(client) => {
                            let _ = events.send(BackendEvent::SftpReady { session_id: session_id.clone(), cwd: client.pwd().to_string() }).await;
                            sftp = Some(client);
                        }
                        Err(error) => { let _ = events.send(BackendEvent::SftpError { session_id: session_id.clone(), error }).await; }
                    }
                }
                SessionResult::TransferFinished(id) => { transfers.remove(&id); }
            },
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                if let Err(error) = result { tracing::error!("Session task failed: {error}"); }
            }
            command = commands.recv() => {
                let Some(command) = command else { break; };
                match command {
                    AppCommand::WriteInput { data, .. } => {
                        if shell_tx.send(ShellCommand::Write(data)).is_err() {
                            let _ = events.send(BackendEvent::ShellUnavailable { session_id: session_id.clone(), error: "No interactive shell is available; SFTP can still be used".into() }).await;
                        }
                    }
                    AppCommand::ResizeSession { cols, rows, .. } => {
                        if shell_tx.send(ShellCommand::Resize(cols, rows)).is_err() {
                            let _ = events.send(BackendEvent::ShellUnavailable { session_id: session_id.clone(), error: "Cannot resize an unavailable interactive shell".into() }).await;
                        }
                    }
                    AppCommand::Inspect { request_id, kind, .. } => {
                        inspection.start(session_id.clone(), request_id, kind, Some(transport.clone()), events.clone(), cancel.clone()).await;
                    }
                    AppCommand::CancelInspection { request_id, .. } => {
                        inspection.cancel(request_id);
                    }
                    AppCommand::SftpInit { .. } => {
                        if let Some(client) = &sftp {
                            let _ = events.send(BackendEvent::SftpReady { session_id: session_id.clone(), cwd: client.pwd().to_string() }).await;
                        } else if !initializing {
                            initializing = true;
                            let transport = transport.clone();
                            let results = result_tx.clone();
                            let cancel = cancel.clone();
                            tasks.spawn(async move {
                                let result = tokio::select! {
                                    _ = cancel.cancelled() => Err("Session is closed".into()),
                                    result = transport.sftp() => result.map_err(|error| error.to_string()),
                                };
                                let _ = results.send(SessionResult::SftpInitialized(result));
                            });
                        }
                    }
                    AppCommand::CancelTransfer { transfer_id, .. } => {
                        if let Some(transfer) = transfers.get(&transfer_id) { transfer.cancel(); }
                        else { let _ = events.send(BackendEvent::SftpError { session_id: session_id.clone(), error: format!("Transfer {transfer_id} is not active") }).await; }
                    }
                    command @ (AppCommand::SftpUpload { .. } | AppCommand::SftpDownload { .. }) => {
                        let id = match &command { AppCommand::SftpUpload { transfer_id, .. } | AppCommand::SftpDownload { transfer_id, .. } => *transfer_id, _ => unreachable!() };
                        if transfers.contains_key(&id) {
                            let _ = events.send(BackendEvent::SftpError { session_id: session_id.clone(), error: format!("Transfer ID {id} is already active") }).await;
                            continue;
                        }
                        if let Some(client) = &sftp {
                            let transfer_cancel = cancel.child_token();
                            transfers.insert(id, transfer_cancel.clone());
                            let client = client.clone();
                            let events = events.clone();
                            let results = result_tx.clone();
                            tasks.spawn(async move { run_transfer(client, command, events, transfer_cancel).await; let _ = results.send(SessionResult::TransferFinished(id)); });
                        } else { reject_command(command, &events, "SFTP is not initialized"); }
                    }
                    command => {
                        if let Some(client) = &sftp {
                            let client = client.clone();
                            let events = events.clone();
                            let cancel = cancel.clone();
                            tasks.spawn(async move { run_operation(client, command, events, cancel).await; });
                        } else { reject_command(command, &events, "SFTP is not initialized"); }
                    }
                }
            }
        }
    }
    cancel.cancel();
    transport.stop_stream();
    inspection.shutdown().await;
    commands.close();
    while let Ok(command) = commands.try_recv() {
        reject_command(command, &events, "Session is closed");
    }
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            tracing::error!("Session task shutdown failed: {error}");
        }
    }
    drop(sftp);
    transport.disconnect().await;
    let _ = events
        .send(BackendEvent::SessionClosed {
            session_id,
            reason,
            retryable,
        })
        .await;
}

async fn run_shell(
    transport: Arc<SshTransport>,
    session_id: String,
    mut commands: mpsc::UnboundedReceiver<ShellCommand>,
    events: mpsc::Sender<BackendEvent>,
    cancel: CancellationToken,
) {
    let shell = tokio::select! {
        _ = cancel.cancelled() => return,
        result = transport.open_shell(80, 24) => result,
    };
    let result = match shell {
        Ok(channel) => {
            let sent = tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                result = events.send(BackendEvent::ShellReady { session_id: session_id.clone() }) => result,
            };
            if sent.is_err() {
                return;
            }
            shell_io(channel, &session_id, &mut commands, &events, &cancel).await
        }
        Err(error) => Err(error),
    };
    if !cancel.is_cancelled() {
        let error = match result {
            Ok(()) => "Interactive shell closed; SFTP remains available".into(),
            Err(error) => error.to_string(),
        };
        let _ = events
            .send(BackendEvent::ShellUnavailable { session_id, error })
            .await;
    }
}

async fn shell_io(
    mut channel: Channel<russh::client::Msg>,
    session_id: &str,
    commands: &mut mpsc::UnboundedReceiver<ShellCommand>,
    events: &mpsc::Sender<BackendEvent>,
    cancel: &CancellationToken,
) -> Result<(), SshError> {
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => { let _ = channel.close().await; return Ok(()); }
            command = commands.recv() => {
                let result = match command {
                    Some(ShellCommand::Write(data)) => tokio::select! { _ = cancel.cancelled() => return Ok(()), result = channel.data(&data[..]) => result },
                    Some(ShellCommand::Resize(cols, rows)) => tokio::select! { _ = cancel.cancelled() => return Ok(()), result = channel.window_change(cols.into(), rows.into(), 0, 0) => result },
                    None => return Ok(()),
                };
                result?;
            }
            message = channel.wait() => {
                match message {
                    Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                        tokio::select! {
                            _ = cancel.cancelled() => return Ok(()),
                            result = events.send(BackendEvent::SessionOutput { session_id: session_id.to_string(), data: data.to_vec() }) => { if result.is_err() { return Ok(()); } },
                        }
                    }
                    Some(ChannelMsg::Close | ChannelMsg::Eof) | None => return Ok(()),
                    _ => {}
                }
            }
        }
    }
}

async fn run_operation(
    client: SftpClient,
    command: AppCommand,
    events: mpsc::Sender<BackendEvent>,
    cancel: CancellationToken,
) {
    match command {
        AppCommand::SftpListDir {
            session_id,
            request_id,
            path,
        } => {
            let resolved = client.resolve_path(&path);
            let event_path = resolved.as_ref().unwrap_or(&path).clone();
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(SftpError::Cancelled),
                result = async { match resolved { Ok(path) => client.list_dir(&path).await, Err(error) => Err(error) } } => result,
            };
            let result = result
                .map(|entries| {
                    entries
                        .into_iter()
                        .map(|entry| FileEntryInfo {
                            name: entry.name,
                            path: entry.path,
                            is_dir: entry.is_dir,
                            is_symlink: entry.is_symlink,
                            size: entry.size,
                            modified: entry.modified,
                            permissions: entry.permissions,
                        })
                        .collect()
                })
                .map_err(|error| error.to_string());
            let _ = events
                .send(BackendEvent::SftpListResult {
                    session_id,
                    request_id,
                    path: event_path,
                    result,
                })
                .await;
        }
        command => {
            let (session_id, request_id, path) = match &command {
                AppCommand::SftpMkdir {
                    session_id,
                    request_id,
                    path,
                }
                | AppCommand::SftpDelete {
                    session_id,
                    request_id,
                    path,
                }
                | AppCommand::SftpChmod {
                    session_id,
                    request_id,
                    path,
                    ..
                } => (session_id.clone(), *request_id, path.clone()),
                AppCommand::SftpRename {
                    session_id,
                    request_id,
                    to,
                    ..
                } => (session_id.clone(), *request_id, to.clone()),
                _ => {
                    reject_command(command, &events, "Unsupported session command");
                    return;
                }
            };
            let parent = remote_parent(&client.resolve_path(&path).unwrap_or(path));
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(SftpError::Cancelled),
                result = async { match command {
                    AppCommand::SftpMkdir { path, .. } => client.mkdir(&path).await,
                    AppCommand::SftpDelete { path, .. } => client.delete(&path).await,
                    AppCommand::SftpRename { from, to, .. } => client.rename(&from, &to).await,
                    AppCommand::SftpChmod { path, mode, .. } => client.chmod(&path, mode).await,
                    _ => unreachable!(),
                }} => result,
            }
            .map_err(|error| error.to_string());
            let _ = events
                .send(BackendEvent::SftpOperationResult {
                    session_id,
                    request_id,
                    path: parent,
                    result,
                })
                .await;
        }
    }
}

async fn run_transfer(
    client: SftpClient,
    command: AppCommand,
    events: mpsc::Sender<BackendEvent>,
    cancel: CancellationToken,
) {
    let (session_id, transfer_id) = match &command {
        AppCommand::SftpUpload {
            session_id,
            transfer_id,
            ..
        }
        | AppCommand::SftpDownload {
            session_id,
            transfer_id,
            ..
        } => (session_id.clone(), *transfer_id),
        _ => return,
    };
    let progress = |bytes_transferred, total_bytes| {
        let _ = events.try_send(BackendEvent::SftpTransferProgress {
            session_id: session_id.clone(),
            transfer_id,
            bytes_transferred,
            total_bytes,
        });
    };
    let result = match command {
        AppCommand::SftpUpload {
            local_path,
            remote_path,
            overwrite,
            ..
        } => {
            client
                .upload(&local_path, &remote_path, overwrite, &cancel, &progress)
                .await
        }
        AppCommand::SftpDownload {
            remote_path,
            local_path,
            overwrite,
            ..
        } => {
            client
                .download(&remote_path, &local_path, overwrite, &cancel, &progress)
                .await
        }
        _ => unreachable!(),
    };
    let outcome = match result {
        Ok(()) => TransferOutcome::Completed,
        Err(SftpError::Cancelled) => TransferOutcome::Cancelled,
        Err(error) => TransferOutcome::Failed(error.to_string()),
    };
    let _ = events
        .send(BackendEvent::SftpTransferComplete {
            session_id,
            transfer_id,
            outcome,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::SshAuthMethod;
    use crate::ssh::KnownHosts;
    use std::time::Duration;

    #[derive(Default)]
    struct SftpOnlyServer {
        channels: HashMap<russh::ChannelId, Channel<russh::server::Msg>>,
    }

    #[async_trait::async_trait]
    impl russh::server::Handler for SftpOnlyServer {
        type Error = russh::Error;
        async fn auth_password(
            &mut self,
            _: &str,
            _: &str,
        ) -> Result<russh::server::Auth, Self::Error> {
            Ok(russh::server::Auth::Accept)
        }
        async fn channel_open_session(
            &mut self,
            channel: Channel<russh::server::Msg>,
            _: &mut russh::server::Session,
        ) -> Result<bool, Self::Error> {
            self.channels.insert(channel.id(), channel);
            Ok(true)
        }
        async fn pty_request(
            &mut self,
            channel: russh::ChannelId,
            _: &str,
            _: u32,
            _: u32,
            _: u32,
            _: u32,
            _: &[(russh::Pty, u32)],
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            session.channel_failure(channel);
            Ok(())
        }
        async fn subsystem_request(
            &mut self,
            channel: russh::ChannelId,
            name: &str,
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            if name == "sftp" {
                session.channel_success(channel);
                russh_sftp::server::run(
                    self.channels.remove(&channel).unwrap().into_stream(),
                    MinimalSftp,
                )
                .await;
            } else {
                session.channel_failure(channel);
            }
            Ok(())
        }
    }

    #[derive(Clone)]
    enum InspectionBehavior {
        Normal,
        StatusFirst,
        Denied,
        Oversized,
        StderrOversized,
        Hanging,
        LateOpen(Arc<tokio::sync::Notify>),
        NeverOpen,
        IgnoredClose,
    }

    struct InspectionServer {
        channels: HashMap<russh::ChannelId, Channel<russh::server::Msg>>,
        behavior: InspectionBehavior,
        opened: mpsc::Sender<usize>,
        executed: Arc<std::sync::atomic::AtomicUsize>,
        channel_count: usize,
    }

    #[async_trait::async_trait]
    impl russh::server::Handler for InspectionServer {
        type Error = russh::Error;

        async fn auth_password(
            &mut self,
            _: &str,
            _: &str,
        ) -> Result<russh::server::Auth, Self::Error> {
            Ok(russh::server::Auth::Accept)
        }

        async fn channel_open_session(
            &mut self,
            channel: Channel<russh::server::Msg>,
            _: &mut russh::server::Session,
        ) -> Result<bool, Self::Error> {
            self.channel_count += 1;
            let _ = self.opened.send(self.channel_count).await;
            if self.channel_count == 2 {
                match &self.behavior {
                    InspectionBehavior::LateOpen(notify) => notify.notified().await,
                    InspectionBehavior::NeverOpen => std::future::pending().await,
                    _ => {}
                }
            }
            self.channels.insert(channel.id(), channel);
            Ok(true)
        }

        async fn pty_request(
            &mut self,
            channel: russh::ChannelId,
            _: &str,
            _: u32,
            _: u32,
            _: u32,
            _: u32,
            _: &[(russh::Pty, u32)],
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            session.channel_success(channel);
            Ok(())
        }

        async fn shell_request(
            &mut self,
            channel: russh::ChannelId,
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            session.channel_success(channel);
            session.data(channel, russh::CryptoVec::from_slice(b"synthetic-shell\n"));
            Ok(())
        }

        async fn data(
            &mut self,
            channel: russh::ChannelId,
            bytes: &[u8],
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            session.data(channel, russh::CryptoVec::from_slice(bytes));
            Ok(())
        }

        async fn subsystem_request(
            &mut self,
            channel: russh::ChannelId,
            name: &str,
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            if name == "sftp" {
                session.channel_success(channel);
                russh_sftp::server::run(
                    self.channels.remove(&channel).unwrap().into_stream(),
                    MinimalSftp,
                )
                .await;
            } else {
                session.channel_failure(channel);
            }
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: russh::ChannelId,
            data: &[u8],
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            self.executed
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert!(data.starts_with(b"/bin/sh -c "));
            if matches!(self.behavior, InspectionBehavior::Denied) {
                session.channel_failure(channel);
                session.close(channel);
                return Ok(());
            }
            session.channel_success(channel);
            match self.behavior {
                InspectionBehavior::Hanging | InspectionBehavior::IgnoredClose => {}
                InspectionBehavior::Oversized => {
                    let handle = session.handle();
                    tokio::spawn(async move {
                        for _ in 0..12 {
                            if handle
                                .data(channel, russh::CryptoVec::from_slice(&vec![b'x'; 32768]))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        let _ = handle.close(channel).await;
                    });
                }
                InspectionBehavior::StderrOversized => {
                    session.extended_data(
                        channel,
                        1,
                        russh::CryptoVec::from_slice(&vec![b'x'; 17000]),
                    );
                    session.close(channel);
                }
                _ => {
                    if matches!(self.behavior, InspectionBehavior::StatusFirst) {
                        session.exit_status_request(channel, 0);
                    }
                    let body = if data.windows(16).any(|part| part == b"RSTERM_SYSTEM_1\\") {
                        b"RSTERM_SYSTEM_1\nLinux\nx86_64\n@@stat\ncpu 100 0 20 80 0 0 0 0\ncpu0 0\n@@uptime\n10.0 0\n".as_slice()
                    } else {
                        b"RSTERM_NETWORK_1\ntcp LISTEN 0 0 127.0.0.1:22 0.0.0.0:*\n".as_slice()
                    };
                    session.data(channel, russh::CryptoVec::from_slice(body));
                    if !matches!(self.behavior, InspectionBehavior::StatusFirst) {
                        session.exit_status_request(channel, 0);
                    }
                    session.eof(channel);
                    session.close(channel);
                }
            }
            Ok(())
        }
    }

    struct InspectionFixture {
        commands: mpsc::Sender<AppCommand>,
        events: mpsc::Receiver<BackendEvent>,
        backend: tokio::task::JoinHandle<()>,
        server: tokio::task::JoinHandle<()>,
        opened: mpsc::Receiver<usize>,
        executed: Arc<std::sync::atomic::AtomicUsize>,
        _known_hosts: tempfile::NamedTempFile,
        session_id: String,
    }

    impl InspectionFixture {
        async fn new(behavior: InspectionBehavior) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let (opened_tx, opened) = mpsc::channel(16);
            let executed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let count = executed.clone();
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let config = russh::server::Config {
                    keys: vec![russh_keys::key::KeyPair::generate_ed25519().unwrap()],
                    auth_rejection_time: Duration::ZERO,
                    auth_rejection_time_initial: Some(Duration::ZERO),
                    ..Default::default()
                };
                let _ = russh::server::run_stream(
                    Arc::new(config),
                    socket,
                    InspectionServer {
                        channels: HashMap::new(),
                        behavior,
                        opened: opened_tx,
                        executed: count,
                        channel_count: 0,
                    },
                )
                .await
                .unwrap()
                .await;
            });
            let known_hosts = tempfile::NamedTempFile::new().unwrap();
            let (commands, rx) = mpsc::channel(16);
            let (events, event_rx) = mpsc::channel(16);
            let backend = tokio::spawn(backend_loop(rx, events));
            let session_id = format!("inspection-{}", uuid::Uuid::new_v4());
            commands
                .send(AppCommand::CreateSshSession {
                    session_id: session_id.clone(),
                    ssh_config: SshConnectionConfig {
                        host: "127.0.0.1".into(),
                        port,
                        username: "fixture".into(),
                        auth: SshAuthMethod::Password("fixture".into()),
                        known_hosts: KnownHosts::load_from(known_hosts.path().to_path_buf())
                            .unwrap(),
                        timeout: Duration::from_secs(5),
                        keepalive_interval: Duration::from_secs(30),
                        keepalive_max: 3,
                    },
                })
                .await
                .unwrap();
            let mut fixture = Self {
                commands,
                events: event_rx,
                backend,
                server,
                opened,
                executed,
                _known_hosts: known_hosts,
                session_id,
            };
            loop {
                match fixture.next().await {
                    BackendEvent::HostKeyPrompt { respond_to, .. } => {
                        respond_to.send(true).unwrap()
                    }
                    BackendEvent::ShellReady { .. } => break,
                    BackendEvent::SessionCreated { .. } | BackendEvent::SessionOutput { .. } => {}
                    event => panic!("Unexpected fixture startup event: {event:?}"),
                }
            }
            fixture
        }

        async fn next(&mut self) -> BackendEvent {
            tokio::time::timeout(Duration::from_secs(8), self.events.recv())
                .await
                .expect("Inspection event timeout")
                .expect("Inspection event channel closed")
        }

        async fn inspect(&self, request_id: u64, kind: crate::inspection::InspectionKind) {
            self.commands
                .send(AppCommand::Inspect {
                    session_id: self.session_id.clone(),
                    request_id,
                    kind,
                })
                .await
                .unwrap();
        }

        async fn result(
            &mut self,
            request_id: u64,
        ) -> Result<crate::inspection::InspectionData, InspectionError> {
            loop {
                match self.next().await {
                    BackendEvent::InspectionResult {
                        session_id,
                        request_id: id,
                        result,
                    } => {
                        assert_eq!(session_id, self.session_id);
                        assert_eq!(id, request_id);
                        return result;
                    }
                    BackendEvent::SessionOutput { data, .. } => {
                        assert_eq!(data, b"synthetic-shell\n")
                    }
                    event => panic!("Unexpected inspection event: {event:?}"),
                }
            }
        }

        async fn ready(&mut self) {
            loop {
                match self.next().await {
                    BackendEvent::InspectionReady { session_id } => {
                        assert_eq!(session_id, self.session_id);
                        return;
                    }
                    BackendEvent::SessionOutput { .. } => {}
                    event => panic!("Unexpected readiness event: {event:?}"),
                }
            }
        }

        async fn shutdown(self) {
            self.commands.send(AppCommand::Shutdown).await.unwrap();
            tokio::time::timeout(Duration::from_secs(3), self.backend)
                .await
                .unwrap()
                .unwrap();
            self.server.abort();
            let _ = self.server.await;
        }
    }

    #[tokio::test]
    async fn inspection_exec_does_not_mix_shell_or_sftp_and_drains_status_first() {
        use crate::inspection::{InspectionData, InspectionKind};
        let mut fixture = InspectionFixture::new(InspectionBehavior::StatusFirst).await;
        fixture.inspect(1, InspectionKind::Network).await;
        assert!(
            matches!(fixture.result(1).await, Ok(InspectionData::Network(snapshot)) if snapshot.connections.len() == 1)
        );
        fixture.ready().await;
        fixture
            .commands
            .send(AppCommand::SftpInit {
                session_id: fixture.session_id.clone(),
            })
            .await
            .unwrap();
        loop {
            match fixture.next().await {
                BackendEvent::SftpReady { cwd, .. } => {
                    assert_eq!(cwd, "/home/test");
                    break;
                }
                BackendEvent::SessionOutput { .. } => {}
                event => panic!("Unexpected SFTP event: {event:?}"),
            }
        }
        fixture
            .commands
            .send(AppCommand::WriteInput {
                session_id: fixture.session_id.clone(),
                data: b"shell-still-live".to_vec(),
            })
            .await
            .unwrap();
        assert!(
            matches!(fixture.next().await, BackendEvent::SessionOutput { data, .. } if data == b"shell-still-live")
        );
        assert_eq!(
            fixture.executed.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        fixture.shutdown().await;
    }

    #[tokio::test]
    async fn inspection_denial_and_output_caps_are_typed() {
        use crate::inspection::InspectionKind;
        for (behavior, expected) in [
            (InspectionBehavior::Denied, InspectionError::Rejected),
            (InspectionBehavior::Oversized, InspectionError::OutputLimit),
            (
                InspectionBehavior::StderrOversized,
                InspectionError::OutputLimit,
            ),
        ] {
            let mut fixture = InspectionFixture::new(behavior).await;
            fixture.inspect(10, InspectionKind::System).await;
            assert_eq!(fixture.result(10).await.unwrap_err(), expected);
            fixture.ready().await;
            fixture.shutdown().await;
        }
    }

    #[tokio::test]
    async fn inspection_cancelled_late_open_is_closed_without_exec_or_slot_reuse() {
        use crate::inspection::InspectionKind;
        let gate = Arc::new(tokio::sync::Notify::new());
        let mut fixture = InspectionFixture::new(InspectionBehavior::LateOpen(gate.clone())).await;
        assert_eq!(fixture.opened.recv().await, Some(1));
        fixture.inspect(1, InspectionKind::Network).await;
        assert_eq!(fixture.opened.recv().await, Some(2));
        fixture
            .commands
            .send(AppCommand::CancelInspection {
                session_id: fixture.session_id.clone(),
                request_id: 999,
            })
            .await
            .unwrap();
        fixture
            .commands
            .send(AppCommand::CancelInspection {
                session_id: fixture.session_id.clone(),
                request_id: 1,
            })
            .await
            .unwrap();
        assert_eq!(
            fixture.result(1).await.unwrap_err(),
            InspectionError::Cancelled
        );
        fixture.inspect(2, InspectionKind::Network).await;
        assert_eq!(
            fixture.result(2).await.unwrap_err(),
            InspectionError::Cleaning
        );
        gate.notify_one();
        assert!(tokio::time::timeout(Duration::from_millis(100), async {
            loop {
                if matches!(
                    fixture.events.recv().await,
                    Some(BackendEvent::InspectionReady { .. })
                ) {
                    break;
                }
            }
        })
        .await
        .is_err());
        assert_eq!(
            fixture.executed.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        fixture.shutdown().await;
    }

    #[tokio::test]
    async fn inspection_never_open_and_ignored_close_are_quarantined_until_teardown() {
        use crate::inspection::InspectionKind;
        for behavior in [
            InspectionBehavior::NeverOpen,
            InspectionBehavior::IgnoredClose,
        ] {
            let mut fixture = InspectionFixture::new(behavior).await;
            let _ = fixture.opened.recv().await;
            fixture.inspect(1, InspectionKind::Network).await;
            assert_eq!(fixture.opened.recv().await, Some(2));
            fixture
                .commands
                .send(AppCommand::CancelInspection {
                    session_id: fixture.session_id.clone(),
                    request_id: 1,
                })
                .await
                .unwrap();
            assert_eq!(
                fixture.result(1).await.unwrap_err(),
                InspectionError::Cancelled
            );
            fixture.inspect(2, InspectionKind::Network).await;
            assert_eq!(
                fixture.result(2).await.unwrap_err(),
                InspectionError::Cleaning
            );
            assert!(fixture.opened.try_recv().is_err());
            fixture.shutdown().await;
        }
    }

    #[tokio::test]
    async fn inspection_timeout_returns_once_and_backpressure_shutdown_does_not_hang() {
        use crate::inspection::InspectionKind;
        let mut fixture = InspectionFixture::new(InspectionBehavior::Hanging).await;
        fixture.inspect(1, InspectionKind::System).await;
        assert_eq!(
            fixture.result(1).await.unwrap_err(),
            InspectionError::Timeout
        );
        fixture.inspect(2, InspectionKind::Network).await;
        assert_eq!(
            fixture.result(2).await.unwrap_err(),
            InspectionError::Cleaning
        );
        fixture.shutdown().await;
        let fixture = InspectionFixture::new(InspectionBehavior::Normal).await;
        for id in 0..16 {
            fixture
                .commands
                .send(AppCommand::WriteInput {
                    session_id: fixture.session_id.clone(),
                    data: vec![b'x'; 1024 + id],
                })
                .await
                .unwrap();
        }
        fixture.inspect(1, InspectionKind::Network).await;
        fixture.shutdown().await;
    }

    struct MinimalSftp;
    impl russh_sftp::server::Handler for MinimalSftp {
        type Error = russh_sftp::protocol::StatusCode;
        fn unimplemented(&self) -> Self::Error {
            russh_sftp::protocol::StatusCode::OpUnsupported
        }
        async fn realpath(
            &mut self,
            id: u32,
            _: String,
        ) -> Result<russh_sftp::protocol::Name, Self::Error> {
            Ok(russh_sftp::protocol::Name {
                id,
                files: vec![russh_sftp::protocol::File {
                    filename: "/home/test".into(),
                    longname: String::new(),
                    attrs: russh_sftp::protocol::FileAttributes::empty(),
                }],
            })
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_pty_writes_resizes_and_closes_through_backend() {
        let (commands, rx) = mpsc::channel(8);
        let (events, mut results) = mpsc::channel(32);
        let backend = tokio::spawn(backend_loop(rx, events));
        let session_id = "local-pty".to_string();
        commands
            .send(AppCommand::CreateSession {
                session_id: session_id.clone(),
                config: SessionConfig {
                    shell: Some("/bin/sh".into()),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        let created = tokio::time::timeout(Duration::from_secs(5), results.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            created,
            BackendEvent::SessionCreated { session_id: id } if id == session_id
        ));
        let suffix = uuid::Uuid::new_v4();
        let marker = format!("rsterm-local-{suffix}");
        let resized_marker = format!("rsterm-resized-{suffix}");
        commands
            .send(AppCommand::WriteInput {
                session_id: session_id.clone(),
                data: format!("printf '%s%s\\n' 'rsterm-local-' '{suffix}'\n").into_bytes(),
            })
            .await
            .unwrap();
        commands
            .send(AppCommand::ResizeSession {
                session_id: session_id.clone(),
                cols: 101,
                rows: 37,
            })
            .await
            .unwrap();
        commands
            .send(AppCommand::WriteInput {
                session_id: session_id.clone(),
                data: format!("printf '%s%s\\n' 'rsterm-resized-' '{suffix}'\n").into_bytes(),
            })
            .await
            .unwrap();
        let output = tokio::time::timeout(Duration::from_secs(5), async {
            let mut output = Vec::new();
            loop {
                match results.recv().await.unwrap() {
                    BackendEvent::SessionOutput {
                        session_id: id,
                        data,
                    } => {
                        assert_eq!(id, session_id);
                        output.extend_from_slice(&data);
                        let text = String::from_utf8_lossy(&output);
                        if text.contains(&marker) && text.contains(&resized_marker) {
                            return output;
                        }
                    }
                    BackendEvent::ShellReady { .. } => {}
                    event => panic!("Unexpected local PTY event: {event:?}"),
                }
            }
        })
        .await
        .expect("Local PTY did not print both markers");
        assert!(String::from_utf8_lossy(&output).contains(&marker));
        commands
            .send(AppCommand::CloseSession {
                session_id: session_id.clone(),
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match results.recv().await.unwrap() {
                    BackendEvent::SessionClosed {
                        session_id: id,
                        reason,
                        retryable,
                    } => {
                        assert_eq!(id, session_id);
                        assert_eq!(reason, None);
                        assert!(!retryable);
                        break;
                    }
                    BackendEvent::SessionOutput { .. } => {}
                    event => panic!("Unexpected local PTY shutdown event: {event:?}"),
                }
            }
        })
        .await
        .expect("Local PTY did not close");
        commands.send(AppCommand::Shutdown).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), backend)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn private_key_authentication_accepts_only_the_authorized_key() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct KeyOnlyServer {
            expected: russh_keys::key::PublicKey,
            authenticated: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl russh::server::Handler for KeyOnlyServer {
            type Error = russh::Error;

            async fn auth_publickey_offered(
                &mut self,
                username: &str,
                key: &russh_keys::key::PublicKey,
            ) -> Result<russh::server::Auth, Self::Error> {
                Ok(if username == "key-user" && key == &self.expected {
                    russh::server::Auth::Accept
                } else {
                    russh::server::Auth::Reject {
                        proceed_with_methods: None,
                    }
                })
            }

            async fn auth_publickey(
                &mut self,
                username: &str,
                key: &russh_keys::key::PublicKey,
            ) -> Result<russh::server::Auth, Self::Error> {
                Ok(if username == "key-user" && key == &self.expected {
                    self.authenticated.fetch_add(1, Ordering::SeqCst);
                    russh::server::Auth::Accept
                } else {
                    russh::server::Auth::Reject {
                        proceed_with_methods: None,
                    }
                })
            }
        }

        let client_key = russh_keys::key::KeyPair::generate_ed25519().unwrap();
        let expected = client_key.clone_public_key().unwrap();
        let mut private_key = Vec::new();
        russh_keys::encode_pkcs8_pem(&client_key, &mut private_key).unwrap();
        let host_key = russh_keys::key::KeyPair::generate_ed25519().unwrap();
        let host_public_key = host_key.clone_public_key().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let authenticated = Arc::new(AtomicUsize::new(0));
        let server_authenticated = authenticated.clone();
        let server = tokio::spawn(async move {
            let config = Arc::new(russh::server::Config {
                keys: vec![host_key],
                auth_rejection_time: Duration::ZERO,
                auth_rejection_time_initial: Some(Duration::ZERO),
                ..Default::default()
            });
            for _ in 0..2 {
                let (socket, peer) = listener.accept().await.unwrap();
                assert!(peer.ip().is_loopback());
                let result = russh::server::run_stream(
                    config.clone(),
                    socket,
                    KeyOnlyServer {
                        expected: expected.clone(),
                        authenticated: server_authenticated.clone(),
                    },
                )
                .await
                .unwrap()
                .await;
                match result {
                    Ok(()) => {}
                    Err(russh::Error::IO(error))
                        if error.kind() == std::io::ErrorKind::UnexpectedEof => {}
                    result => panic!("Unexpected key-auth server shutdown: {result:?}"),
                }
            }
        });
        let file = tempfile::NamedTempFile::new().unwrap();
        let known_hosts = KnownHosts::load_from(file.path().to_path_buf()).unwrap();
        known_hosts
            .add("127.0.0.1", port, &host_public_key)
            .unwrap();
        let (events, mut results) = mpsc::channel(8);
        let mut config = SshConnectionConfig {
            host: "127.0.0.1".into(),
            port,
            username: "key-user".into(),
            auth: SshAuthMethod::Key {
                private_key,
                passphrase: None,
            },
            known_hosts,
            timeout: Duration::from_secs(5),
            keepalive_interval: Duration::from_secs(15),
            keepalive_max: 3,
        };
        let transport = SshTransport::connect(
            &config,
            "authorized-key".into(),
            events.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("Authorized private key should authenticate");
        assert_eq!(authenticated.load(Ordering::SeqCst), 1);
        transport.disconnect().await;
        drop(transport);

        let wrong_key = russh_keys::key::KeyPair::generate_ed25519().unwrap();
        let mut wrong_private_key = Vec::new();
        russh_keys::encode_pkcs8_pem(&wrong_key, &mut wrong_private_key).unwrap();
        config.auth = SshAuthMethod::Key {
            private_key: wrong_private_key,
            passphrase: None,
        };
        let rejected = SshTransport::connect(
            &config,
            "unauthorized-key".into(),
            events,
            CancellationToken::new(),
        )
        .await;
        assert!(matches!(rejected, Err(SshError::AuthFailed { .. })));
        assert_eq!(authenticated.load(Ordering::SeqCst), 1);
        assert!(results.try_recv().is_err());
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn sftp_initializes_after_shell_rejection_over_real_ssh() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let config = russh::server::Config {
                keys: vec![russh_keys::key::KeyPair::generate_ed25519().unwrap()],
                auth_rejection_time: Duration::ZERO,
                auth_rejection_time_initial: Some(Duration::ZERO),
                ..Default::default()
            };
            let result =
                russh::server::run_stream(Arc::new(config), socket, SftpOnlyServer::default())
                    .await
                    .unwrap()
                    .await;
            match result {
                Ok(()) => {}
                Err(russh::Error::IO(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof => {}
                result => panic!("Unexpected server shutdown: {result:?}"),
            }
        });
        let file = tempfile::NamedTempFile::new().unwrap();
        let (commands, rx) = mpsc::channel(16);
        let (events, mut results) = mpsc::channel(16);
        let backend = tokio::spawn(backend_loop(rx, events));
        commands
            .send(AppCommand::CreateSshSession {
                session_id: "sftp-only".into(),
                ssh_config: SshConnectionConfig {
                    host: "127.0.0.1".into(),
                    port,
                    username: "test".into(),
                    auth: SshAuthMethod::Password("test".into()),
                    known_hosts: KnownHosts::load_from(file.path().to_path_buf()).unwrap(),
                    timeout: Duration::from_secs(5),
                    keepalive_interval: Duration::from_secs(15),
                    keepalive_max: 3,
                },
            })
            .await
            .unwrap();
        let mut created = false;
        let mut shell_rejected = false;
        let mut ready = false;
        while !ready || !shell_rejected {
            match tokio::time::timeout(Duration::from_secs(5), results.recv())
                .await
                .unwrap()
                .unwrap()
            {
                BackendEvent::HostKeyPrompt { respond_to, .. } => respond_to.send(true).unwrap(),
                BackendEvent::SessionCreated { .. } => {
                    created = true;
                    commands
                        .send(AppCommand::SftpInit {
                            session_id: "sftp-only".into(),
                        })
                        .await
                        .unwrap();
                }
                BackendEvent::ShellUnavailable { .. } => shell_rejected = true,
                BackendEvent::SftpReady { cwd, .. } => {
                    assert_eq!(cwd, "/home/test");
                    ready = true;
                }
                event => panic!("Unexpected event: {event:?}"),
            }
        }
        assert!(created);
        commands.send(AppCommand::Shutdown).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), backend)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn unknown_session_request_returns_correlated_error() {
        let (commands, rx) = mpsc::channel(8);
        let (events, mut event_rx) = mpsc::channel(8);
        let backend = tokio::spawn(backend_loop(rx, events));
        commands
            .send(AppCommand::SftpListDir {
                session_id: "missing".into(),
                request_id: 42,
                path: "/".into(),
            })
            .await
            .unwrap();
        assert!(matches!(
            event_rx.recv().await.unwrap(),
            BackendEvent::SftpListResult {
                request_id: 42,
                result: Err(_),
                ..
            }
        ));
        commands.send(AppCommand::Shutdown).await.unwrap();
        backend.await.unwrap();
    }

    #[tokio::test]
    async fn inspection_unknown_session_rejection_is_followed_by_readiness() {
        let (commands, rx) = mpsc::channel(8);
        let (events, mut event_rx) = mpsc::channel(8);
        let backend = tokio::spawn(backend_loop(rx, events));
        commands
            .send(AppCommand::Inspect {
                session_id: "missing-inspection".into(),
                request_id: 91,
                kind: crate::inspection::InspectionKind::System,
            })
            .await
            .unwrap();
        assert!(matches!(
            event_rx.recv().await.unwrap(),
            BackendEvent::InspectionResult {
                request_id: 91,
                result: Err(InspectionError::Unavailable),
                ..
            }
        ));
        assert!(
            matches!(event_rx.recv().await.unwrap(), BackendEvent::InspectionReady { session_id } if session_id == "missing-inspection")
        );
        commands.send(AppCommand::Shutdown).await.unwrap();
        backend.await.unwrap();
    }

    #[tokio::test]
    async fn slow_handshake_does_not_block_dispatch_and_close_cancels_it() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let held = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let file = tempfile::NamedTempFile::new().unwrap();
        let (commands, rx) = mpsc::channel(8);
        let (events, mut event_rx) = mpsc::channel(8);
        let backend = tokio::spawn(backend_loop(rx, events));
        commands
            .send(AppCommand::CreateSshSession {
                session_id: "slow".into(),
                ssh_config: SshConnectionConfig {
                    host: "127.0.0.1".into(),
                    port: address.port(),
                    username: "user".into(),
                    auth: SshAuthMethod::Password("password".into()),
                    known_hosts: KnownHosts::load_from(file.path().to_path_buf()).unwrap(),
                    timeout: Duration::from_secs(60),
                    keepalive_interval: Duration::from_secs(15),
                    keepalive_max: 3,
                },
            })
            .await
            .unwrap();
        commands
            .send(AppCommand::SftpListDir {
                session_id: "missing".into(),
                request_id: 7,
                path: "/".into(),
            })
            .await
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            event,
            BackendEvent::SftpListResult {
                request_id: 7,
                result: Err(_),
                ..
            }
        ));
        commands
            .send(AppCommand::CloseSession {
                session_id: "slow".into(),
            })
            .await
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(event, BackendEvent::SessionClosed { session_id, retryable: false, .. } if session_id == "slow")
        );
        commands.send(AppCommand::Shutdown).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), backend)
            .await
            .unwrap()
            .unwrap();
        held.abort();
    }

    #[tokio::test]
    async fn slow_connection_does_not_block_another_connection_failure() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let slow_port = listener.local_addr().unwrap().port();
        let refused = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let refused_port = refused.local_addr().unwrap().port();
        drop(refused);
        let held = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let known = tempfile::NamedTempFile::new().unwrap();
        let (commands, rx) = mpsc::channel(8);
        let (events, mut results) = mpsc::channel(8);
        let backend = tokio::spawn(backend_loop(rx, events));
        for (session_id, port) in [("slow", slow_port), ("refused", refused_port)] {
            commands
                .send(AppCommand::CreateSshSession {
                    session_id: session_id.into(),
                    ssh_config: SshConnectionConfig {
                        host: "127.0.0.1".into(),
                        port,
                        username: "user".into(),
                        auth: SshAuthMethod::Agent,
                        known_hosts: KnownHosts::load_from(known.path().to_path_buf()).unwrap(),
                        timeout: Duration::from_secs(60),
                        keepalive_interval: Duration::from_secs(15),
                        keepalive_max: 3,
                    },
                })
                .await
                .unwrap();
        }
        let event = tokio::time::timeout(Duration::from_secs(2), results.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(event, BackendEvent::SessionFailed { session_id, .. } if session_id == "refused")
        );
        commands.send(AppCommand::Shutdown).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), backend)
            .await
            .unwrap()
            .unwrap();
        held.abort();
    }

    #[tokio::test]
    async fn operation_error_is_correlated_and_identifies_resolved_parent() {
        let mut mock = crate::sftp::MockSftpSessionOps::new();
        mock.expect_create_dir()
            .with(mockall::predicate::eq("/home/user/new"))
            .returning(|_| {
                Err(SftpError::PermissionDenied {
                    path: "/home/user/new".into(),
                })
            });
        let client = SftpClient::with_session(Box::new(mock), "/home/user".into());
        let (events, mut results) = mpsc::channel(8);
        run_operation(
            client,
            AppCommand::SftpMkdir {
                session_id: "s".into(),
                request_id: 9,
                path: "~/new".into(),
            },
            events,
            CancellationToken::new(),
        )
        .await;
        assert!(
            matches!(results.recv().await.unwrap(), BackendEvent::SftpOperationResult { request_id: 9, path, result: Err(_), .. } if path == "/home/user")
        );
    }

    #[tokio::test]
    async fn failed_transfer_emits_one_terminal_outcome() {
        let client = SftpClient::with_session(
            Box::new(crate::sftp::MockSftpSessionOps::new()),
            "/home".into(),
        );
        let directory = tempfile::tempdir().unwrap();
        let (events, mut results) = mpsc::channel(8);
        run_transfer(
            client,
            AppCommand::SftpUpload {
                session_id: "s".into(),
                transfer_id: 3,
                local_path: directory.path().join("missing"),
                remote_path: "file".into(),
                overwrite: false,
            },
            events,
            CancellationToken::new(),
        )
        .await;
        assert!(matches!(
            results.recv().await.unwrap(),
            BackendEvent::SftpTransferComplete {
                transfer_id: 3,
                outcome: TransferOutcome::Failed(_),
                ..
            }
        ));
        assert!(results.recv().await.is_none());
    }

    #[tokio::test]
    async fn completed_transfer_emits_one_terminal_outcome() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("empty");
        tokio::fs::write(&source, b"").await.unwrap();
        let mut mock = crate::sftp::MockSftpSessionOps::new();
        mock.expect_create_exclusive().returning(|_| {
            struct EmptyFile;
            #[async_trait::async_trait]
            impl crate::sftp::SftpFileWriteOps for EmptyFile {
                async fn write_all(&mut self, _: &[u8]) -> Result<(), SftpError> {
                    Ok(())
                }
                async fn close(&mut self) -> Result<(), SftpError> {
                    Ok(())
                }
            }
            Ok(Box::new(EmptyFile))
        });
        mock.expect_rename().times(1).returning(|_, _, _| Ok(()));
        let client = SftpClient::with_session(Box::new(mock), "/home".into());
        let (events, mut results) = mpsc::channel(8);
        run_transfer(
            client,
            AppCommand::SftpUpload {
                session_id: "s".into(),
                transfer_id: 2,
                local_path: source,
                remote_path: "file".into(),
                overwrite: false,
            },
            events,
            CancellationToken::new(),
        )
        .await;
        let mut outcomes = Vec::new();
        while let Some(event) = results.recv().await {
            if let BackendEvent::SftpTransferComplete {
                transfer_id,
                outcome,
                ..
            } = event
            {
                assert_eq!(transfer_id, 2);
                outcomes.push(outcome);
            }
        }
        assert_eq!(outcomes, vec![TransferOutcome::Completed]);
    }

    #[tokio::test]
    async fn cancelled_transfer_emits_one_terminal_outcome() {
        let client = SftpClient::with_session(
            Box::new(crate::sftp::MockSftpSessionOps::new()),
            "/home".into(),
        );
        let (events, mut rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        cancel.cancel();
        run_transfer(
            client,
            AppCommand::SftpUpload {
                session_id: "s".into(),
                transfer_id: 1,
                local_path: "missing".into(),
                remote_path: "file".into(),
                overwrite: false,
            },
            events,
            cancel,
        )
        .await;
        assert!(matches!(
            rx.recv().await.unwrap(),
            BackendEvent::SftpTransferComplete {
                transfer_id: 1,
                outcome: TransferOutcome::Cancelled,
                ..
            }
        ));
        assert!(rx.recv().await.is_none());
    }
}
