//! Top-level server loop.
//!
//! Accepts client connections, routes messages to workspaces/panes, runs the
//! serialization loop, and manages the PTY read loops.

use crate::engine::Engine;
use crate::engine::PaneSpawnConfig;
use crate::engine::native::NativeEngine;
use crate::ipc::{ClientConnection, ClientConnectionReader, ClientConnectionWriter, Listener};
use crate::os::OsInterface;
use crate::pane::Pane;
use crate::protocol;
use crate::screen::{restart_safe_scrollback, strip_client_queries};
use crate::workspace::{
    AttachError, AttachMode, AttachOutcome, ClientRole, DetachOutcome, DetachReason,
    TerminationReason, Workspace, WorkspacePolicy,
};
use rttx_proto::{bytes_to_uuid, uuid_to_bytes, v3};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use uuid::Uuid;

/// Per-client protocol state negotiated during handshake.
#[derive(Debug, Clone)]
pub enum ClientProtocol {
    /// V3 client with negotiated capabilities.
    V3 { effective_caps: Vec<i32> },
}

/// A message sent to a client. With v2 removed, every client message is a
/// v3 `ServerEnvelope`.
pub type ClientMsg = v3::ServerEnvelope;

/// Return the first 8 characters of a UUID for compact log output.
#[must_use]
pub fn short_id(id: Uuid) -> String {
    id.to_string()[..8].to_string()
}

/// Capacity for the per-client push channel (Deltas, broadcasts).
/// Large enough to absorb short bursts; slow clients that fall behind
/// will have messages dropped (Deltas are replaceable by a snapshot on
/// reconnect).
pub const PUSH_CHANNEL_BOUND: usize = 4096;

/// Capacity for the per-client response channel (pong, snapshots).
const RESP_CHANNEL_BOUND: usize = 256;

/// Send a message to previously collected sender handles.
///
/// This is the lock-free counterpart of [`Server::broadcast_to_workspace`]:
/// collect handles with [`Server::collect_workspace_senders`] while holding
/// the mutex, then call this after releasing it.
///
/// Returns the IDs of clients whose push channels overflowed. The caller
/// must re-acquire the server lock and call [`Server::handle_push_overflows`]
/// for each returned client.
fn send_to_collected(
    senders: &[(Uuid, mpsc::Sender<ClientMsg>, Option<ClientProtocol>)],
    runtime_id: Uuid,
    pane_id: Uuid,
    msg: &ClientMsg,
    metrics: &crate::metrics::DaemonMetrics,
) -> Vec<Uuid> {
    let mut overflowed = Vec::new();
    for (client_id, sender, _protocol) in senders {
        if let Err(mpsc::error::TrySendError::Full(_)) =
            crate::instrument::instrumented_try_send(sender, msg.clone(), metrics)
        {
            tracing::warn!(
                "Client {} push channel full — dropping message (workspace={}, pane={})",
                short_id(*client_id),
                short_id(runtime_id),
                short_id(pane_id),
            );
            overflowed.push(*client_id);
        }
    }
    overflowed
}

/// Per-workspace lock type used throughout the server.
pub type WorkspaceLock = Arc<Mutex<Workspace>>;

/// Shared mutable server state.
///
/// Workspaces are individually locked so independent workspaces never
/// block each other.  The outer server mutex protects the registry
/// (workspace map, client maps, PTY maps) and is held only briefly for
/// lookups and structural changes.
pub struct Server {
    /// All active workspaces, each behind its own lock.
    pub workspaces: HashMap<Uuid, WorkspaceLock>,
    /// Server's own identity.
    pub server_id: Uuid,
    /// The engine used to spawn pane processes.
    pub engine: Box<dyn Engine>,
    /// OS abstraction for paths.
    pub os: Box<dyn OsInterface>,
    /// Always-on profiling metrics shared with the profiling layer.
    pub metrics: Arc<crate::metrics::DaemonMetrics>,
    /// Ring buffer writer for direct flight event recording.
    pub ring: Arc<crate::flight::RingWriter>,
    /// Per-client bounded push channels for server-initiated messages (Deltas, etc.).
    client_senders: HashMap<Uuid, mpsc::Sender<ClientMsg>>,
    /// Per-client response channels for request/response messages.
    client_resp_senders: HashMap<Uuid, mpsc::Sender<ClientMsg>>,
    /// Per-client protocol version and capabilities.
    client_protocols: HashMap<Uuid, ClientProtocol>,
    /// Per-pane PTY write handles for Input and Resize routing.
    pty_writers: HashMap<Uuid, Arc<tokio::sync::Mutex<pty_process::OwnedWritePty>>>,
    /// Per-pane kill signals to cancel PTY read loops.
    pty_kill_senders: HashMap<Uuid, oneshot::Sender<()>>,
    /// Per-client, per-pane render sizes reported via `ReportClientSize`.
    /// Drives the multi-client PTY min-size policy (RFC-031 §4). Ephemeral:
    /// cleared when the client disconnects.
    client_pane_sizes: HashMap<Uuid, HashMap<Uuid, (u16, u16)>>,
    /// Cooperative shutdown signal — set to `true` to stop the server.
    shutdown_tx: watch::Sender<bool>,
}

impl Server {
    /// Create a new server with the native engine.
    #[must_use]
    pub fn new(
        os: Box<dyn OsInterface>,
        metrics: Arc<crate::metrics::DaemonMetrics>,
        ring: Arc<crate::flight::RingWriter>,
    ) -> Self {
        let (shutdown_tx, _) = watch::channel(false);
        Self {
            workspaces: HashMap::new(),
            server_id: Uuid::new_v4(),
            engine: Box::new(NativeEngine),
            os,
            metrics,
            ring,
            client_senders: HashMap::new(),
            client_resp_senders: HashMap::new(),
            client_protocols: HashMap::new(),
            pty_writers: HashMap::new(),
            pty_kill_senders: HashMap::new(),
            client_pane_sizes: HashMap::new(),
            shutdown_tx,
        }
    }

    /// Subscribe to the shutdown signal.
    #[must_use]
    pub fn shutdown_rx(&self) -> watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    /// Trigger cooperative shutdown.
    pub fn request_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Number of connected client push channels.
    #[must_use]
    pub fn client_sender_count(&self) -> usize {
        self.client_senders.len()
    }

    /// Number of active PTY write handles.
    #[must_use]
    pub fn pty_writer_count(&self) -> usize {
        self.pty_writers.len()
    }

    /// Human-readable label for a workspace: `"name" (short_id)`.
    ///
    /// Falls back to just the short ID when the workspace is not found or
    /// its lock cannot be acquired without blocking.
    #[must_use]
    pub fn workspace_label(&self, runtime_id: Uuid) -> String {
        self.workspaces.get(&runtime_id).map_or_else(
            || format!("({})", short_id(runtime_id)),
            |rt_lock| {
                rt_lock.try_lock().map_or_else(
                    |_| format!("({})", short_id(runtime_id)),
                    |rt| format!("\"{}\" ({})", rt.name, short_id(runtime_id)),
                )
            },
        )
    }

    /// Load persisted state and resurrect workspaces.
    ///
    /// Reads v2 per-workspace files from `$XDG_STATE_HOME/rttx/daemon/`.
    /// If no v2 state exists, starts clean.
    pub fn load_persisted_state(&mut self) {
        let state_dir = self.os.state_dir();

        if let Some(result) = crate::state::persistence::load_all(&state_dir) {
            let total = result.workspaces.len() + result.failed_ids.len();
            tracing::info!(
                "Loaded {} persisted workspaces ({} skipped)",
                result.workspaces.len(),
                result.failed_ids.len(),
            );
            for rf in &result.workspaces {
                let mut rt = Workspace::from_workspace_file(rf);
                let runtime_id = rt.id;
                for pane in rt.panes.values_mut() {
                    pane.scrollback_log_path =
                        Some(crate::state::layout::scrollback_log(&state_dir, runtime_id, pane.id));
                }
                self.workspaces.insert(rt.id, Arc::new(Mutex::new(rt)));
            }

            if total > 0 || result.failed_ids.is_empty() {
                return;
            }
        }

        tracing::info!(
            "No persisted state found. Starting fresh. \
             Daemon state is stored in {} (RFC-022).",
            state_dir.display()
        );
    }

    /// Reconstruct resurrected workspaces: replay scrollback logs into pane
    /// screens and spawn fresh shells in saved working directories.
    ///
    /// Called after `load_persisted_state` once the server is wrapped in
    /// `Arc<Mutex<>>` so we can spawn PTY read loops.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn reconstruct_workspaces(server: &Arc<Mutex<Self>>) {
        enum ReplayData {
            Snapshot(crate::state::types::ScreenSnapshotV1),
            Scrollback { clean: Vec<u8>, rewrite: Option<(std::path::PathBuf, Vec<u8>)> },
            None,
        }

        // Extract metrics for instrumented lock helpers and PTY read loops.
        let metrics = { server.lock().await.metrics.clone() };

        // Phase 1: collect pane metadata and paths under the server lock.
        let replay_targets: Vec<(Uuid, Uuid, String, Option<std::path::PathBuf>)>;
        let state_dir;
        {
            let s = crate::instrument::lock_server(server, &metrics).await;
            state_dir = s.os.state_dir();
            let mut targets = Vec::new();
            for rt_lock in s.workspaces.values() {
                let rt = crate::instrument::lock_workspace(rt_lock, &metrics).await;
                let label = format!("\"{}\" ({})", rt.name, short_id(rt.id));
                for pane in rt.panes.values() {
                    targets.push((rt.id, pane.id, label.clone(), pane.scrollback_log_path.clone()));
                }
            }
            replay_targets = targets;
        }

