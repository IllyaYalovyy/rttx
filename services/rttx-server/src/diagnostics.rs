//! Runtime memory diagnostics.
//!
//! Collects per-runtime and per-pane memory metrics from the server state
//! for the `rttx-server diagnostics` CLI command and periodic debug logging.

use crate::server::Server;
use std::fmt;

/// Per-pane memory metrics.
#[derive(Debug, Clone)]
pub struct PaneDiagnostics {
    pub id: String,
    pub raw_bytes_len: usize,
    pub pending_flush_len: usize,
    pub is_exited: bool,
}

/// Per-runtime memory metrics.
#[derive(Debug, Clone)]
pub struct RuntimeDiagnostics {
    pub id: String,
    pub name: String,
    pub active_pane_count: usize,
    pub exited_pane_count: usize,
    pub attached_client_count: usize,
    pub panes: Vec<PaneDiagnostics>,
}

/// Server-wide diagnostics report.
#[derive(Debug, Clone)]
pub struct DiagnosticsReport {
    pub runtime_count: usize,
    pub total_pane_count: usize,
    pub total_active_panes: usize,
    pub total_exited_panes: usize,
    pub client_count: usize,
    pub pty_writer_count: usize,
    pub total_raw_bytes: usize,
    pub total_pending_flush: usize,
    pub runtimes: Vec<RuntimeDiagnostics>,
}

impl fmt::Display for DiagnosticsReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Runtimes: {}", self.runtime_count)?;
        writeln!(
            f,
            "Panes: {} ({} active, {} exited)",
            self.total_pane_count, self.total_active_panes, self.total_exited_panes
        )?;
        writeln!(f, "Connected clients: {}", self.client_count)?;
        writeln!(f, "PTY writers: {}", self.pty_writer_count)?;
        writeln!(f, "Total raw_bytes: {} bytes", self.total_raw_bytes)?;
        writeln!(f, "Total pending_flush: {} bytes", self.total_pending_flush)?;

        if !self.runtimes.is_empty() {
            writeln!(f)?;
            for rt in &self.runtimes {
                writeln!(f, "  Runtime \"{}\" ({}):", rt.name, &rt.id[..8.min(rt.id.len())])?;
                writeln!(
                    f,
                    "    Panes: {} active, {} exited",
                    rt.active_pane_count, rt.exited_pane_count
                )?;
                writeln!(f, "    Attached clients: {}", rt.attached_client_count)?;
                for pane in &rt.panes {
                    let status = if pane.is_exited { "exited" } else { "active" };
                    writeln!(
                        f,
                        "    Pane {} ({}): raw_bytes={}, pending_flush={}",
                        &pane.id[..8.min(pane.id.len())],
                        status,
                        pane.raw_bytes_len,
                        pane.pending_flush_len,
                    )?;
                }
            }
        }

        Ok(())
    }
}

impl Server {
    /// Collect a diagnostics report from the current server state.
    ///
    /// Uses `try_lock` on per-runtime locks to avoid blocking.  Runtimes
    /// whose lock cannot be acquired are silently skipped.
    #[must_use]
    pub fn diagnostics(&self) -> DiagnosticsReport {
        let mut runtimes = Vec::with_capacity(self.runtimes.len());
        let mut total_raw_bytes = 0usize;
        let mut total_pending_flush = 0usize;
        let mut total_active_panes = 0usize;
        let mut total_exited_panes = 0usize;

        for rt_lock in self.runtimes.values() {
            let Ok(rt) = rt_lock.try_lock() else {
                continue;
            };
            let mut panes = Vec::with_capacity(rt.panes.len());
            let mut active = 0usize;
            let mut exited = 0usize;

            for pane in rt.panes.values() {
                let raw_bytes_len = pane.screen.raw_bytes().len();
                let pending_flush_len = pane.pending_flush_len();
                total_raw_bytes += raw_bytes_len;
                total_pending_flush += pending_flush_len;

                if pane.is_exited() {
                    exited += 1;
                } else {
                    active += 1;
                }

                panes.push(PaneDiagnostics {
                    id: pane.id.to_string(),
                    raw_bytes_len,
                    pending_flush_len,
                    is_exited: pane.is_exited(),
                });
            }

            total_active_panes += active;
            total_exited_panes += exited;

            runtimes.push(RuntimeDiagnostics {
                id: rt.id.to_string(),
                name: rt.name.clone(),
                active_pane_count: active,
                exited_pane_count: exited,
                attached_client_count: rt.attached_client_count(),
                panes,
            });
        }

        DiagnosticsReport {
            runtime_count: self.runtimes.len(),
            total_pane_count: total_active_panes + total_exited_panes,
            total_active_panes,
            total_exited_panes,
            client_count: self.client_sender_count(),
            pty_writer_count: self.pty_writer_count(),
            total_raw_bytes,
            total_pending_flush,
            runtimes,
        }
    }

