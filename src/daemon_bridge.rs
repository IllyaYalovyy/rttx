//! Bridge between the glib main loop and the tokio runtime.
//!
//! The GTK app runs on a glib main loop. Daemon I/O uses tokio types
//! (`UnixStream`, `AsyncRead`, `AsyncWrite`) that require a tokio reactor.
//! This module provides a background tokio runtime and channel-based
//! communication to bridge the two worlds.

use crate::daemon::{DaemonConnection, DaemonError, DaemonReader, DaemonWriter};
use rttx_proto::proto;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

/// A handle to the background tokio runtime for daemon I/O.
///
/// All daemon communication goes through this bridge. The GTK thread
/// sends commands via channels; the tokio runtime executes them and
/// sends results back.
pub struct DaemonBridge {
    rt: tokio::runtime::Runtime,
    writer: Arc<Mutex<Option<DaemonWriter>>>,
}

impl std::fmt::Debug for DaemonBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonBridge").finish_non_exhaustive()
    }
}

impl DaemonBridge {
    /// Create a new bridge with a background tokio runtime.
    pub fn new() -> Result<Self, DaemonError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(DaemonError::Io)?;
        Ok(Self { rt, writer: Arc::new(Mutex::new(None)) })
    }

    /// Connect to a local daemon via Unix socket. Performs handshake.
    pub fn connect(&self, socket_path: &Path) -> Result<DaemonConnection, DaemonError> {
        self.rt.block_on(DaemonConnection::connect(socket_path))
    }

    /// Install a connection: split into reader/writer, start the reader loop.
    ///
    /// Returns a receiver for server messages that the GTK thread can poll.
    pub fn install_connection(
        &self,
        conn: DaemonConnection,
    ) -> mpsc::UnboundedReceiver<proto::ServerMessage> {
        let (reader, writer) = conn.into_split();
        *self.rt.block_on(self.writer.lock()) = Some(writer);

        let (tx, rx) = mpsc::unbounded_channel();
        self.rt.spawn(reader_loop(reader, tx));
        rx
    }

    /// Run a request/response exchange on the connection before splitting.
    ///
    /// Used for `create_session`, `attach_session`, `create_pane`, `list_sessions`.
    pub fn run<F, T>(&self, f: F) -> Result<T, DaemonError>
    where
        F: std::future::Future<Output = Result<T, DaemonError>>,
    {
        self.rt.block_on(f)
    }

    /// Send input to a pane (non-blocking from GTK thread).
    pub fn send_input(&self, session_id: Uuid, pane_id: Uuid, data: Vec<u8>) {
        let writer = Arc::clone(&self.writer);
        self.rt.spawn(async move {
            let mut guard = writer.lock().await;
            if let Some(ref mut w) = *guard
                && let Err(e) = w.send_input(session_id, pane_id, &data).await
            {
                log::error!("Failed to send input: {e}");
            }
        });
    }

    /// Send resize to a pane (non-blocking from GTK thread).
    pub fn send_resize(&self, session_id: Uuid, pane_id: Uuid, cols: u16, rows: u16) {
        let writer = Arc::clone(&self.writer);
        self.rt.spawn(async move {
            let mut guard = writer.lock().await;
            if let Some(ref mut w) = *guard
                && let Err(e) = w.send_resize(session_id, pane_id, cols, rows).await
            {
                log::error!("Failed to send resize: {e}");
            }
        });
    }

    /// Send detach (non-blocking from GTK thread).
    pub fn detach_session(&self, session_id: Uuid) {
        let writer = Arc::clone(&self.writer);
        self.rt.spawn(async move {
            let mut guard = writer.lock().await;
            if let Some(ref mut w) = *guard
                && let Err(e) = w.detach_session(session_id).await
            {
                log::error!("Failed to detach session: {e}");
            }
        });
    }
}

/// Background reader loop that forwards server messages to the GTK thread.
async fn reader_loop(mut reader: DaemonReader, tx: mpsc::UnboundedSender<proto::ServerMessage>) {
    loop {
        match reader.recv().await {
            Ok(Some(msg)) => {
                if tx.send(msg).is_err() {
                    break; // GTK side dropped the receiver
                }
            }
            Ok(None) => {
                log::warn!("Daemon connection closed");
                break;
            }
            Err(e) => {
                log::error!("Daemon read error: {e}");
                break;
            }
        }
    }
}