        // Phase 2: read screen snapshots and scrollback files outside the lock.
        let mut replay_results: Vec<(Uuid, Uuid, String, ReplayData)> =
            Vec::with_capacity(replay_targets.len());
        for (runtime_id, pane_id, label, log_path) in replay_targets {
            let pane_short = short_id(pane_id);

            if let Some(snap) =
                crate::state::persistence::load_screen_snapshot(&state_dir, runtime_id, pane_id)
            {
                tracing::info!(
                    "Restoring pane {pane_short} in workspace {label} from screen snapshot ({} bytes, seq={})",
                    snap.screen_bytes.len(),
                    snap.pane_output_seq,
                );
                replay_results.push((runtime_id, pane_id, label, ReplayData::Snapshot(snap)));
                continue;
            }

            if let Some(ref path) = log_path
                && path.exists()
            {
                match std::fs::read(path) {
                    Ok(data) => {
                        let restart_safe = restart_safe_scrollback(&data);
                        let clean = strip_client_queries(restart_safe);
                        tracing::info!(
                            "Replaying {} bytes of scrollback for pane {pane_short} in workspace {label} (no snapshot)",
                            clean.len(),
                        );
                        let rewrite =
                            (clean.len() != data.len()).then(|| (path.clone(), clean.clone()));
                        replay_results.push((
                            runtime_id,
                            pane_id,
                            label,
                            ReplayData::Scrollback { clean: clean.clone(), rewrite },
                        ));
                        continue;
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to read scrollback log for pane {pane_short} in workspace {label}: {e}",
                        );
                    }
                }
            }
            replay_results.push((runtime_id, pane_id, label, ReplayData::None));
        }

        // Write cleaned scrollback files outside the lock.
        for (_, _, label, data) in &replay_results {
            if let ReplayData::Scrollback { rewrite: Some((path, clean)), .. } = data
                && let Err(e) = std::fs::write(path, clean)
            {
                tracing::error!(
                    "Failed to rewrite restart-safe scrollback in workspace {label}: {e}",
                );
            }
        }

        // Phase 3: feed replay data into pane screens using per-workspace locks.
        #[allow(clippy::type_complexity)]
        let panes_to_reconstruct: Vec<(
            Uuid,
            Uuid,
            String,
            Option<String>,
            u16,
            u16,
            bool,
        )>;
        {
            let s = crate::instrument::lock_server(server, &metrics).await;
            for (runtime_id, pane_id, _, data) in replay_results {
                if let Some(rt_lock) = s.workspaces.get(&runtime_id) {
                    let mut rt = crate::instrument::lock_workspace(rt_lock, &metrics).await;
                    if let Some(pane) = rt.panes.get_mut(&pane_id) {
                        match data {
                            ReplayData::Snapshot(snap) => pane.restore_from_snapshot(&snap),
                            ReplayData::Scrollback { clean, .. } => pane.screen.feed(&clean),
                            ReplayData::None => {}
                        }
                    }
                }
            }

            let mut targets = Vec::new();
            for rt_lock in s.workspaces.values() {
                let rt = crate::instrument::lock_workspace(rt_lock, &metrics).await;
                let name = rt.name.clone();
                for pane in rt.panes.values() {
                    targets.push((
                        rt.id,
                        pane.id,
                        name.clone(),
                        pane.cwd.clone(),
                        pane.cols,
                        pane.rows,
                        pane.no_persist,
                    ));
                }
            }
            panes_to_reconstruct = targets;
        }

        if panes_to_reconstruct.is_empty() {
            return;
        }

        tracing::info!("Reconstructing {} panes", panes_to_reconstruct.len());

        for (runtime_id, pane_id, workspace_name, cwd, cols, rows, no_persist) in
            panes_to_reconstruct
        {
            let workspace_label = format!("\"{}\" ({})", workspace_name, short_id(runtime_id));
            let pane_short = short_id(pane_id);
            let pty_result = {
                let s = crate::instrument::lock_server(server, &metrics).await;
                let (command, env) =
                    pane_spawn_parts(&s.os.state_dir(), runtime_id, pane_id, no_persist, true);
                let config = PaneSpawnConfig { command, cwd, env, cols, rows };
                s.engine.spawn_pane(pane_id, &config)
            };

            match pty_result {
                Ok(pty) => {
                    let child_pid = pty.pid();
                    let (reader, writer, child) = pty.into_parts();
                    let (kill_tx, kill_rx) = oneshot::channel();
                    {
                        let mut s = crate::instrument::lock_server(server, &metrics).await;
                        s.pty_writers.insert(pane_id, Arc::new(tokio::sync::Mutex::new(writer)));
                        s.pty_kill_senders.insert(pane_id, kill_tx);
                        if let Some(rt_lock) = s.workspaces.get(&runtime_id) {
                            let mut rt = crate::instrument::lock_workspace(rt_lock, &metrics).await;
                            let _ = rt.set_pane_exit_status(pane_id, None);
                            if let Some(pane) = rt.panes.get_mut(&pane_id) {
                                pane.child_pid = child_pid;
                            }
                        }
                    }
                    spawn_pty_read_loop(
                        Arc::clone(server),
                        runtime_id,
                        pane_id,
                        &workspace_name,
                        reader,
                        child,
                        kill_rx,
                        Arc::clone(&metrics),
                        {
                            let s = crate::instrument::lock_server(server, &metrics).await;
                            Arc::clone(&s.ring)
                        },
                    );
                    tracing::info!(
                        "Reconstructed pane {pane_short} in workspace {workspace_label}"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to reconstruct pane {pane_short} in workspace {workspace_label}: {e}"
                    );
                    let s = crate::instrument::lock_server(server, &metrics).await;
                    if let Some(rt_lock) = s.workspaces.get(&runtime_id) {
                        let mut rt = crate::instrument::lock_workspace(rt_lock, &metrics).await;
                        let _ = rt.set_pane_exit_status(pane_id, Some(-1));
                    }
                }
            }
        }
    }

    /// Send a message to the provided clients, converting v2 push messages
    /// to v3 envelopes for v3 clients.
    ///
    /// On push channel overflow:
    /// - V3 + `OPT_RESYNC`: sends `StreamOverflow` via the response channel
    /// - V2 or V3 without `OPT_RESYNC`: removes the push sender (force disconnect)
    fn broadcast_to_clients<I>(
        &mut self,
        client_ids: I,
        exclude_client_id: Option<Uuid>,
        msg: &ClientMsg,
    ) where
        I: IntoIterator<Item = Uuid>,
    {
        let mut overflowed: Vec<Uuid> = Vec::new();
        for client_id in client_ids {
            if Some(client_id) == exclude_client_id {
                continue;
            }
            let Some(sender) = self.client_senders.get(&client_id) else {
                continue;
            };
            if let Err(mpsc::error::TrySendError::Full(_)) =
                crate::instrument::instrumented_try_send(sender, msg.clone(), &self.metrics)
            {
                tracing::warn!(
                    "Client {} push channel full — dropping message",
                    short_id(client_id),
                );
                overflowed.push(client_id);
            }
        }
        for client_id in overflowed {
            self.handle_single_push_overflow(client_id);
        }
    }

    /// Collect cloned sender handles for all clients attached to a workspace.
    ///
    /// `attached_client_ids` must be extracted from the workspace while its
    /// lock is held.  The returned senders can be used after releasing both
    /// the server and workspace mutexes via [`send_to_collected`].
    fn collect_senders_for_clients(
        &self,
        attached_client_ids: &[Uuid],
    ) -> Vec<(Uuid, mpsc::Sender<ClientMsg>, Option<ClientProtocol>)> {
        attached_client_ids
            .iter()
            .filter_map(|&cid| {
                self.client_senders
                    .get(&cid)
                    .map(|s| (cid, s.clone(), self.client_protocols.get(&cid).cloned()))
            })
            .collect()
    }

    /// Look up the protocol version for a client.
    #[must_use]
    pub fn client_protocol(&self, client_id: Uuid) -> Option<&ClientProtocol> {
        self.client_protocols.get(&client_id)
    }

    /// Register a client's protocol version.
    pub fn set_client_protocol(&mut self, client_id: Uuid, protocol: ClientProtocol) {
        self.client_protocols.insert(client_id, protocol);
    }

    /// Check whether a push sender is registered for a client.
    #[must_use]
    pub fn has_client_sender(&self, client_id: Uuid) -> bool {
        self.client_senders.contains_key(&client_id)
    }

    /// Handle push channel overflow for a batch of clients.
    ///
    /// Called by the PTY read loop after `send_to_collected` reports overflows.
    /// `runtime_id` is used to build `StreamOverflow` events for resync-capable
    /// clients.
    pub fn handle_push_overflows(&mut self, overflowed: &[Uuid], runtime_id: Uuid) {
        for &client_id in overflowed {
            self.handle_single_push_overflow_for_workspace(client_id, runtime_id);
        }
    }

    /// Handle push overflow for a single client in a workspace context.
    ///
    /// V3 + `OPT_RESYNC`: sends `StreamOverflow` via the response channel.
    /// Otherwise: removes the push sender to force disconnect.
    fn handle_single_push_overflow_for_workspace(&mut self, client_id: Uuid, runtime_id: Uuid) {
        if let Some(ClientProtocol::V3 { effective_caps }) = self.client_protocols.get(&client_id)
            && rttx_proto::v3_resync::is_supported(effective_caps)
        {
            let overflow = rttx_proto::v3_resync::build_stream_overflow(runtime_id, None, 1);
            let env = rttx_proto::v3_resync::build_stream_overflow_envelope(overflow);
            if let Some(resp_tx) = self.client_resp_senders.get(&client_id) {
                if resp_tx.try_send(env).is_err() {
                    tracing::error!(
                        "Client {} resp channel also full — forcing disconnect",
                        short_id(client_id),
                    );
                    self.force_disconnect_client(client_id);
                }
                return;
            }
        }
        self.force_disconnect_client(client_id);
    }

    /// Handle push overflow for a single client (no workspace context).
    ///
    /// Used by `broadcast_to_clients` which may not have a specific `runtime_id`.
    fn handle_single_push_overflow(&mut self, client_id: Uuid) {
        // Without a workspace context we cannot build a StreamOverflow event,
        // so try to find the workspace this client is attached to.  Because
        // per-workspace locks are held independently, we use try_lock to
        // avoid blocking.
        let runtime_id = self.workspaces.iter().find_map(|(&rid, rt_lock)| {
            rt_lock
                .try_lock()
                .ok()
                .filter(|rt| rt.attached_clients.contains_key(&client_id))
                .map(|_| rid)
        });

        if let Some(rid) = runtime_id {
            self.handle_single_push_overflow_for_workspace(client_id, rid);
        } else {
            self.force_disconnect_client(client_id);
        }
    }

    /// Remove a client's push sender to force the writer task to exit.
    fn force_disconnect_client(&mut self, client_id: Uuid) {
        tracing::warn!(
            "Forcing disconnect for client {} — push channel overflow without OPT_RESYNC",
            short_id(client_id),
        );
        self.client_senders.remove(&client_id);
    }

    fn terminate_workspace(
        &mut self,
        runtime_id: Uuid,
        final_revision: u64,
        reason: TerminationReason,
        exclude_client_id: Option<Uuid>,
    ) -> Option<ClientMsg> {
        let rt_lock = self.workspaces.remove(&runtime_id)?;
        let Ok(rt) = rt_lock.try_lock() else {
            tracing::warn!(
                "Cannot terminate workspace {} — lock contended, re-inserting",
                short_id(runtime_id)
            );
            self.workspaces.insert(runtime_id, rt_lock);
            return None;
        };
        let attached_client_ids: Vec<_> = rt.attached_clients.keys().copied().collect();
        let pane_ids: Vec<_> = rt.panes.keys().copied().collect();
        drop(rt);
        for pane_id in pane_ids {
            self.pty_writers.remove(&pane_id);
            if let Some(kill_tx) = self.pty_kill_senders.remove(&pane_id) {
                let _ = kill_tx.send(());
            }
        }

        // Remove the workspace's on-disk directory in a background thread.
        let state_dir = self.os.state_dir();
        crate::state::cleanup::remove_runtime_dir_background(&state_dir, runtime_id);

        let msg = protocol::v3_workspace_terminated(runtime_id, final_revision, reason);
        self.broadcast_to_clients(attached_client_ids, exclude_client_id, &msg);
        Some(msg)
    }

    /// Handle a single v3 client envelope, returning an optional response.
    #[allow(clippy::significant_drop_tightening, clippy::too_many_lines)]
    pub async fn handle_v3_message(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        effective_caps: &[i32],
        envelope: v3::ClientEnvelope,
        metrics: &Arc<crate::metrics::DaemonMetrics>,
    ) -> Option<v3::ServerEnvelope> {
        let request_id = envelope.request_id;
        let Some(command) = envelope.command else {
            return Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(
                    v3::ErrorKind::InvalidArgument,
                    "empty envelope",
                    "Dispatch",
                ),
            ));
        };

        match command {
            v3::client_envelope::Command::Ping(ping) => {
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::Pong(v3::Pong { nonce: ping.nonce }),
                ))
            }

            v3::client_envelope::Command::ListWorkspaces(_) => {
                let s = crate::instrument::lock_server(server, metrics).await;
                let has_enriched_inventory = rttx_proto::v3_inventory::is_supported(effective_caps);
                let mut infos = Vec::with_capacity(s.workspaces.len());
                for rt_lock in s.workspaces.values() {
                    let rt = crate::instrument::lock_workspace(rt_lock, metrics).await;
                    infos.push(protocol::v3_workspace_info_for(
                        client_id,
                        &rt,
                        has_enriched_inventory,
                    ));
                }
                drop(s);
                infos.sort_by(|a, b| a.id.cmp(&b.id));
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::WorkspaceList(v3::WorkspaceList {
                        workspaces: infos,
                    }),
                ))
            }

            v3::client_envelope::Command::GetDiagnostics(_) => {
                if !rttx_proto::v3_diagnostics::is_supported(effective_caps) {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::UnsupportedCapability,
                            "OPT_DIAGNOSTICS not negotiated",
                            "GetDiagnostics",
                        ),
                    ));
                }
                let s = crate::instrument::lock_server(server, metrics).await;
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::DiagnosticsReport(
                        protocol::v3_diagnostics_report(&s),
                    ),
                ))
            }

            v3::client_envelope::Command::CreateWorkspace(req) => {
                let mut s = crate::instrument::lock_server(server, metrics).await;
                let rt = Workspace::new(req.name);
                let runtime_id = rt.id;
                let policy = WorkspacePolicy::from_v3_proto(req.policy);
                let mut rt = rt;
                rt.policy = policy;
                let label = format!("\"{}\" ({})", rt.name, short_id(runtime_id));
                let policy_str = match policy {
                    WorkspacePolicy::Persistent => "persistent",
                    WorkspacePolicy::Ephemeral => "ephemeral",
                };
                let revision = rt.revision();
                s.workspaces.insert(runtime_id, Arc::new(Mutex::new(rt)));
                tracing::info!("Workspace created: {label}, policy={policy_str}");
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::WorkspaceCreated(v3::WorkspaceCreated {
                        runtime_id: uuid_to_bytes(runtime_id),
                        workspace_revision: revision,
                    }),
                ))
            }

            v3::client_envelope::Command::AttachWorkspace(req) => {
                Self::handle_v3_attach(server, client_id, request_id, req, metrics).await
            }

            v3::client_envelope::Command::DetachWorkspace(req) => {
                Self::handle_v3_detach(server, client_id, request_id, req, metrics).await
            }

            v3::client_envelope::Command::TerminateWorkspace(req) => {
                Self::handle_v3_terminate(server, client_id, request_id, req, metrics).await
            }

            v3::client_envelope::Command::CreatePane(req) => {
                Self::handle_v3_create_pane(server, client_id, request_id, req, metrics).await
            }

            v3::client_envelope::Command::ClosePane(req) => {
                Self::handle_v3_close_pane(server, client_id, request_id, req, metrics).await
            }

            v3::client_envelope::Command::TerminalInput(input) => {
                Self::handle_v3_terminal_input(server, client_id, input, metrics).await
            }

            v3::client_envelope::Command::ResizePane(req) => {
                Self::handle_v3_resize(server, client_id, req, metrics).await
            }

            v3::client_envelope::Command::SetPaneTitle(req) => {
                Self::handle_v3_set_pane_title(server, client_id, req, metrics).await
            }

            v3::client_envelope::Command::SetPaneNoPersist(req) => {
                Self::handle_v3_set_pane_no_persist(server, client_id, req, metrics).await
            }
            v3::client_envelope::Command::SplitPane(req) => {
                Self::handle_v3_split_pane(server, client_id, request_id, req, metrics).await
            }
            v3::client_envelope::Command::ResizeSplit(req) => {
                Self::handle_v3_resize_split(server, client_id, request_id, req, metrics).await
            }
            v3::client_envelope::Command::SetFocus(req) => {
                Self::handle_v3_set_focus(server, client_id, request_id, req, metrics).await
            }
            v3::client_envelope::Command::ReportClientSize(req) => {
                Self::handle_v3_report_client_size(server, client_id, req, metrics).await
            }

            v3::client_envelope::Command::RenameWorkspace(req) => {
                let runtime_id = match bytes_to_uuid(&req.runtime_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(rttx_proto::v3_error::build_error_response(
                            request_id,
                            rttx_proto::v3_error::build_error(
                                v3::ErrorKind::InvalidArgument,
                                &e.to_string(),
                                "RenameWorkspace",
                            ),
                        ));
                    }
                };
                let rt_lock = {
                    let s = crate::instrument::lock_server(server, metrics).await;
                    match s.workspaces.get(&runtime_id) {
                        Some(rt) => Arc::clone(rt),
                        None => {
                            return Some(rttx_proto::v3_error::build_error_response(
                                request_id,
                                rttx_proto::v3_error::build_error(
                                    v3::ErrorKind::WorkspaceNotFound,
                                    "workspace not found",
                                    "RenameWorkspace",
                                ),
                            ));
                        }
                    }
                };
                let mut rt = crate::instrument::lock_workspace(&rt_lock, metrics).await;
                if !rt.client_has_write_access(client_id) {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::OwnershipConflict,
                            "workspace is currently owned by another client",
                            "RenameWorkspace",
                        ),
                    ));
                }
                let old_name = rt.name.clone();
                let revision = rt.rename(req.name.clone());
                tracing::info!(
                    "Workspace renamed: \"{}\" -> \"{}\" ({})",
                    old_name,
                    req.name,
                    short_id(runtime_id),
                );
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::WorkspaceRenamed(v3::WorkspaceRenamed {
                        runtime_id: uuid_to_bytes(runtime_id),
                        name: req.name,
                        workspace_revision: revision,
                    }),
                ))
            }

            v3::client_envelope::Command::ResyncWorkspace(req) => {
                if !rttx_proto::v3_resync::is_supported(effective_caps) {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::UnsupportedCapability,
                            "OPT_RESYNC not negotiated",
                            "ResyncWorkspace",
                        ),
                    ));
                }
                let runtime_id = match bytes_to_uuid(&req.runtime_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(rttx_proto::v3_error::build_error_response(
                            request_id,
                            rttx_proto::v3_error::build_error(
                                v3::ErrorKind::InvalidArgument,
                                &e.to_string(),
                                "ResyncWorkspace",
                            ),
                        ));
                    }
                };
                let rt_lock = {
                    let s = crate::instrument::lock_server(server, metrics).await;
                    match s.workspaces.get(&runtime_id) {
                        Some(rt) => Arc::clone(rt),
                        None => {
                            return Some(rttx_proto::v3_error::build_error_response(
                                request_id,
                                rttx_proto::v3_error::build_error(
                                    v3::ErrorKind::WorkspaceNotFound,
                                    "workspace not found",
                                    "ResyncWorkspace",
                                ),
                            ));
                        }
                    }
                };
                let rt = crate::instrument::lock_workspace(&rt_lock, metrics).await;
                let role = rt
                    .client_role(client_id)
                    .map_or(v3::WorkspaceClientRole::Unattached, ClientRole::as_v3_proto);
                let snapshot = protocol::build_v3_workspace_snapshot(&rt, runtime_id, role);
                Some(rttx_proto::v3_snapshot::build_snapshot_response(request_id, snapshot))
            }

            v3::client_envelope::Command::GetScrollback(req) => {
                if !rttx_proto::v3_scrollback::is_supported(effective_caps) {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::UnsupportedCapability,
                            "OPT_CHUNKED_SCROLLBACK not negotiated",
                            "GetScrollback",
                        ),
                    ));
                }
                let runtime_id = match bytes_to_uuid(&req.runtime_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(rttx_proto::v3_error::build_error_response(
                            request_id,
                            rttx_proto::v3_error::build_error(
                                v3::ErrorKind::InvalidArgument,
                                &e.to_string(),
                                "GetScrollback",
                            ),
                        ));
                    }
                };
                let pane_id = match bytes_to_uuid(&req.pane_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(rttx_proto::v3_error::build_error_response(
                            request_id,
                            rttx_proto::v3_error::build_error(
                                v3::ErrorKind::InvalidArgument,
                                &e.to_string(),
                                "GetScrollback",
                            ),
                        ));
                    }
                };
                let rt_lock = {
                    let s = crate::instrument::lock_server(server, metrics).await;
                    match s.workspaces.get(&runtime_id) {
                        Some(rt) => Arc::clone(rt),
                        None => {
                            return Some(rttx_proto::v3_error::build_error_response(
                                request_id,
                                rttx_proto::v3_error::build_error(
                                    v3::ErrorKind::WorkspaceNotFound,
                                    "workspace not found",
                                    "GetScrollback",
                                ),
                            ));
                        }
                    }
                };
                let rt = crate::instrument::lock_workspace(&rt_lock, metrics).await;
                let Some(pane) = rt.panes.get(&pane_id) else {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::PaneNotFound,
                            "pane not found",
                            "GetScrollback",
                        ),
                    ));
                };
                let limit = rttx_proto::v3_scrollback::cap_limit(req.limit) as usize;
                let raw = pane.screen.raw_bytes();
                let offset = req.offset as usize;
                let (data, is_last) = if offset >= raw.len() {
                    (bytes::Bytes::new(), true)
                } else {
                    let end = raw.len().min(offset + limit);
                    let chunk = &raw[offset..end];
                    (bytes::Bytes::copy_from_slice(chunk), end >= raw.len())
                };
                let chunk = rttx_proto::v3_scrollback::build_scrollback_chunk(
                    runtime_id, pane_id, req.offset, data, is_last,
                );
                Some(rttx_proto::v3_scrollback::build_scrollback_chunk_response(request_id, chunk))
            }

            v3::client_envelope::Command::TakeoverWorkspace(req) => {
                if !rttx_proto::v3_takeover::is_supported(effective_caps) {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::UnsupportedCapability,
                            "OPT_RUNTIME_TAKEOVER not negotiated",
                            "TakeoverWorkspace",
                        ),
                    ));
                }
                let runtime_id = match bytes_to_uuid(&req.runtime_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(rttx_proto::v3_error::build_error_response(
                            request_id,
                            rttx_proto::v3_error::build_error(
                                v3::ErrorKind::InvalidArgument,
                                &e.to_string(),
                                "TakeoverWorkspace",
                            ),
                        ));
                    }
                };
                let rt_lock = {
                    let s = crate::instrument::lock_server(server, metrics).await;
                    match s.workspaces.get(&runtime_id) {
                        Some(rt) => Arc::clone(rt),
                        None => {
                            return Some(rttx_proto::v3_error::build_error_response(
                                request_id,
                                rttx_proto::v3_error::build_error(
                                    v3::ErrorKind::WorkspaceNotFound,
                                    "workspace not found",
                                    "TakeoverWorkspace",
                                ),
                            ));
                        }
                    }
                };
                let mut rt = crate::instrument::lock_workspace(&rt_lock, metrics).await;
                match rt.attach_client(client_id, AttachMode::TakeOver) {
                    Ok(AttachOutcome::Attached { revision, .. }) => {
                        Some(rttx_proto::v3_takeover::build_takeover_completed_response(
                            request_id,
                            rttx_proto::v3_takeover::build_takeover_completed(runtime_id, revision),
                        ))
                    }
                    Ok(AttachOutcome::Blocked { .. }) | Err(AttachError::UnsupportedTakeOver) => {
                        Some(rttx_proto::v3_error::build_error_response(
                            request_id,
                            rttx_proto::v3_error::build_error(
                                v3::ErrorKind::OwnershipConflict,
                                "takeover failed",
                                "TakeoverWorkspace",
                            ),
                        ))
                    }
                }
            }

            v3::client_envelope::Command::Shutdown(_) => None,
        }
    }
}

