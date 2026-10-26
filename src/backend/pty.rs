use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use parking_lot::Mutex;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc;

use crate::error::PtyError;

#[allow(clippy::double_must_use)]
#[async_trait::async_trait]
pub trait PtyBackend: Send + Sync + 'static {
    async fn write(&self, data: &[u8]) -> Result<(), PtyError>;
    async fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError>;
    fn is_alive(&self) -> bool;
    fn kill(&self);
}

pub struct LocalPty {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Option<Box<dyn portable_pty::Child + Send + Sync>>>>,
    alive: Arc<AtomicBool>,
}

impl LocalPty {
    pub fn new(
        shell: &str,
        cols: u16,
        rows: u16,
        output_tx: mpsc::Sender<Vec<u8>>,
    ) -> Result<Self, PtyError> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| PtyError::CreationFailed(error.to_string()))?;
        let mut command = CommandBuilder::new(shell);
        for argument in crate::platform::shell_args() {
            command.arg(argument);
        }
        for (key, value) in crate::platform::pty_environment() {
            command.env(key, value);
        }
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| PtyError::CreationFailed(error.to_string()))?;
        drop(pair.slave);
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| PtyError::CreationFailed(error.to_string()))?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| PtyError::CreationFailed(error.to_string()))?;
        let alive = Arc::new(AtomicBool::new(true));
        let reader_alive = alive.clone();
        thread::spawn(move || {
            let mut buffer = [0; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        if output_tx.blocking_send(buffer[..count].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
            reader_alive.store(false, Ordering::Release);
        });
        Ok(Self {
            master: Arc::new(Mutex::new(pair.master)),
            writer: Arc::new(Mutex::new(writer)),
            child: Arc::new(Mutex::new(Some(child))),
            alive,
        })
    }
}

#[async_trait::async_trait]
impl PtyBackend for LocalPty {
    async fn write(&self, data: &[u8]) -> Result<(), PtyError> {
        let writer = self.writer.clone();
        let data = data.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut writer = writer.lock();
            writer.write_all(&data)?;
            writer.flush()?;
            Ok(())
        })
        .await
        .map_err(|error| PtyError::CreationFailed(error.to_string()))?
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        let master = self.master.clone();
        tokio::task::spawn_blocking(move || {
            master
                .lock()
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|error| PtyError::ResizeFailed(error.to_string()))
        })
        .await
        .map_err(|error| PtyError::ResizeFailed(error.to_string()))?
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    fn kill(&self) {
        self.alive.store(false, Ordering::Release);
        let child = self.child.clone();
        thread::spawn(move || {
            if let Some(mut child) = child.lock().take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        });
    }
}

impl Drop for LocalPty {
    fn drop(&mut self) {
        self.kill();
    }
}
