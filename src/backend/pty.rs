// src/backend/pty.rs
use std::io::{Read, Write};
use std::sync::Arc;
use std::thread;

use parking_lot::Mutex;
use portable_pty::{native_pty_system, CommandBuilder, PtyPair, PtySize};
use tokio::sync::mpsc as tokio_mpsc;

use crate::error::PtyError;

/// PTY backend trait - async push model
#[async_trait::async_trait]
pub trait PtyBackend: Send + Sync + 'static {
    async fn write(&self, data: &[u8]) -> Result<(), PtyError>;
    async fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError>;
    fn is_alive(&self) -> bool;
    fn kill(&self);
}

/// Local PTY implementation using portable-pty
pub struct LocalPty {
    pair: Arc<Mutex<PtyPair>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Option<Box<dyn portable_pty::Child + Send + Sync>>>>,
    alive: Arc<std::sync::atomic::AtomicBool>,
}

impl LocalPty {
    pub fn new(
        shell: &str,
        cols: u16,
        rows: u16,
        output_tx: tokio_mpsc::Sender<Vec<u8>>,
    ) -> Result<Self, PtyError> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::CreationFailed(e.to_string()))?;

        // Spawn shell with proper environment
        let mut cmd = CommandBuilder::new(shell);
        for (key, value) in crate::platform::pty_environment() {
            cmd.env(key, value);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::CreationFailed(e.to_string()))?;

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PtyError::CreationFailed(e.to_string()))?;

        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));

        // Spawn reader thread
        let alive_clone = alive.clone();
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PtyError::CreationFailed(e.to_string()))?;

        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        alive_clone.store(false, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                    Ok(n) => {
                        if output_tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!("PTY read error: {}", e);
                        alive_clone.store(false, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                }
            }
        });

        Ok(Self {
            pair: Arc::new(Mutex::new(pair)),
            writer: Arc::new(Mutex::new(writer)),
            child: Arc::new(Mutex::new(Some(child))),
            alive,
        })
    }
}

#[async_trait::async_trait]
impl PtyBackend for LocalPty {
    async fn write(&self, data: &[u8]) -> Result<(), PtyError> {
        let mut writer = self.writer.lock();
        writer.write_all(data)?;
        writer.flush()?;
        Ok(())
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        let pair = self.pair.lock();
        pair.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::ResizeFailed(e.to_string()))?;
        Ok(())
    }

    fn is_alive(&self) -> bool {
        self.alive.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn kill(&self) {
        if let Some(mut child) = self.child.lock().take() {
            let _ = child.kill();
        }
    }
}