/// Server capabilities advertised during v3 handshake.
pub const SERVER_CAPABILITIES: &[v3::Capability] = &[
    v3::Capability::CoreWorkspaceLifecycle,
    v3::Capability::CorePaneLifecycle,
    v3::Capability::CoreTerminalIo,
    v3::Capability::CoreTerminalModes,
    v3::Capability::CorePasteIntent,
    v3::Capability::CoreFocusEvents,
    v3::Capability::OptWorkspaceInventory,
    v3::Capability::OptResync,
    v3::Capability::OptChunkedScrollback,
    v3::Capability::OptDiagnostics,
    v3::Capability::OptWorkspaceTakeover,
];

// ── V3 dispatch helpers ─────────────────────────────────────────

#[allow(clippy::significant_drop_tightening)]
impl Server {
    #[allow(clippy::significant_drop_tightening)]
    async fn handle_v3_attach(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        request_id: u64,
        req: v3::AttachWorkspace,
        metrics: &Arc<crate::metrics::DaemonMetrics>,
    ) -> Option<v3::ServerEnvelope> {
        let runtime_id = match bytes_to_uuid(&req.runtime_id) {
            Ok(id) => id,
            Err(e) => {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::InvalidArgument,
                        &e.to_string(),
                        "AttachWorkspace",
                    ),
                ));
            }
        };
        let attach_mode = AttachMode::from_v3_proto(req.attach_mode);
        let rt_lock = {
            let s = crate::instrument::lock_server(server, metrics).await;
            match s.workspaces.get(&runtime_id) {
                Some(rt) => Arc::clone(rt),
                None => {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::WorkspaceNotFound,
                            "workspace not found",
                            "AttachWorkspace",
                        ),
                    ));
                }
            }
        };
        let mut rt = crate::instrument::lock_workspace(&rt_lock, metrics).await;
        let attach_outcome = match rt.attach_client(client_id, attach_mode) {
            Ok(outcome) => outcome,
            Err(AttachError::UnsupportedTakeOver) => {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::TakeoverRequired,
                        "use TakeoverWorkspace command",
                        "AttachWorkspace",
                    ),
                ));
            }
        };
        match attach_outcome {
            AttachOutcome::Attached { role, .. } => {
                let v3_role = role.as_v3_proto();
                let snapshot = protocol::build_v3_workspace_snapshot(&rt, runtime_id, v3_role);
                let workspace_label = format!("\"{}\" ({})", rt.name, short_id(runtime_id));
                tracing::info!(
                    "Client {} attached to workspace {workspace_label} as {role:?}",
                    short_id(client_id)
                );
                Some(rttx_proto::v3_snapshot::build_snapshot_response(request_id, snapshot))
            }
            AttachOutcome::Blocked { current_role, .. } => {
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::AttachBlocked(v3::AttachBlocked {
                        runtime_id: uuid_to_bytes(runtime_id),
                        current_client_role: current_role
                            .map_or(v3::WorkspaceClientRole::Unattached, ClientRole::as_v3_proto)
                            as i32,
                        attached_client_count: u32::try_from(rt.attached_client_count())
                            .unwrap_or(u32::MAX),
                        read_only_client_count: u32::try_from(rt.read_only_client_count())
                            .unwrap_or(u32::MAX),
                    }),
                ))
            }
        }
    }

    async fn handle_v3_detach(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        request_id: u64,
        req: v3::DetachWorkspace,
        metrics: &Arc<crate::metrics::DaemonMetrics>,
    ) -> Option<v3::ServerEnvelope> {
        let runtime_id = match bytes_to_uuid(&req.runtime_id) {
            Ok(id) => id,
            Err(e) => {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::InvalidArgument,
                        &e.to_string(),
                        "DetachWorkspace",
                    ),
                ));
            }
        };
        let rt_lock = {
            let s = crate::instrument::lock_server(server, metrics).await;
            match s.workspaces.get(&runtime_id) {
                Some(rt) => Arc::clone(rt),
                None => {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::WorkspaceNotFound,
                            "workspace not found",
                            "DetachWorkspace",
                        ),
                    ));
                }
            }
        };
        let mut rt = crate::instrument::lock_workspace(&rt_lock, metrics).await;
        match rt.detach_client(client_id, DetachReason::ExplicitRequest) {
            DetachOutcome::Detached { revision } | DetachOutcome::NotAttached { revision } => {
                let workspace_label = format!("\"{}\" ({})", rt.name, short_id(runtime_id));
                tracing::info!(
                    "Client {} detached from workspace {workspace_label}",
                    short_id(client_id)
                );
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::WorkspaceDetached(v3::WorkspaceDetached {
                        runtime_id: uuid_to_bytes(runtime_id),
                        workspace_revision: revision,
                    }),
                ))
            }
            DetachOutcome::Terminated { final_revision, reason } => {
                let workspace_label = format!("\"{}\" ({})", rt.name, short_id(runtime_id));
                tracing::info!(
                    "Client {} detached from workspace {workspace_label} (terminated: {reason:?})",
                    short_id(client_id)
                );
                drop(rt);
                let mut s = crate::instrument::lock_server(server, metrics).await;
                let _ = s.terminate_workspace(runtime_id, final_revision, reason, Some(client_id));
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::WorkspaceTerminated(v3::WorkspaceTerminated {
                        runtime_id: uuid_to_bytes(runtime_id),
                        final_revision,
                        reason: reason.as_v3_proto() as i32,
                    }),
                ))
            }
        }
    }

    async fn handle_v3_terminate(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        request_id: u64,
        req: v3::TerminateWorkspace,
        metrics: &Arc<crate::metrics::DaemonMetrics>,
    ) -> Option<v3::ServerEnvelope> {
        let runtime_id = match bytes_to_uuid(&req.runtime_id) {
            Ok(id) => id,
            Err(e) => {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::InvalidArgument,
                        &e.to_string(),
                        "TerminateWorkspace",
                    ),
                ));
            }
        };
        let mut s = crate::instrument::lock_server(server, metrics).await;
        let Some(rt_lock) = s.workspaces.get(&runtime_id) else {
            return Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(
                    v3::ErrorKind::WorkspaceNotFound,
                    "workspace not found",
                    "TerminateWorkspace",
                ),
            ));
        };
        let rt = crate::instrument::lock_workspace(rt_lock, metrics).await;
        if rt.has_write_owner() && !rt.client_has_write_access(client_id) {
            return Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(
                    v3::ErrorKind::OwnershipConflict,
                    "workspace is currently owned by another client",
                    "TerminateWorkspace",
                ),
            ));
        }
        let final_revision = rt.revision().saturating_add(1);
        let workspace_label = format!("\"{}\" ({})", rt.name, short_id(runtime_id));
        drop(rt);
        let _ = s.terminate_workspace(
            runtime_id,
            final_revision,
            TerminationReason::Explicit,
            Some(client_id),
        );
        tracing::info!("Workspace terminated: {workspace_label}");
        Some(rttx_proto::v3_envelope::build_response_envelope(
            request_id,
            v3::server_envelope::Payload::WorkspaceTerminated(v3::WorkspaceTerminated {
                runtime_id: uuid_to_bytes(runtime_id),
                final_revision,
                reason: v3::WorkspaceTerminationReason::Explicit as i32,
            }),
        ))
    }

    #[allow(clippy::significant_drop_tightening)]
    async fn handle_v3_create_pane(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        request_id: u64,
        req: v3::CreatePane,
        metrics: &Arc<crate::metrics::DaemonMetrics>,
    ) -> Option<v3::ServerEnvelope> {
        let runtime_id = match bytes_to_uuid(&req.runtime_id) {
            Ok(id) => id,
            Err(e) => {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::InvalidArgument,
                        &e.to_string(),
                        "CreatePane",
                    ),
                ));
            }
        };
        let pane_id = Uuid::new_v4();
        let no_persist = req.no_persist.unwrap_or(false);
        let (pty_result, workspace_label, cols, rows, initial_cwd) = {
            let s = crate::instrument::lock_server(server, metrics).await;
            let Some(rt_lock) = s.workspaces.get(&runtime_id) else {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::WorkspaceNotFound,
                        "workspace not found",
                        "CreatePane",
                    ),
                ));
            };
            let rt = crate::instrument::lock_workspace(rt_lock, metrics).await;
            if !rt.client_has_write_access(client_id) {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::OwnershipConflict,
                        "workspace is currently owned by another client",
                        "CreatePane",
                    ),
                ));
            }
            let label = format!("\"{}\" ({})", rt.name, short_id(runtime_id));
            let (command, env) = pane_spawn_parts(
                &s.os.state_dir(),
                runtime_id,
                pane_id,
                no_persist,
                req.dark_background.unwrap_or(true),
            );
            let cols = if req.cols > 0 { req.cols as u16 } else { 80 };
            let rows = if req.rows > 0 { req.rows as u16 } else { 24 };
            let cwd = req.cwd.or_else(|| rt.any_pane_cwd());
            let config = PaneSpawnConfig { command, cwd: cwd.clone(), env, cols, rows };
            (s.engine.spawn_pane(pane_id, &config), label, cols, rows, cwd)
        };
        match pty_result {
            Ok(pty) => {
                let child_pid = pty.pid();
                let (reader, writer, mut child) = pty.into_parts();
                let (kill_tx, kill_rx) = oneshot::channel();
                let (revision, workspace_name, metrics, ring) = {
                    let mut s = crate::instrument::lock_server(server, metrics).await;
                    let Some(rt_lock) = s.workspaces.get(&runtime_id) else {
                        let _ = child.start_kill();
                        return Some(rttx_proto::v3_error::build_error_response(
                            request_id,
                            rttx_proto::v3_error::build_error(
                                v3::ErrorKind::WorkspaceNotFound,
                                "workspace not found",
                                "CreatePane",
                            ),
                        ));
                    };
                    let mut rt = crate::instrument::lock_workspace(rt_lock, metrics).await;
                    let mut pane = Pane::new(pane_id, cols, rows);
                    pane.child_pid = child_pid;
                    pane.no_persist = no_persist;
                    pane.cwd = initial_cwd;
                    rt.add_pane(pane);
                    let revision = rt.revision();
                    let name = rt.name.clone();
                    drop(rt);
                    s.pty_writers.insert(pane_id, Arc::new(tokio::sync::Mutex::new(writer)));
                    s.pty_kill_senders.insert(pane_id, kill_tx);
                    let m = s.metrics.clone();
                    let r = Arc::clone(&s.ring);
                    (revision, name, m, r)
                };
                spawn_pty_read_loop(
                    Arc::clone(server),
                    runtime_id,
                    pane_id,
                    &workspace_name,
                    reader,
                    child,
                    kill_rx,
                    metrics,
                    ring,
                );
                tracing::info!(
                    "Pane {} created in workspace \"{}\" ({})",
                    short_id(pane_id),
                    workspace_name,
                    short_id(runtime_id)
                );
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::PaneCreated(v3::PaneCreated {
                        runtime_id: uuid_to_bytes(runtime_id),
                        pane_id: uuid_to_bytes(pane_id),
                        workspace_revision: revision,
                    }),
                ))
            }
            Err(e) => {
                tracing::error!(
                    "Failed to spawn PTY for pane {} in workspace {workspace_label}: {e}",
                    short_id(pane_id)
                );
                Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::Internal,
                        &format!("failed to spawn pane: {e}"),
                        "CreatePane",
                    ),
                ))
            }
        }
    }

    async fn handle_v3_close_pane(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        request_id: u64,
        req: v3::ClosePane,
        metrics: &Arc<crate::metrics::DaemonMetrics>,
    ) -> Option<v3::ServerEnvelope> {
        let runtime_id = match bytes_to_uuid(&req.runtime_id) {
            Ok(id) => id,
            Err(e) => {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::InvalidArgument,
                        &e.to_string(),
                        "ClosePane",
                    ),
                ));
            }
        };
        let pane_id = match bytes_to_uuid(&req.pane_id) {
            Ok(id) => id,
            Err(e) => {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::InvalidArgument,
                        &e.to_string(),
                        "ClosePane",
                    ),
                ));
            }
        };
        let mut s = crate::instrument::lock_server(server, metrics).await;
        let Some(rt_lock) = s.workspaces.get(&runtime_id) else {
            return Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(
                    v3::ErrorKind::WorkspaceNotFound,
                    "workspace not found",
                    "ClosePane",
                ),
            ));
        };
        let mut rt = crate::instrument::lock_workspace(rt_lock, metrics).await;
        if !rt.client_has_write_access(client_id) {
            return Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(
                    v3::ErrorKind::OwnershipConflict,
                    "workspace is currently owned by another client",
                    "ClosePane",
                ),
            ));
        }
        if rt.remove_pane(pane_id).is_none() {
            return Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(
                    v3::ErrorKind::PaneNotFound,
                    "pane not found",
                    "ClosePane",
                ),
            ));
        }
        let revision = rt.revision();
        let workspace_label = format!("\"{}\" ({})", rt.name, short_id(runtime_id));
        // Capture the fan-out set before releasing the workspace lock so the
        // close delta can reach every *other* attached client (RFC-031 §5):
        // their view must collapse the tree when a peer closes a pane.
        let attached = rt.attached_clients.keys().copied().collect::<Vec<_>>();
        drop(rt);
        s.pty_writers.remove(&pane_id);
        if let Some(kill_tx) = s.pty_kill_senders.remove(&pane_id) {
            let _ = kill_tx.send(());
        }
        // The pane left the tree: sweep its durable artifacts (RFC-031 §8).
        crate::state::cleanup::remove_pane_state_background(&s.os.state_dir(), runtime_id, pane_id);
        tracing::info!("Pane {} closed in workspace {workspace_label}", short_id(pane_id));
        let closed = rttx_proto::v3_tree::build_pane_closed(runtime_id, pane_id, revision);
        s.broadcast_to_clients(
            attached.iter().copied(),
            Some(client_id),
            &rttx_proto::v3_tree::build_pane_closed_push(closed.clone()),
        );
        Some(rttx_proto::v3_tree::build_pane_closed_response(request_id, closed))
    }

    async fn handle_v3_terminal_input(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        input: v3::TerminalInput,
        metrics: &Arc<crate::metrics::DaemonMetrics>,
    ) -> Option<v3::ServerEnvelope> {
        let Ok(runtime_id) = bytes_to_uuid(&input.runtime_id) else { return None };
        let Ok(pane_id) = bytes_to_uuid(&input.pane_id) else { return None };
        let (writer, resolved_bytes) = {
            let s = crate::instrument::lock_server(server, metrics).await;
            let rt_lock = s.workspaces.get(&runtime_id).cloned()?;
            let writer = s.pty_writers.get(&pane_id).cloned();
            drop(s);
            let rt = crate::instrument::lock_workspace(&rt_lock, metrics).await;
            if !rt.panes.contains_key(&pane_id) {
                return None;
            }
            if !rt.client_has_write_access(client_id) {
                return None;
            }
            let modes = rt.panes.get(&pane_id)?.screen.terminal_mode_state();
            let resolved =
                rttx_proto::v3_terminal_input::resolve_input(input.kind.as_ref(), &modes);
            (writer, resolved)
        };
        if resolved_bytes.is_empty() {
            return None;
        }
        if let Some(writer) = writer {
            let pane_short = short_id(pane_id);
            let mut w = writer.lock().await;
            if let Err(e) = w.write_all(&resolved_bytes).await {
                tracing::error!("Failed to write to PTY {pane_short}: {e}");
            }
            if let Err(e) = w.flush().await {
                tracing::error!("Failed to flush PTY {pane_short}: {e}");
            }
        }
        None
    }

    async fn handle_v3_resize(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        req: v3::ResizePane,
        metrics: &Arc<crate::metrics::DaemonMetrics>,
    ) -> Option<v3::ServerEnvelope> {
        let Ok(pane_id) = bytes_to_uuid(&req.pane_id) else { return None };
        let Ok(runtime_id) = bytes_to_uuid(&req.runtime_id) else { return None };
        let Ok(cols) = u16::try_from(req.cols) else { return None };
        let Ok(rows) = u16::try_from(req.rows) else { return None };
        let (writer, rt_lock) = {
            let s = crate::instrument::lock_server(server, metrics).await;
            let rt_lock = s.workspaces.get(&runtime_id).cloned()?;
            let writer = s.pty_writers.get(&pane_id).cloned();
            (writer, rt_lock)
        };
        {
            let rt = crate::instrument::lock_workspace(&rt_lock, metrics).await;
            if !rt.panes.contains_key(&pane_id) {
                return None;
            }
            if !rt.client_has_write_access(client_id) {
                return None;
            }
        }
        let writer = writer?;
        {
            let w = writer.lock().await;
            if let Err(e) = w.resize(pty_process::Size::new(rows, cols)) {
                tracing::error!(
                    "Failed to resize PTY {} in workspace {}: {e}",
                    short_id(pane_id),
                    short_id(runtime_id)
                );
                return None;
            }
        }
        let mut rt = crate::instrument::lock_workspace(&rt_lock, metrics).await;
        rt.resize_pane(pane_id, cols, rows)?;
        None
    }

    async fn handle_v3_set_pane_title(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        req: v3::SetPaneTitle,
        metrics: &Arc<crate::metrics::DaemonMetrics>,
    ) -> Option<v3::ServerEnvelope> {
        let Ok(runtime_id) = bytes_to_uuid(&req.runtime_id) else { return None };
        let Ok(pane_id) = bytes_to_uuid(&req.pane_id) else { return None };
        let rt_lock = {
            let s = crate::instrument::lock_server(server, metrics).await;
            s.workspaces.get(&runtime_id)?.clone()
        };
        let mut rt = crate::instrument::lock_workspace(&rt_lock, metrics).await;
        if !rt.client_has_write_access(client_id) {
            return None;
        }
        let _ = rt.set_pane_title(pane_id, req.title);
        None
    }

    async fn handle_v3_set_pane_no_persist(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        req: v3::SetPaneNoPersist,
        metrics: &Arc<crate::metrics::DaemonMetrics>,
    ) -> Option<v3::ServerEnvelope> {
        let Ok(runtime_id) = bytes_to_uuid(&req.runtime_id) else { return None };
        let Ok(pane_id) = bytes_to_uuid(&req.pane_id) else { return None };
        let rt_lock = {
            let s = crate::instrument::lock_server(server, metrics).await;
            s.workspaces.get(&runtime_id)?.clone()
        };
        let mut rt = crate::instrument::lock_workspace(&rt_lock, metrics).await;
        if !rt.client_has_write_access(client_id) {
            return None;
        }
        let _ = rt.set_pane_no_persist(pane_id, req.no_persist);
        None
    }

    // ── Tree mutations and viewport (RFC-031 §5) ────────────────

    /// Split the leaf holding `target_pane_id`, spawning a shell in a
    /// server-minted pane (RFC-031 G1). Replaces the client-identity
    /// `CreatePane` for tree growth.
    async fn handle_v3_split_pane(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        request_id: u64,
        req: v3::SplitPane,
        metrics: &Arc<crate::metrics::DaemonMetrics>,
    ) -> Option<v3::ServerEnvelope> {
        let err = |kind, msg: &str| {
            Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(kind, msg, "SplitPane"),
            ))
        };
        let Ok(runtime_id) = bytes_to_uuid(&req.runtime_id) else {
            return err(v3::ErrorKind::InvalidArgument, "invalid workspace id");
        };
        let Ok(target_pane_id) = bytes_to_uuid(&req.target_pane_id) else {
            return err(v3::ErrorKind::InvalidArgument, "invalid target pane id");
        };
        let axis = protocol::proto_axis_to_split(
            v3::PaneSplitAxis::try_from(req.axis).unwrap_or(v3::PaneSplitAxis::Horizontal),
        );
        // A client that omits the ratio gets an even split rather than a rejection.
        let ratio = if crate::pane_tree::ratio_is_valid(req.ratio) { req.ratio } else { 0.5 };
        let pane_id = Uuid::new_v4();
        let no_persist = req.no_persist.unwrap_or(false);

        let (pty_result, cols, rows, initial_cwd) = {
            let s = crate::instrument::lock_server(server, metrics).await;
            let Some(rt_lock) = s.workspaces.get(&runtime_id) else {
                return err(v3::ErrorKind::WorkspaceNotFound, "workspace not found");
            };
            let rt = crate::instrument::lock_workspace(rt_lock, metrics).await;
            if !rt.client_has_write_access(client_id) {
                return err(
                    v3::ErrorKind::OwnershipConflict,
                    "workspace is currently owned by another client",
                );
            }
            if !rt.tree.contains(crate::pane_tree::PaneId::from_uuid(target_pane_id)) {
                return err(v3::ErrorKind::PaneNotFound, "split target pane not found");
            }
            let (command, env) = pane_spawn_parts(
                &s.os.state_dir(),
                runtime_id,
                pane_id,
                no_persist,
                req.dark_background.unwrap_or(true),
            );
            let cols = if req.cols > 0 { req.cols as u16 } else { 80 };
            let rows = if req.rows > 0 { req.rows as u16 } else { 24 };
            // Inherit the working directory from the pane being split.
            let cwd = req
                .cwd
                .clone()
                .or_else(|| rt.panes.get(&target_pane_id).and_then(Pane::effective_cwd))
                .or_else(|| rt.any_pane_cwd());
            let config = PaneSpawnConfig { command, cwd: cwd.clone(), env, cols, rows };
            (s.engine.spawn_pane(pane_id, &config), cols, rows, cwd)
        };

        let pty = match pty_result {
            Ok(pty) => pty,
            Err(e) => {
                tracing::error!("Failed to spawn PTY for split pane {}: {e}", short_id(pane_id));
                return err(v3::ErrorKind::Internal, &format!("failed to spawn pane: {e}"));
            }
        };
        let child_pid = pty.pid();
        let (reader, writer, mut child) = pty.into_parts();
        let (kill_tx, kill_rx) = oneshot::channel();

        let (revision, workspace_name, read_metrics, ring) = {
            let mut s = crate::instrument::lock_server(server, metrics).await;
            let Some(rt_lock) = s.workspaces.get(&runtime_id).cloned() else {
                let _ = child.start_kill();
                return err(v3::ErrorKind::WorkspaceNotFound, "workspace not found");
            };
            let split = {
                let mut rt = crate::instrument::lock_workspace(&rt_lock, metrics).await;
                let mut pane = Pane::new(pane_id, cols, rows);
                pane.child_pid = child_pid;
                pane.no_persist = no_persist;
                pane.cwd = initial_cwd;
                rt.split_pane(target_pane_id, pane, axis, ratio).map(|revision| {
                    (
                        revision,
                        rt.name.clone(),
                        rt.attached_clients.keys().copied().collect::<Vec<_>>(),
                    )
                })
            };
            let Some((revision, name, attached)) = split else {
                // The target vanished between phases; do not leak the shell.
                let _ = child.start_kill();
                return err(v3::ErrorKind::PaneNotFound, "split target pane not found");
            };
            s.pty_writers.insert(pane_id, Arc::new(tokio::sync::Mutex::new(writer)));
            s.pty_kill_senders.insert(pane_id, kill_tx);
            let evt = rttx_proto::v3_tree::build_pane_split(
                runtime_id,
                target_pane_id,
                pane_id,
                protocol::split_axis_to_proto(axis),
                ratio,
                revision,
            );
            s.broadcast_to_clients(
                attached.iter().copied(),
                Some(client_id),
                &rttx_proto::v3_tree::build_pane_split_push(evt),
            );
            (revision, name, s.metrics.clone(), Arc::clone(&s.ring))
        };

        spawn_pty_read_loop(
            Arc::clone(server),
            runtime_id,
            pane_id,
            &workspace_name,
            reader,
            child,
            kill_rx,
            read_metrics,
            ring,
        );
        tracing::info!(
            "Pane {} split from {} in workspace \"{}\" ({})",
            short_id(pane_id),
            short_id(target_pane_id),
            workspace_name,
            short_id(runtime_id),
        );
        Some(rttx_proto::v3_tree::build_pane_split_response(
            request_id,
            rttx_proto::v3_tree::build_pane_split(
                runtime_id,
                target_pane_id,
                pane_id,
                protocol::split_axis_to_proto(axis),
                ratio,
                revision,
            ),
        ))
    }

    /// Set the logical ratio of the split addressed by `path` (RFC-031 §5).
    async fn handle_v3_resize_split(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        request_id: u64,
        req: v3::ResizeSplit,
        metrics: &Arc<crate::metrics::DaemonMetrics>,
    ) -> Option<v3::ServerEnvelope> {
        let err = |kind, msg: &str| {
            Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(kind, msg, "ResizeSplit"),
            ))
        };
        let Ok(runtime_id) = bytes_to_uuid(&req.runtime_id) else {
            return err(v3::ErrorKind::InvalidArgument, "invalid workspace id");
        };
        let Some(path) = protocol::decode_side_path(&req.path) else {
            return err(v3::ErrorKind::InvalidArgument, "invalid split path");
        };
        let mut s = crate::instrument::lock_server(server, metrics).await;
        let Some(rt_lock) = s.workspaces.get(&runtime_id).cloned() else {
            return err(v3::ErrorKind::WorkspaceNotFound, "workspace not found");
        };
        let result = {
            let mut rt = crate::instrument::lock_workspace(&rt_lock, metrics).await;
            if !rt.client_has_write_access(client_id) {
                return err(
                    v3::ErrorKind::OwnershipConflict,
                    "workspace is currently owned by another client",
                );
            }
            rt.resize_split(&path, req.ratio)
                .map(|revision| (revision, rt.attached_clients.keys().copied().collect::<Vec<_>>()))
        };
        let Some((revision, attached)) = result else {
            return err(v3::ErrorKind::InvalidArgument, "split not found or invalid ratio");
        };
        let wire_path = rttx_proto::v3_tree::decode_side_path(&req.path);
        let evt =
            rttx_proto::v3_tree::build_split_resized(runtime_id, wire_path, req.ratio, revision);
        s.broadcast_to_clients(
            attached.iter().copied(),
            Some(client_id),
            &rttx_proto::v3_tree::build_split_resized_push(evt.clone()),
        );
        Some(rttx_proto::v3_tree::build_split_resized_response(request_id, evt))
    }

    /// Update the fallback focus (default-active) pane (RFC-031 §3, §5).
    async fn handle_v3_set_focus(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        request_id: u64,
        req: v3::SetFocus,
        metrics: &Arc<crate::metrics::DaemonMetrics>,
    ) -> Option<v3::ServerEnvelope> {
        let err = |kind, msg: &str| {
            Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(kind, msg, "SetFocus"),
            ))
        };
        let Ok(runtime_id) = bytes_to_uuid(&req.runtime_id) else {
            return err(v3::ErrorKind::InvalidArgument, "invalid workspace id");
        };
        let Ok(pane_id) = bytes_to_uuid(&req.pane_id) else {
            return err(v3::ErrorKind::InvalidArgument, "invalid pane id");
        };
        let mut s = crate::instrument::lock_server(server, metrics).await;
        let Some(rt_lock) = s.workspaces.get(&runtime_id).cloned() else {
            return err(v3::ErrorKind::WorkspaceNotFound, "workspace not found");
        };
        let result = {
            let mut rt = crate::instrument::lock_workspace(&rt_lock, metrics).await;
            if !rt.client_has_write_access(client_id) {
                return err(
                    v3::ErrorKind::OwnershipConflict,
                    "workspace is currently owned by another client",
                );
            }
            rt.set_default_active_pane(pane_id)
                .map(|revision| (revision, rt.attached_clients.keys().copied().collect::<Vec<_>>()))
        };
        let Some((revision, attached)) = result else {
            return err(v3::ErrorKind::PaneNotFound, "pane not found");
        };
        let evt = rttx_proto::v3_tree::build_focus_changed(runtime_id, pane_id, revision);
        s.broadcast_to_clients(
            attached.iter().copied(),
            Some(client_id),
            &rttx_proto::v3_tree::build_focus_changed_push(evt.clone()),
        );
        Some(rttx_proto::v3_tree::build_focus_changed_response(request_id, evt))
    }

    /// Record a client's per-pane render sizes and apply the multi-client PTY
    /// min-size policy (RFC-031 §4). Ephemeral viewport state — no response.
    async fn handle_v3_report_client_size(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        req: v3::ReportClientSize,
        metrics: &Arc<crate::metrics::DaemonMetrics>,
    ) -> Option<v3::ServerEnvelope> {
        let Ok(runtime_id) = bytes_to_uuid(&req.runtime_id) else { return None };
        let reported: Vec<(Uuid, u16, u16)> = req
            .panes
            .iter()
            .filter_map(|p| {
                let id = bytes_to_uuid(&p.pane_id).ok()?;
                Some((id, u16::try_from(p.cols).unwrap_or(0), u16::try_from(p.rows).unwrap_or(0)))
            })
            .collect();
        if reported.is_empty() {
            return None;
        }

        // Phase 1: record sizes, then resolve the new min size per pane.
        let mut targets: Vec<(
            Uuid,
            u16,
            u16,
            Arc<tokio::sync::Mutex<pty_process::OwnedWritePty>>,
        )> = Vec::new();
        {
            let mut s = crate::instrument::lock_server(server, metrics).await;
            let rt_lock = s.workspaces.get(&runtime_id).cloned()?;
            {
                let rt = crate::instrument::lock_workspace(&rt_lock, metrics).await;
                rt.client_role(client_id)?;
            }
            let entry = s.client_pane_sizes.entry(client_id).or_default();
            for (id, cols, rows) in &reported {
                entry.insert(*id, (*cols, *rows));
            }
            for (id, _, _) in &reported {
                let sizes = s.client_pane_sizes.values().filter_map(|m| m.get(id).copied());
                if let (Some((cols, rows)), Some(writer)) =
                    (protocol::min_client_pane_size(sizes), s.pty_writers.get(id).cloned())
                {
                    targets.push((*id, cols, rows, writer));
                }
            }
        }

        // Phase 2: apply PTY resizes (writer lock is async; held without the
        // server lock), then record the new pane dimensions.
        for (id, cols, rows, writer) in targets {
            {
                let w = writer.lock().await;
                if let Err(e) = w.resize(pty_process::Size::new(rows, cols)) {
                    tracing::warn!("min-size PTY resize failed for pane {}: {e}", short_id(id));
                    continue;
                }
            }
            let rt_lock = {
                let s = crate::instrument::lock_server(server, metrics).await;
                s.workspaces.get(&runtime_id).cloned()
            };
            if let Some(rt_lock) = rt_lock {
                let mut rt = crate::instrument::lock_workspace(&rt_lock, metrics).await;
                let _ = rt.resize_pane(id, cols, rows);
            }
        }
        None
    }
}

