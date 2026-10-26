use std::path::PathBuf;

use gpui_kit::component::{
    button::{Button, ButtonVariants},
    progress::Progress,
    ActiveTheme, Disableable, Sizable,
};
use gpui_kit::{div, prelude::*, px, Context, Render, Task, Window};
use tokio::sync::mpsc;

use crate::backend::{AppCommand, BackendEvent, TransferOutcome};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TransferDirection {
    Upload,
    Download,
}

#[derive(Clone)]
pub(super) struct TransferRequest {
    pub direction: TransferDirection,
    pub local_path: PathBuf,
    pub remote_path: String,
    pub total_bytes: u64,
}

#[derive(PartialEq, Eq)]
enum TransferStatus {
    Queued,
    Running,
    Cancelling,
    Completed,
    Cancelled,
    Failed(String),
}

impl TransferStatus {
    fn active(&self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Cancelling)
    }
}

struct Transfer {
    id: u64,
    request: TransferRequest,
    transferred: u64,
    total: u64,
    status: TransferStatus,
}

pub(super) struct TransferQueueView {
    session_id: String,
    command_tx: mpsc::Sender<AppCommand>,
    next_id: u64,
    connected: bool,
    transfers: Vec<Transfer>,
    tasks: Vec<Task<()>>,
}

impl TransferQueueView {
    pub(super) fn new(session_id: String, command_tx: mpsc::Sender<AppCommand>) -> Self {
        Self {
            session_id,
            command_tx,
            next_id: 1,
            connected: false,
            transfers: Vec::new(),
            tasks: Vec::new(),
        }
    }

    pub(super) fn set_connected(&mut self, connected: bool, cx: &mut Context<Self>) {
        self.connected = connected;
        if !connected {
            self.tasks.clear();
            for transfer in &mut self.transfers {
                if transfer.status.active() {
                    transfer.status = TransferStatus::Failed("连接已断开".into());
                }
            }
        }
        cx.notify();
    }