    /// Log key memory metrics at debug level.
    pub fn log_diagnostics(&self) {
        let report = self.diagnostics();
        tracing::debug!(
            runtimes = report.runtime_count,
            panes = report.total_pane_count,
            active_panes = report.total_active_panes,
            exited_panes = report.total_exited_panes,
            clients = report.client_count,
            pty_writers = report.pty_writer_count,
            raw_bytes = report.total_raw_bytes,
            pending_flush = report.total_pending_flush,
            "memory diagnostics"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::Pane;
    use crate::runtime::{Runtime, RuntimePolicy};
    use std::sync::Arc;
    use uuid::Uuid;

    fn test_server() -> Server {
        use crate::os::OsInterface;
        use std::path::PathBuf;

        #[derive(Debug)]
        struct TestOs;
        impl OsInterface for TestOs {
            fn runtime_dir(&self) -> PathBuf {
                PathBuf::from("/tmp/test-runtime")
            }
            fn cache_dir(&self) -> PathBuf {
                PathBuf::from("/tmp/test-cache")
            }
            fn state_dir(&self) -> PathBuf {
                PathBuf::from("/tmp/test-state/rttx/daemon")
            }
        }
        let dir = tempfile::TempDir::new().unwrap();
        let ring = std::sync::Arc::new(crate::flight::RingWriter::open(dir.path()).unwrap());
        std::mem::forget(dir);
        Server::new(
            Box::new(TestOs),
            std::sync::Arc::new(crate::metrics::DaemonMetrics::new()),
            ring,
        )
    }

    #[test]
    fn empty_server_diagnostics() {
        let server = test_server();
        let report = server.diagnostics();
        assert_eq!(report.runtime_count, 0);
        assert_eq!(report.total_pane_count, 0);
        assert_eq!(report.total_active_panes, 0);
        assert_eq!(report.total_exited_panes, 0);
        assert_eq!(report.client_count, 0);
        assert_eq!(report.pty_writer_count, 0);
        assert_eq!(report.total_raw_bytes, 0);
        assert_eq!(report.total_pending_flush, 0);
    }

    #[test]
    fn diagnostics_counts_runtimes_and_panes() {
        let mut server = test_server();
        let mut rt = Runtime::new("test".into());
        rt.policy = RuntimePolicy::Persistent;

        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"hello world");
        rt.add_pane(pane);

        let mut exited_pane = Pane::new(Uuid::new_v4(), 80, 24);
        exited_pane.set_exited(0);
        rt.add_pane(exited_pane);

        server.runtimes.insert(rt.id, Arc::new(tokio::sync::Mutex::new(rt)));

        let report = server.diagnostics();
        assert_eq!(report.runtime_count, 1);
        assert_eq!(report.total_pane_count, 2);
        assert_eq!(report.total_active_panes, 1);
        assert_eq!(report.total_exited_panes, 1);
        assert_eq!(report.total_raw_bytes, 11); // "hello world"
    }

    #[test]
    fn diagnostics_display_format() {
        let server = test_server();
        let report = server.diagnostics();
        let output = report.to_string();
        assert!(output.contains("Runtimes: 0"));
        assert!(output.contains("Connected clients: 0"));
    }

    #[test]
    fn diagnostics_after_runtime_removal_returns_to_zero() {
        let mut server = test_server();
        let mut rt = Runtime::new("temp".into());
        let sid = rt.id;
        rt.add_pane(Pane::new(Uuid::new_v4(), 80, 24));
        server.runtimes.insert(sid, Arc::new(tokio::sync::Mutex::new(rt)));

        assert_eq!(server.diagnostics().runtime_count, 1);

        server.runtimes.remove(&sid);
        let report = server.diagnostics();
        assert_eq!(report.runtime_count, 0);
        assert_eq!(report.total_pane_count, 0);
    }
}