/// Build the command + environment for a freshly spawned pane shell:
/// shell-correct durable history (RFC-031 §7) plus the light/dark `COLORFGBG`
/// hint. Returns the command override (empty = default login shell) and the
/// environment additions.
fn pane_spawn_parts(
    state_dir: &std::path::Path,
    runtime_id: Uuid,
    pane_id: Uuid,
    no_persist: bool,
    dark_background: bool,
) -> (Vec<String>, Vec<(String, String)>) {
    let mut spawn = crate::shell_init::build(
        &crate::shell_init::user_shell(),
        state_dir,
        runtime_id,
        pane_id,
        no_persist,
    );
    let colorfgbg = if dark_background { "15;0" } else { "0;15" };
    spawn.env.push(("COLORFGBG".into(), colorfgbg.into()));
    (spawn.command, spawn.env)
}
const COALESCE_MAX_BYTES: usize = 64 * 1024;

/// How long to wait for additional PTY data after the first read.
const COALESCE_WINDOW: Duration = Duration::from_millis(1);

/// Warn when the server mutex is held longer than this in the PTY read loop.
pub const MUTEX_HOLD_WARN_THRESHOLD: Duration = Duration::from_millis(10);

/// How often (in serialization ticks) to poll /proc/<pid>/cwd for CWD changes.
const CWD_POLL_INTERVAL_TICKS: u64 = 5;