    pub(super) fn start(
        &mut self,
        request: TransferRequest,
        overwrite: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_id;
        self.next_id += 1;
        let command = match request.direction {
            TransferDirection::Upload => AppCommand::SftpUpload {
                session_id: self.session_id.clone(),
                transfer_id: id,
                local_path: request.local_path.clone(),
                remote_path: request.remote_path.clone(),
                overwrite,
            },
            TransferDirection::Download => AppCommand::SftpDownload {
                session_id: self.session_id.clone(),
                transfer_id: id,
                remote_path: request.remote_path.clone(),
                local_path: request.local_path.clone(),
                overwrite,
            },
        };
        self.transfers.push(Transfer {
            id,
            total: request.total_bytes,
            request,
            transferred: 0,
            status: if self.connected {
                TransferStatus::Queued
            } else {
                TransferStatus::Failed("SFTP 尚未连接".into())
            },
        });
        if self.connected {
            let command_tx = self.command_tx.clone();
            self.tasks.retain(|task| !task.is_ready());
            self.tasks.push(cx.spawn_in(window, async move |view, cx| {
                let result = command_tx.send(command).await;
                let _ = view.update_in(cx, |view, _, cx| {
                    if let Some(transfer) = view.transfers.iter_mut().find(|item| item.id == id) {
                        if transfer.status == TransferStatus::Queued {
                            transfer.status = match result {
                                Ok(()) => TransferStatus::Running,
                                Err(_) => TransferStatus::Failed("后台命令通道已关闭".into()),
                            };
                        }
                    }
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    pub(super) fn handle_event(&mut self, event: &BackendEvent, cx: &mut Context<Self>) {
        match event {
            BackendEvent::SftpTransferProgress {
                session_id,
                transfer_id,
                bytes_transferred,
                total_bytes,
            } if session_id == &self.session_id => {
                if let Some(transfer) = self
                    .transfers
                    .iter_mut()
                    .find(|item| item.id == *transfer_id)
                {
                    if transfer.status.active() {
                        transfer.transferred = *bytes_transferred;
                        transfer.total = *total_bytes;
                        if transfer.status != TransferStatus::Cancelling {
                            transfer.status = TransferStatus::Running;
                        }
                    }
                }
            }
            BackendEvent::SftpTransferComplete {
                session_id,
                transfer_id,
                outcome,
            } if session_id == &self.session_id => {
                if let Some(transfer) = self
                    .transfers
                    .iter_mut()
                    .find(|item| item.id == *transfer_id)
                {
                    transfer.status = match outcome {
                        TransferOutcome::Completed => {
                            transfer.transferred = transfer.total;
                            TransferStatus::Completed
                        }
                        TransferOutcome::Cancelled => TransferStatus::Cancelled,
                        TransferOutcome::Failed(error) => TransferStatus::Failed(error.clone()),
                    };
                }
            }
            _ => return,
        }
        cx.notify();
    }

    fn cancel(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(transfer) = self.transfers.iter_mut().find(|item| item.id == id) else {
            return;
        };
        if transfer.status != TransferStatus::Running || !self.connected {
            return;
        }
        transfer.status = TransferStatus::Cancelling;
        let command_tx = self.command_tx.clone();
        let session_id = self.session_id.clone();
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn_in(window, async move |view, cx| {
            if command_tx
                .send(AppCommand::CancelTransfer {
                    session_id,
                    transfer_id: id,
                })
                .await
                .is_err()
            {
                let _ = view.update_in(cx, |view, _, cx| {
                    if let Some(transfer) = view.transfers.iter_mut().find(|item| item.id == id) {
                        if transfer.status.active() {
                            transfer.status =
                                TransferStatus::Failed("无法取消：后台命令通道已关闭".into());
                        }
                    }
                    cx.notify();
                });
            }
        }));
        cx.notify();
    }
}

impl Render for TransferQueueView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self
            .transfers
            .iter()
            .filter(|item| item.status.active())
            .count();
        let has_finished = self.transfers.iter().any(|item| !item.status.active());
        let mut rows = div()
            .id("transfer-rows")
            .max_h(px(116.))
            .overflow_y_scroll();
        for transfer in &self.transfers {
            let id = transfer.id;
            let filename = transfer
                .request
                .local_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| transfer.request.local_path.display().to_string());
            let direction = match transfer.request.direction {
                TransferDirection::Upload => "上传",
                TransferDirection::Download => "下载",
            };
            let status = match &transfer.status {
                TransferStatus::Queued => "排队中".into(),
                TransferStatus::Running => format!(
                    "{} / {}",
                    format_bytes(transfer.transferred),
                    format_bytes(transfer.total)
                ),
                TransferStatus::Cancelling => "正在取消…".into(),
                TransferStatus::Completed => {
                    format!("已完成 · {}", format_bytes(transfer.total))
                }
                TransferStatus::Cancelled => "已取消".into(),
                TransferStatus::Failed(error) => format!("失败：{error}"),
            };
            let failed = matches!(transfer.status, TransferStatus::Failed(_));
            let percentage = if transfer.total == 0 {
                if transfer.status == TransferStatus::Completed {
                    100.
                } else {
                    0.
                }
            } else {
                transfer.transferred as f32 / transfer.total as f32 * 100.
            };
            rows = rows.child(
                div()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .w(px(66.))
                                    .child(direction),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .truncate()
                                    .child(filename),
                            )
                            .child(
                                Button::new(("cancel-transfer", id))
                                    .label("取消")
                                    .ghost()
                                    .small()
                                    .disabled(
                                        transfer.status != TransferStatus::Running
                                            || !self.connected,
                                    )
                                    .on_click(cx.listener(move |view, _, window, cx| {
                                        view.cancel(id, window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if failed {
                                cx.theme().danger
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(status),
                    )
                    .child(
                        Progress::new(("transfer-progress", id))
                            .small()
                            .value(percentage)
                            .accessibility_label(format!("{direction}进度")),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .flex_none()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("传输队列 · {active} 个进行中")),
                    )
                    .child(
                        Button::new("clear-finished-transfers")
                            .label("清除已结束")
                            .ghost()
                            .small()
                            .disabled(!has_finished)
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.transfers.retain(|item| item.status.active());
                                cx.notify();
                            })),
                    ),
            )
            .when(!self.transfers.is_empty(), |this| this.child(rows))
    }
}

pub(super) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::format_bytes;

    #[test]
    fn byte_sizes_use_binary_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1_572_864), "1.5 MiB");
    }
}