/// How long a PTY read loop yields after detecting mutex contention.
///
/// When the Phase 1 lock hold exceeds [`MUTEX_HOLD_WARN_THRESHOLD`], the
/// read loop sleeps for this duration before continuing. This breaks the
/// convoy effect where N read loops continuously re-acquire the mutex,
/// starving input handlers and the serialization loop.
pub const CONTENTION_BACKOFF: Duration = Duration::from_micros(200);

/// Spawn a background task that reads PTY output and broadcasts Deltas.
#[allow(clippy::too_many_arguments)] // metrics parameter required for instrumentation
fn spawn_pty_read_loop(
    server: Arc<Mutex<Server>>,
    runtime_id: Uuid,
    pane_id: Uuid,
    workspace_name: &str,
    mut reader: pty_process::OwnedReadPty,
    mut child: tokio::process::Child,
    mut kill_rx: oneshot::Receiver<()>,
    metrics: Arc<crate::metrics::DaemonMetrics>,
    ring: Arc<crate::flight::RingWriter>,
) {
    let workspace_label = format!("\"{}\" ({})", workspace_name, short_id(runtime_id));
    let pane_short = short_id(pane_id);
    tokio::spawn(async move {
        // Grab the per-workspace lock once at startup so the hot path
        // never touches the server mutex for workspace access.
        let rt_lock: Option<WorkspaceLock> = {
            let s = crate::instrument::lock_server(&server, &metrics).await;
            s.workspaces.get(&runtime_id).cloned()
        };
        let Some(rt_lock) = rt_lock else {
            tracing::error!("Workspace {workspace_label} not found at PTY read loop start");
            return;
        };

        let mut buf = [0u8; 4096];
        let mut batch = bytes::BytesMut::with_capacity(COALESCE_MAX_BYTES);
        loop {
            tokio::select! {
                result = reader.read(&mut buf) => {
                    match result {
                        Ok(0) => break,
                        Ok(n) => {
                            batch.extend_from_slice(&buf[..n]);

                            // Drain additional available data within a short window.
                            if batch.len() < COALESCE_MAX_BYTES {
                                let deadline = tokio::time::Instant::now() + COALESCE_WINDOW;
                                loop {
                                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                                    if remaining.is_zero() || batch.len() >= COALESCE_MAX_BYTES {
                                        break;
                                    }
                                    match tokio::time::timeout(remaining, reader.read(&mut buf)).await {
                                        Ok(Ok(0) | Err(_)) | Err(_) => break,
                                        Ok(Ok(n)) => batch.extend_from_slice(&buf[..n]),
                                    }
                                }
                            }

                            let data = batch.split().freeze();
                            let batch_len = data.len() as u64;
                            metrics.bytes_read_from_pty.fetch_add(batch_len, std::sync::atomic::Ordering::Relaxed);
                            let pty_batch_start = std::time::Instant::now();
                            let mut pane_context = [0u8; 16];
                            pane_context[..16].copy_from_slice(pane_id.as_bytes());

                            // Phase 1: accept raw bytes under the per-workspace lock
                            // (fast memcpy, no VTE parsing).  The server mutex is
                            // only touched briefly to collect client senders.
                            let (mut taken_screen, senders, output_seq, contended) = {
                                let lock_start = std::time::Instant::now();
                                let mut rt = crate::instrument::lock_workspace(&rt_lock, &metrics).await;
                                let (screen, seq) = if let Some(pane) = rt.panes.get_mut(&pane_id) {
                                    pane.accept_output(&data);
                                    (Some(pane.take_screen()), pane.output_seq)
                                } else {
                                    (None, 0)
                                };
                                let client_ids: Vec<Uuid> =
                                    rt.attached_clients.keys().copied().collect();
                                drop(rt);
                                let s = crate::instrument::lock_server(&server, &metrics).await;
                                let senders = s.collect_senders_for_clients(&client_ids);
                                drop(s);
                                let hold = lock_start.elapsed();
                                if hold > MUTEX_HOLD_WARN_THRESHOLD {
                                    tracing::warn!(
                                        hold_ms = hold.as_millis() as u64,
                                        pane = %pane_short,
                                        workspace = %workspace_label,
                                        "mutex held too long in PTY read loop",
                                    );
                                }
                                (screen, senders, seq, hold > MUTEX_HOLD_WARN_THRESHOLD)
                            };

                            // Adaptive throttle: yield when contention is detected
                            // so other tasks (input, serialization) can acquire the
                            // mutex instead of being starved by N read loops.
                            if contended {
                                tokio::time::sleep(CONTENTION_BACKOFF).await;
                            }

                            // Phase 2: VTE parsing outside any lock.
                            if let Some(ref mut screen) = taken_screen {
                                let _vte_span = tracing::info_span!(
                                    target: "rttx_profile",
                                    "vte.parse",
                                    span_kind = "vte_parse",
                                    pane_id = %pane_short,
                                    bytes_parsed = batch_len,
                                ).entered();
                                screen.parse(&data);
                            }

                            // Phase 3: return parsed screen under per-workspace lock,
                            // collect PTY writer from server.
                            let (new_cwd, new_title, pending_replies, pty_writer) = {
                                let mut rt = crate::instrument::lock_workspace(&rt_lock, &metrics).await;
                                if let Some(screen) = taken_screen
                                    && let Some(pane) = rt.panes.get_mut(&pane_id)
                                {
                                    let result = pane.return_screen(screen);
                                    let cwd = result.new_cwd.and_then(|cwd| {
                                        let rev = rt.set_pane_cwd(pane_id, &cwd)?;
                                        Some((cwd, rev))
                                    });
                                    let title = result.new_title.and_then(|title| {
                                        let rev = rt.set_pane_title(pane_id, title.clone())?;
                                        Some((title, rev))
                                    });
                                    let needs_writer = !result.pending_replies.is_empty();
                                    drop(rt);
                                    let writer = if needs_writer {
                                        let s = crate::instrument::lock_server(&server, &metrics).await;
                                        s.pty_writers.get(&pane_id).cloned()
                                    } else {
                                        None
                                    };
                                    (cwd, title, result.pending_replies, writer)
                                } else {
                                    (None, None, Vec::new(), None)
                                }
                            };

                            // Phase 4: write DSR replies without any lock.
                            if let Some(writer) = pty_writer {
                                let mut w = writer.lock().await;
                                for reply in &pending_replies {
                                    if let Err(e) = w.write_all(reply).await {
                                        tracing::error!(
                                            "Failed to write DSR reply to PTY {pane_short} in workspace {workspace_label}: {e}"
                                        );
                                    }
                                }
                                if let Err(e) = w.flush().await {
                                    tracing::error!(
                                        "Failed to flush DSR reply to PTY {pane_short} in workspace {workspace_label}: {e}"
                                    );
                                }
                            }

                            // Phase 5: broadcast to clients without any lock.
                            let client_data = crate::screen::strip_client_queries(&data);
                            let mut all_overflows = Vec::new();
                            if !client_data.is_empty() {
                                let msg = protocol::v3_delta(runtime_id, pane_id, bytes::Bytes::from(client_data.clone()), output_seq);
                                all_overflows.extend(send_to_collected(&senders, runtime_id, pane_id, &msg, &metrics));
                            }
                            if let Some((cwd, revision)) = new_cwd {
                                let msg = protocol::v3_cwd_changed(runtime_id, pane_id, cwd, revision);
                                all_overflows.extend(send_to_collected(&senders, runtime_id, pane_id, &msg, &metrics));
                            }
                            if let Some((title, revision)) = new_title {
                                let msg = protocol::v3_title_changed(runtime_id, pane_id, title, revision);
                                all_overflows.extend(send_to_collected(&senders, runtime_id, pane_id, &msg, &metrics));
                            }
                            if !all_overflows.is_empty() {
                                all_overflows.sort_unstable();
                                all_overflows.dedup();
                                let mut s = crate::instrument::lock_server(&server, &metrics).await;
                                s.handle_push_overflows(&all_overflows, runtime_id);
                            }

                            metrics.pty_read_latency_us.record(pty_batch_start.elapsed().as_micros() as u64);
                            ring.record(&crate::flight::FlightEvent {
                                timestamp_ns: metrics.epoch.elapsed().as_nanos() as u64,
                                span_id: 0,
                                event_type: crate::flight::EventType::Exit,
                                span_kind: crate::flight::SpanKind::PtyRead,
                                context: pane_context,
                                value: pty_batch_start.elapsed().as_nanos() as u64,
                            });
                        }
                        Err(e) => {
                            tracing::error!("PTY read error for pane {pane_short} in workspace {workspace_label}: {e}");
                            break;
                        }
                    }
                }
                _ = &mut kill_rx => {
                    let _ = child.start_kill();
                    tracing::info!("PTY read loop cancelled for pane {pane_short} in workspace {workspace_label}");
                    return;
                }
            }
        }

        // Child exited naturally — collect exit status.
        let status = match child.wait().await {
            Ok(s) => s.code().unwrap_or(-1),
            Err(e) => {
                tracing::error!(
                    "Failed to wait on child for pane {pane_short} in workspace {workspace_label}: {e}"
                );
                -1
            }
        };

        // Feed terminal cleanup and broadcast exit under per-workspace lock.
        {
            let mut rt = crate::instrument::lock_workspace(&rt_lock, &metrics).await;
            if let Some(pane) = rt.panes.get_mut(&pane_id) {
                pane.feed_cleanup();
            }
            let client_ids: Vec<Uuid> = rt.attached_clients.keys().copied().collect();
            drop(rt);

            let cleanup_delta = protocol::v3_delta(
                runtime_id,
                pane_id,
                bytes::Bytes::from_static(crate::screen::terminal_cleanup_bytes()),
                0,
            );
            let s = crate::instrument::lock_server(&server, &metrics).await;
            let senders = s.collect_senders_for_clients(&client_ids);
            drop(s);
            for (_, sender, _) in &senders {
                let _ = sender.try_send(cleanup_delta.clone());
            }
        }

        {
            let mut rt = crate::instrument::lock_workspace(&rt_lock, &metrics).await;
            let exit_msg = {
                let msg = rt.set_pane_exit_status(pane_id, Some(status)).map(|revision| {
                    protocol::v3_pane_exited(runtime_id, pane_id, status, revision)
                });
                if let Some(pane) = rt.panes.get_mut(&pane_id) {
                    pane.release_scrollback();
                }
                msg
            };
            let client_ids: Vec<Uuid> = rt.attached_clients.keys().copied().collect();
            drop(rt);

            if let Some(msg) = exit_msg {
                let s = crate::instrument::lock_server(&server, &metrics).await;
                let senders = s.collect_senders_for_clients(&client_ids);
                drop(s);
                for (_, sender, _) in &senders {
                    let _ = sender.try_send(msg.clone());
                }
            }
        }

        {
            let mut s = crate::instrument::lock_server(&server, &metrics).await;
            s.pty_writers.remove(&pane_id);
            s.pty_kill_senders.remove(&pane_id);
        }

        tracing::info!(
            "PTY exited for pane {pane_short} in workspace {workspace_label}, status {status}"
        );
    });
}
/// Run the serialization loop, writing state to disk every `interval`.
///
/// Uses v2 per-workspace files with symlink-based backup.
///
/// Dirty-flag optimization (RFC-022 §5): only workspaces where
/// `revision > persisted_revision` are written. The daemon index is
/// rewritten only when the set of workspace IDs changes.
///
/// Stops when the shutdown signal fires.
pub async fn serialization_loop(
    server: Arc<Mutex<Server>>,
    interval: Duration,
    shutdown_rx: &mut watch::Receiver<bool>,
    ring: Arc<crate::flight::RingWriter>,
) {
    let metrics = { server.lock().await.metrics.clone() };
    let mut ticker = tokio::time::interval(interval);
    let mut diagnostics_counter = 0u64;
    let mut last_persisted_ids: Vec<Uuid> = Vec::new();
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = shutdown_rx.changed() => {
                tracing::info!("Serialization loop stopping (shutdown)");
                return;
            }
        }

        metrics.serialization_ticks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tick_start = std::time::Instant::now();

        let s = crate::instrument::lock_server(&server, &metrics).await;
        let state_dir = s.os.state_dir();

        // Phase 1: drain pending scrollback bytes using per-workspace locks.
        let mut flush_jobs: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();
        let workspace_entries: Vec<(Uuid, WorkspaceLock)> =
            s.workspaces.iter().map(|(&id, rt)| (id, Arc::clone(rt))).collect();
        drop(s);

        for (runtime_id, rt_lock) in &workspace_entries {
            let mut rt = crate::instrument::lock_workspace(rt_lock, &metrics).await;
            for pane in rt.panes.values_mut() {
                if !pane.has_pending_flush() || pane.no_persist {
                    if pane.no_persist {
                        let _ = pane.take_pending_flush();
                    }
                    continue;
                }
                let path = crate::state::layout::scrollback_log(&state_dir, *runtime_id, pane.id);
                let data = pane.take_pending_flush();
                pane.scrollback_log_path = Some(path.clone());
                flush_jobs.push((path, data));
            }
        }

        // Log diagnostics every 30 ticks (~30 seconds at 1s interval).
        diagnostics_counter += 1;
        if diagnostics_counter.is_multiple_of(30) {
            let s = crate::instrument::lock_server(&server, &metrics).await;
            s.log_diagnostics();
        }

        // CWD polling via /proc/<pid>/cwd every 5 ticks (~5s).
        // Detects CWD changes when OSC 7 is not emitted by the shell.
        if diagnostics_counter.is_multiple_of(CWD_POLL_INTERVAL_TICKS) {
            let mut cwd_changes: Vec<(Uuid, Uuid, String, u64, Vec<Uuid>)> = Vec::new();
            for (runtime_id, rt_lock) in &workspace_entries {
                let mut rt = crate::instrument::lock_workspace(rt_lock, &metrics).await;
                let client_ids: Vec<Uuid> = rt.attached_clients.keys().copied().collect();
                if client_ids.is_empty() {
                    continue;
                }
                let pane_ids: Vec<Uuid> = rt.panes.keys().copied().collect();
                for pane_id in pane_ids {
                    let Some(pane) = rt.panes.get(&pane_id) else { continue };
                    if pane.is_exited() || pane.child_pid.is_none() {
                        continue;
                    }
                    let proc_cwd = pane.read_proc_cwd();
                    if let Some(ref new_cwd) = proc_cwd
                        && pane.cwd.as_deref() != Some(new_cwd.as_str())
                    {
                        let cwd_val = new_cwd.clone();
                        let Some(pane) = rt.panes.get_mut(&pane_id) else { continue };
                        pane.cwd = Some(cwd_val.clone());
                        if let Some(rev) = rt.set_pane_cwd(pane_id, &cwd_val) {
                            cwd_changes.push((
                                *runtime_id,
                                pane_id,
                                cwd_val,
                                rev,
                                client_ids.clone(),
                            ));
                        }
                    }
                }
            }
            if !cwd_changes.is_empty() {
                let mut s = crate::instrument::lock_server(&server, &metrics).await;
                for (runtime_id, pane_id, cwd, revision, client_ids) in &cwd_changes {
                    let msg =
                        protocol::v3_cwd_changed(*runtime_id, *pane_id, cwd.clone(), *revision);
                    s.broadcast_to_clients(client_ids.iter().copied(), None, &msg);
                }
            }
        }

        // Collect v2 workspace files only for dirty persistent workspaces.
        // Screen snapshots are written for ALL persistent workspaces every 30
        // ticks (~30s) to ensure terminal state survives hard kills even when
        // no metadata changes (title/CWD/attach) bump the revision.
        let mut dirty_runtime_files = Vec::new();
        let mut screen_snapshots = Vec::new();
        let mut current_ids = Vec::new();
        let snapshot_due = diagnostics_counter.is_multiple_of(30);

        for (runtime_id, rt_lock) in &workspace_entries {
            let rt = crate::instrument::lock_workspace(rt_lock, &metrics).await;
            if rt.policy == WorkspacePolicy::Persistent {
                current_ids.push(*runtime_id);
                if rt.is_dirty() {
                    dirty_runtime_files.push(rt.to_workspace_file());
                    for pane in rt.panes.values() {
                        screen_snapshots.push((*runtime_id, pane.to_screen_snapshot()));
                    }
                } else if snapshot_due {
                    // Periodic snapshot: capture screen state even when
                    // workspace metadata hasn't changed.
                    for pane in rt.panes.values() {
                        screen_snapshots.push((*runtime_id, pane.to_screen_snapshot()));
                    }
                }
            }
        }

        current_ids.sort();
        let index_changed = current_ids != last_persisted_ids;

        // Phase 2: flush scrollback to disk outside the lock.
        for (path, data) in &flush_jobs {
            let _flush_span = tracing::info_span!(
                target: "rttx_profile",
                "io.flush",
                span_kind = "io_flush",
                path = %path.display(),
            )
            .entered();
            if let Err(e) = crate::pane::write_scrollback_to_disk(path, data) {
                tracing::error!("Failed to flush scrollback to {}: {e}", path.display());
            }
        }

        // Write v2 daemon index only when workspace IDs changed.
        if index_changed {
            let _flush_span = tracing::info_span!(
                target: "rttx_profile",
                "io.flush",
                span_kind = "io_flush",
                path = %state_dir.join("index.json").display(),
            )
            .entered();
            if let Err(e) = crate::state::persistence::save_daemon_index(&state_dir, &current_ids) {
                tracing::error!("Failed to write v2 daemon index: {e}");
            } else {
                last_persisted_ids = current_ids;
            }
        }

        // Write only dirty workspaces.
        let written_ids: Vec<Uuid> = dirty_runtime_files.iter().map(|rf| rf.spec.id).collect();
        for rf in &dirty_runtime_files {
            let _flush_span = tracing::info_span!(
                target: "rttx_profile",
                "io.flush",
                span_kind = "io_flush",
                path = %state_dir.join("workspaces").join(rf.spec.id.to_string()).display(),
            )
            .entered();
            if let Err(e) = crate::state::persistence::save_workspace(&state_dir, rf) {
                tracing::error!("Failed to write v2 workspace {}: {e}", short_id(rf.spec.id));
            }
        }

        // Write screen snapshots for dirty workspaces.
        for (runtime_id, snap) in &screen_snapshots {
            let _flush_span = tracing::info_span!(
                target: "rttx_profile",
                "io.flush",
                span_kind = "io_flush",
                path = %state_dir.join("workspaces").join(runtime_id.to_string()).join("screen").display(),
            ).entered();
            if let Err(e) =
                crate::state::persistence::save_screen_snapshot(&state_dir, *runtime_id, snap)
            {
                tracing::error!(
                    "Failed to write screen snapshot for pane {} in workspace {}: {e}",
                    short_id(snap.pane_id),
                    short_id(*runtime_id)
                );
            }
        }

        // Mark successfully written workspaces as persisted.
        if !written_ids.is_empty() {
            let s = crate::instrument::lock_server(&server, &metrics).await;
            for id in &written_ids {
                if let Some(rt_lock) = s.workspaces.get(id) {
                    let mut rt = crate::instrument::lock_workspace(rt_lock, &metrics).await;
                    rt.mark_persisted();
                }
            }
        }

        metrics.serialization_tick_latency_us.record(tick_start.elapsed().as_micros() as u64);
        ring.record(&crate::flight::FlightEvent {
            timestamp_ns: metrics.epoch.elapsed().as_nanos() as u64,
            span_id: 0,
            event_type: crate::flight::EventType::Exit,
            span_kind: crate::flight::SpanKind::SerializationTick,
            context: [0; 16],
            value: tick_start.elapsed().as_nanos() as u64,
        });
    }
}

/// Persist final state and flush all scrollback to disk.
///
/// Writes all persistent workspaces unconditionally (ignoring dirty flags)
/// because this is the last chance before shutdown.
pub async fn persist_and_cleanup(server: &Arc<Mutex<Server>>) {
    let metrics = { server.lock().await.metrics.clone() };
    let s = crate::instrument::lock_server(server, &metrics).await;
    let state_dir = s.os.state_dir();

    // Collect workspace locks and drain pending scrollback.
    let workspace_entries: Vec<(Uuid, WorkspaceLock)> =
        s.workspaces.iter().map(|(&id, rt)| (id, Arc::clone(rt))).collect();
    drop(s);

    let mut flush_jobs: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();
    for (runtime_id, rt_lock) in &workspace_entries {
        let mut rt = crate::instrument::lock_workspace(rt_lock, &metrics).await;
        for pane in rt.panes.values_mut() {
            if !pane.has_pending_flush() || pane.no_persist {
                if pane.no_persist {
                    let _ = pane.take_pending_flush();
                }
                continue;
            }
            let path = crate::state::layout::scrollback_log(&state_dir, *runtime_id, pane.id);
            let data = pane.take_pending_flush();
            pane.scrollback_log_path = Some(path.clone());
            flush_jobs.push((path, data));
        }
    }

    // Collect v2 per-workspace files (all persistent, not just dirty).
    let mut runtime_files = Vec::new();
    let mut screen_snapshots = Vec::new();

    for (runtime_id, rt_lock) in &workspace_entries {
        let mut rt = crate::instrument::lock_workspace(rt_lock, &metrics).await;
        if rt.policy == WorkspacePolicy::Persistent {
            runtime_files.push(rt.to_workspace_file());
            for pane in rt.panes.values() {
                screen_snapshots.push((*runtime_id, pane.to_screen_snapshot()));
            }
            rt.mark_persisted();
        }
    }
    // All I/O happens outside any lock.
    for (path, data) in &flush_jobs {
        if let Err(e) = crate::pane::write_scrollback_to_disk(path, data) {
            tracing::error!("Failed to flush scrollback to {} on shutdown: {e}", path.display());
        }
    }

    let ids: Vec<_> = runtime_files.iter().map(|rf| rf.spec.id).collect();
    if let Err(e) = crate::state::persistence::save_daemon_index(&state_dir, &ids) {
        tracing::error!("Failed to write v2 daemon index on shutdown: {e}");
    }
    for rf in &runtime_files {
        if let Err(e) = crate::state::persistence::save_workspace(&state_dir, rf) {
            tracing::error!(
                "Failed to write v2 workspace {} on shutdown: {e}",
                short_id(rf.spec.id)
            );
        }
    }
    for (runtime_id, snap) in &screen_snapshots {
        if let Err(e) =
            crate::state::persistence::save_screen_snapshot(&state_dir, *runtime_id, snap)
        {
            tracing::error!(
                "Failed to write screen snapshot for pane {} on shutdown: {e}",
                short_id(snap.pane_id)
            );
        }
    }

    tracing::info!("Final state persisted");
}

/// Maximum number of concurrent client connections.
///
/// Far more than any normal usage (1–5 GUI clients). Protects against
/// resource exhaustion from connection bursts.
pub const MAX_CONCURRENT_CLIENTS: usize = 128;

/// Run the main server loop: accept clients, handle messages, manage PTYs.
///
/// Returns when a cooperative shutdown is signaled (via `Shutdown` message
/// or OS signal). The caller is responsible for process-level cleanup
/// (PID file removal, `process::exit`).
pub async fn run(server: Arc<Mutex<Server>>) -> anyhow::Result<()> {
    let metrics = { server.lock().await.metrics.clone() };
    let (socket_path, mut shutdown_rx) = {
        let s = crate::instrument::lock_server(&server, &metrics).await;
        (s.os.runtime_dir().join("rttx-server.sock"), s.shutdown_rx())
    };

    let listener = Listener::bind(&socket_path)?;
    tracing::info!("Listening on {}", socket_path.display());

    // Start serialization loop.
    let ser_server = Arc::clone(&server);
    let ser_ring = { server.lock().await.ring.clone() };
    let mut ser_shutdown_rx = shutdown_rx.clone();
    tokio::spawn(async move {
        serialization_loop(ser_server, Duration::from_secs(1), &mut ser_shutdown_rx, ser_ring)
            .await;
    });

    let connection_limit = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_CLIENTS));

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (conn, peer_pid) = result?;
                let Ok(permit) = connection_limit.clone().try_acquire_owned() else {
                    tracing::warn!("Connection limit reached ({MAX_CONCURRENT_CLIENTS}), rejecting client");
                    continue;
                };
                let server = Arc::clone(&server);
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = handle_client(server, conn, peer_pid).await;
                });
            }
            _ = shutdown_rx.changed() => {
                tracing::info!("Shutdown signal received, persisting state...");
                break;
            }
        }
    }

    persist_and_cleanup(&server).await;
    Ok(())
}

/// Extract the command type name from a v3 `ClientEnvelope` for profiling.
const fn v3_msg_type(envelope: &v3::ClientEnvelope) -> &'static str {
    match &envelope.command {
        Some(v3::client_envelope::Command::Ping(_)) => "Ping",
        Some(v3::client_envelope::Command::ListWorkspaces(_)) => "ListWorkspaces",
        Some(v3::client_envelope::Command::CreateWorkspace(_)) => "CreateWorkspace",
        Some(v3::client_envelope::Command::AttachWorkspace(_)) => "AttachWorkspace",
        Some(v3::client_envelope::Command::DetachWorkspace(_)) => "DetachWorkspace",
        Some(v3::client_envelope::Command::TerminateWorkspace(_)) => "TerminateWorkspace",
        Some(v3::client_envelope::Command::CreatePane(_)) => "CreatePane",
        Some(v3::client_envelope::Command::ClosePane(_)) => "ClosePane",
        Some(v3::client_envelope::Command::TerminalInput(_)) => "TerminalInput",
        Some(v3::client_envelope::Command::ResizePane(_)) => "ResizePane",
        Some(v3::client_envelope::Command::SetPaneTitle(_)) => "SetPaneTitle",
        Some(v3::client_envelope::Command::SetPaneNoPersist(_)) => "SetPaneNoPersist",
        Some(v3::client_envelope::Command::SplitPane(_)) => "SplitPane",
        Some(v3::client_envelope::Command::ResizeSplit(_)) => "ResizeSplit",
        Some(v3::client_envelope::Command::SetFocus(_)) => "SetFocus",
        Some(v3::client_envelope::Command::ReportClientSize(_)) => "ReportClientSize",
        Some(v3::client_envelope::Command::Shutdown(_)) => "Shutdown",
        Some(v3::client_envelope::Command::GetDiagnostics(_)) => "GetDiagnostics",
        Some(v3::client_envelope::Command::RenameWorkspace(_)) => "RenameWorkspace",
        Some(v3::client_envelope::Command::TakeoverWorkspace(_)) => "TakeoverWorkspace",
        Some(v3::client_envelope::Command::ResyncWorkspace(_)) => "ResyncWorkspace",
        Some(v3::client_envelope::Command::GetScrollback(_)) => "GetScrollback",
        None => "Empty",
    }
}

/// Handle a single stdio client (for `attach-stdio` SSH tunneling).
///
/// Serves one client over stdin/stdout using the same protocol as the
/// Unix socket path. The server must already be running (workspaces loaded,
/// PTYs reconstructed).
pub async fn handle_stdio_client(server: Arc<Mutex<Server>>) -> anyhow::Result<()> {
    let stream = crate::ipc::StdioStream::new();
    let conn = ClientConnection::new(stream);
    handle_client(server, conn, None).await
}

#[allow(clippy::significant_drop_tightening)]
async fn handle_client<S>(
    server: Arc<Mutex<Server>>,
    mut conn: ClientConnection<S>,
    _peer_pid: Option<u32>,
) -> anyhow::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let client_id = Uuid::new_v4();
    let client_short = short_id(client_id);

    let (tx, rx) = mpsc::channel(PUSH_CHANNEL_BOUND);
    let (resp_tx, resp_rx) = mpsc::channel::<ClientMsg>(RESP_CHANNEL_BOUND);
    let metrics = { server.lock().await.metrics.clone() };
    metrics.connected_clients.fetch_add(1, Ordering::Relaxed);
    {
        let mut s = crate::instrument::lock_server(&server, &metrics).await;
        s.client_senders.insert(client_id, tx);
        s.client_resp_senders.insert(client_id, resp_tx.clone());
    }

    let _session_span = tracing::info_span!(
        target: "rttx_profile",
        "client.session",
        span_kind = "client_session",
        client_id = %client_short,
    );

    // Read the first raw frame to detect v2 vs v3 protocol.
    let Some(raw_frame) = conn.read_raw_frame().await? else {
        tracing::debug!("Client probe from {client_short} (disconnected before handshake)");
        let mut s = crate::instrument::lock_server(&server, &metrics).await;
        s.client_senders.remove(&client_id);
        s.client_resp_senders.remove(&client_id);
        metrics.connected_clients.fetch_sub(1, Ordering::Relaxed);
        metrics.client_disconnects.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    };

    // Detect the v3 ClientHello handshake; any non-v3 first frame is rejected.
    let is_v3 =
        try_v3_handshake(&server, client_id, &client_short, &mut conn, &raw_frame, &metrics)
            .await?;

    let protocol_version = if is_v3 { "v3" } else { "v2" };
    tracing::info!(
        target: "rttx_profile",
        client_id = %client_short,
        protocol_version,
        "client session started",
    );

    let (reader, writer) = conn.into_split();
    let write_short = client_short.clone();
    let writer_metrics = Arc::clone(&metrics);
    let writer_task = tokio::spawn(client_writer(writer, rx, resp_rx, write_short, writer_metrics));

    let (result, handshake_completed) = if is_v3 {
        tracing::info!("Client {client_short} connected (v3)");
        v3_client_reader(server.clone(), client_id, &client_short, reader, resp_tx, metrics.clone())
            .await
    } else {
        // The v2 protocol is no longer supported. A first frame that is not a
        // valid v3 ClientHello is rejected and the connection is dropped.
        tracing::warn!("Client {client_short} rejected — v2 protocol no longer supported");
        (Ok(()), false)
    };

    // Cleanup: remove sender and detach from all workspaces.
    {
        let mut s = crate::instrument::lock_server(&server, &metrics).await;
        s.client_senders.remove(&client_id);
        s.client_resp_senders.remove(&client_id);
        s.client_protocols.remove(&client_id);
        s.client_pane_sizes.remove(&client_id);
        if handshake_completed {
            let rt_locks: Vec<WorkspaceLock> = s.workspaces.values().cloned().collect();
            drop(s);
            for rt_lock in rt_locks {
                let mut rt = crate::instrument::lock_workspace(&rt_lock, &metrics).await;
                let _ = rt.detach_client(client_id, DetachReason::Disconnect);
            }
        }
    }

    writer_task.abort();

    // Record disconnect reason.
    let disconnect_reason = if result.is_err() { "error" } else { "eof" };
    tracing::info!(
        target: "rttx_profile",
        client_id = %client_short,
        reason = disconnect_reason,
        "client disconnected",
    );
    metrics.connected_clients.fetch_sub(1, Ordering::Relaxed);
    metrics.client_disconnects.fetch_add(1, Ordering::Relaxed);

    if let Err(ref e) = result {
        tracing::error!("Client {client_short} error: {e}");
    }

    result
}

/// Parse and validate a length-prefix-stripped frame as a v3 `ClientHello`.
///
/// Returns `Some` only for a structurally valid v3 hello (16-byte client id
/// and a non-zero `max_protocol_version`). Legacy v2 `ClientMessage` frames —
/// which are no longer supported — fail this check and yield `None`, so the
/// caller rejects the connection.
fn parse_v3_client_hello(payload: &[u8]) -> Option<v3::ClientHello> {
    use prost::Message;
    let client_hello = v3::ClientHello::decode(payload).ok()?;
    if client_hello.client_id.len() != 16 || client_hello.max_protocol_version == 0 {
        return None;
    }
    Some(client_hello)
}

/// Attempt v3 handshake. Returns `true` if the client speaks v3.
///
/// On success, sends `ServerHello` and registers the client protocol.
/// On failure (the frame is not a valid v3 `ClientHello`), returns `false`
/// so the caller can reject the connection — non-v3 clients are not
/// supported.
async fn try_v3_handshake<S>(
    server: &Arc<Mutex<Server>>,
    client_id: Uuid,
    client_short: &str,
    conn: &mut ClientConnection<S>,
    raw_frame: &[u8],
    metrics: &Arc<crate::metrics::DaemonMetrics>,
) -> anyhow::Result<bool>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    // Skip the 4-byte length prefix.
    let payload = &raw_frame[4..];
    let Some(client_hello) = parse_v3_client_hello(payload) else {
        return Ok(false);
    };

    let server_version = env!("CARGO_PKG_VERSION");
    let server_id = crate::instrument::lock_server(server, metrics).await.server_id;

    match rttx_proto::v3_handshake::negotiate_version(
        client_hello.min_protocol_version,
        client_hello.max_protocol_version,
        rttx_proto::v3_handshake::V3_PROTOCOL_VERSION,
        rttx_proto::v3_handshake::V3_PROTOCOL_VERSION,
    ) {
        Ok(negotiated_version) => {
            let server_caps: Vec<i32> = SERVER_CAPABILITIES.iter().map(|c| *c as i32).collect();
            let effective_caps = rttx_proto::v3_handshake::effective_capabilities(
                &client_hello.capabilities,
                &server_caps,
            );

            // Validate core capabilities from client.
            if let Err(missing) =
                rttx_proto::v3_handshake::validate_server_capabilities(&effective_caps)
            {
                let err = rttx_proto::v3_handshake::missing_capabilities_error(&missing);
                conn.send_v3_error(&err).await?;
                return Err(anyhow::anyhow!("v3 client {client_short} missing core capabilities"));
            }

            let hello = rttx_proto::v3_handshake::build_server_hello(
                server_id,
                server_version,
                negotiated_version,
                SERVER_CAPABILITIES,
            );
            conn.send_v3_server_hello(&hello).await?;

            {
                let mut s = crate::instrument::lock_server(server, metrics).await;
                s.set_client_protocol(client_id, ClientProtocol::V3 { effective_caps });
            }

            Ok(true)
        }
        Err(err) => {
            conn.send_v3_error(&err).await?;
            Err(anyhow::anyhow!(
                "v3 version negotiation failed for {client_short}: {}",
                err.message
            ))
        }
    }
}

/// Read v3 client envelopes and dispatch responses via `resp_tx`.
async fn v3_client_reader(
    server: Arc<Mutex<Server>>,
    client_id: Uuid,
    client_short: &str,
    mut reader: ClientConnectionReader,
    resp_tx: mpsc::Sender<ClientMsg>,
    metrics: Arc<crate::metrics::DaemonMetrics>,
) -> (anyhow::Result<()>, bool) {
    loop {
        let envelope = match reader.read_v3_envelope().await {
            Ok(Some(env)) => env,
            Ok(None) => {
                tracing::info!("Client {client_short} disconnected");
                return (Ok(()), true);
            }
            Err(e) => return (Err(e.into()), true),
        };

        // Check for Shutdown (fire-and-forget).
        if matches!(envelope.command, Some(v3::client_envelope::Command::Shutdown(_))) {
            tracing::warn!("Shutdown requested by client {client_short} (v3)");
            crate::instrument::lock_server(&server, &metrics).await.request_shutdown();
            return (Ok(()), true);
        }

        // Fast-path: respond to Ping without acquiring the server mutex.
        if let Some(v3::client_envelope::Command::Ping(ref ping)) = envelope.command {
            metrics.messages_dispatched.fetch_add(1, Ordering::Relaxed);
            let response = rttx_proto::v3_envelope::build_response_envelope(
                envelope.request_id,
                v3::server_envelope::Payload::Pong(v3::Pong { nonce: ping.nonce }),
            );
            if resp_tx.send(response).await.is_err() {
                return (Ok(()), true);
            }
            continue;
        }

        let _msg_type = v3_msg_type(&envelope);
        metrics.messages_dispatched.fetch_add(1, Ordering::Relaxed);
        let dispatch_start = std::time::Instant::now();

        // Look up effective capabilities for this client.
        let effective_caps = {
            let s = crate::instrument::lock_server(&server, &metrics).await;
            match s.client_protocol(client_id) {
                Some(ClientProtocol::V3 { effective_caps }) => effective_caps.clone(),
                _ => Vec::new(),
            }
        };

        if let Some(response) =
            Server::handle_v3_message(&server, client_id, &effective_caps, envelope, &metrics).await
            && resp_tx.send(response).await.is_err()
        {
            metrics.dispatch_latency_us.record(dispatch_start.elapsed().as_micros() as u64);
            return (Ok(()), true);
        }
        metrics.dispatch_latency_us.record(dispatch_start.elapsed().as_micros() as u64);
    }
}

/// Drain both the push channel and the response channel, writing each
/// message to the client socket. Exits when both senders are dropped or
/// a write error occurs.
async fn client_writer(
    mut writer: ClientConnectionWriter,
    mut push_rx: mpsc::Receiver<ClientMsg>,
    mut resp_rx: mpsc::Receiver<ClientMsg>,
    client_short: String,
    metrics: Arc<crate::metrics::DaemonMetrics>,
) {
    use prost::Message;
    use tracing::Instrument;

    loop {
        let msg = tokio::select! {
            biased;
            msg = resp_rx.recv() => match msg {
                Some(m) => m,
                None => match push_rx.recv().await {
                    Some(m) => m,
                    None => break,
                },
            },
            msg = push_rx.recv() => match msg {
                Some(m) => m,
                None => match resp_rx.recv().await {
                    Some(m) => m,
                    None => break,
                },
            },
        };

        let bytes_len = 4 + msg.encoded_len();

        let write_span = tracing::info_span!(
            target: "rttx_profile",
            "client.write",
            span_kind = "client_write",
            client_id = %client_short,
            bytes_written = bytes_len,
        );

        let result = async { writer.send_v3_envelope(&msg).await }.instrument(write_span).await;

        match result {
            Ok(()) => {
                metrics.bytes_written_to_clients.fetch_add(bytes_len as u64, Ordering::Relaxed);
            }
            Err(e) => {
                tracing::error!("Client {client_short} write error: {e}");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests;
