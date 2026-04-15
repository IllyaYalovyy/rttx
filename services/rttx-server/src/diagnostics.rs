//! Runtime memory diagnostics.
//!
//! Collects per-session and per-pane memory metrics from the server state
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

/// Per-session memory metrics.
#[derive(Debug, Clone)]
pub struct SessionDiagnostics {
    pub id: String,
    pub name: String,
    pub active_pane_count: usize,
    pub exited_pane_count: usize,
    pub command_history_len: usize,
    pub attached_client_count: usize,
    pub panes: Vec<PaneDiagnostics>,
}

/// Server-wide diagnostics report.
#[derive(Debug, Clone)]
pub struct DiagnosticsReport {
    pub session_count: usize,
    pub total_pane_count: usize,
    pub total_active_panes: usize,
    pub total_exited_panes: usize,
    pub client_count: usize,
    pub pty_writer_count: usize,
    pub total_raw_bytes: usize,
    pub total_pending_flush: usize,
    pub total_command_history: usize,
    pub sessions: Vec<SessionDiagnostics>,
}

impl fmt::Display for DiagnosticsReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Sessions: {}", self.session_count)?;
        writeln!(
            f,
            "Panes: {} ({} active, {} exited)",
            self.total_pane_count, self.total_active_panes, self.total_exited_panes
        )?;
        writeln!(f, "Connected clients: {}", self.client_count)?;
        writeln!(f, "PTY writers: {}", self.pty_writer_count)?;
        writeln!(f, "Total raw_bytes: {} bytes", self.total_raw_bytes)?;
        writeln!(f, "Total pending_flush: {} bytes", self.total_pending_flush)?;
        writeln!(f, "Total command_history entries: {}", self.total_command_history)?;

        if !self.sessions.is_empty() {
            writeln!(f)?;
            for session in &self.sessions {
                writeln!(
                    f,
                    "  Session \"{}\" ({}):",
                    session.name,
                    &session.id[..8.min(session.id.len())]
                )?;
                writeln!(
                    f,
                    "    Panes: {} active, {} exited",
                    session.active_pane_count, session.exited_pane_count
                )?;
                writeln!(f, "    Command history: {} entries", session.command_history_len)?;
                writeln!(f, "    Attached clients: {}", session.attached_client_count)?;
                for pane in &session.panes {
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
    #[must_use]
    pub fn diagnostics(&self) -> DiagnosticsReport {
        let mut sessions = Vec::with_capacity(self.sessions.len());
        let mut total_raw_bytes = 0usize;
        let mut total_pending_flush = 0usize;
        let mut total_command_history = 0usize;
        let mut total_active_panes = 0usize;
        let mut total_exited_panes = 0usize;

        for session in self.sessions.values() {
            let mut panes = Vec::with_capacity(session.panes.len());
            let mut active = 0usize;
            let mut exited = 0usize;

            for pane in session.panes.values() {
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
            total_command_history += session.command_history.len();

            sessions.push(SessionDiagnostics {
                id: session.id.to_string(),
                name: session.name.clone(),
                active_pane_count: active,
                exited_pane_count: exited,
                command_history_len: session.command_history.len(),
                attached_client_count: session.attached_client_count(),
                panes,
            });
        }

        DiagnosticsReport {
            session_count: self.sessions.len(),
            total_pane_count: total_active_panes + total_exited_panes,
            total_active_panes,
            total_exited_panes,
            client_count: self.client_sender_count(),
            pty_writer_count: self.pty_writer_count(),
            total_raw_bytes,
            total_pending_flush,
            total_command_history,
            sessions,
        }
    }

    /// Log key memory metrics at debug level.
    pub fn log_diagnostics(&self) {
        let report = self.diagnostics();
        tracing::debug!(
            sessions = report.session_count,
            panes = report.total_pane_count,
            active_panes = report.total_active_panes,
            exited_panes = report.total_exited_panes,
            clients = report.client_count,
            pty_writers = report.pty_writer_count,
            raw_bytes = report.total_raw_bytes,
            pending_flush = report.total_pending_flush,
            command_history = report.total_command_history,
            "memory diagnostics"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::Pane;
    use crate::session::{RuntimePolicy, Session};
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
        }
        Server::new(Box::new(TestOs))
    }

    #[test]
    fn empty_server_diagnostics() {
        let server = test_server();
        let report = server.diagnostics();
        assert_eq!(report.session_count, 0);
        assert_eq!(report.total_pane_count, 0);
        assert_eq!(report.total_active_panes, 0);
        assert_eq!(report.total_exited_panes, 0);
        assert_eq!(report.client_count, 0);
        assert_eq!(report.pty_writer_count, 0);
        assert_eq!(report.total_raw_bytes, 0);
        assert_eq!(report.total_pending_flush, 0);
        assert_eq!(report.total_command_history, 0);
    }

    #[test]
    fn diagnostics_counts_sessions_and_panes() {
        let mut server = test_server();
        let mut session = Session::new("test".into());
        session.policy = RuntimePolicy::Persistent;

        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"hello world");
        session.add_pane(pane);

        let mut exited_pane = Pane::new(Uuid::new_v4(), 80, 24);
        exited_pane.set_exited(0);
        session.add_pane(exited_pane);

        server.sessions.insert(session.id, session);

        let report = server.diagnostics();
        assert_eq!(report.session_count, 1);
        assert_eq!(report.total_pane_count, 2);
        assert_eq!(report.total_active_panes, 1);
        assert_eq!(report.total_exited_panes, 1);
        assert_eq!(report.total_raw_bytes, 11); // "hello world"
    }

    #[test]
    fn diagnostics_counts_command_history() {
        let mut server = test_server();
        let mut session = Session::new("test".into());
        session.command_history.push(crate::pane::HistoryEntry {
            command: "ls".into(),
            cwd: "/tmp".into(),
            timestamp: std::time::SystemTime::now(),
            pane_id: Uuid::new_v4(),
        });
        server.sessions.insert(session.id, session);

        let report = server.diagnostics();
        assert_eq!(report.total_command_history, 1);
    }

    #[test]
    fn diagnostics_display_format() {
        let server = test_server();
        let report = server.diagnostics();
        let output = report.to_string();
        assert!(output.contains("Sessions: 0"));
        assert!(output.contains("Connected clients: 0"));
    }

    #[test]
    fn diagnostics_after_session_removal_returns_to_zero() {
        let mut server = test_server();
        let mut session = Session::new("temp".into());
        let sid = session.id;
        session.add_pane(Pane::new(Uuid::new_v4(), 80, 24));
        server.sessions.insert(sid, session);

        assert_eq!(server.diagnostics().session_count, 1);

        server.sessions.remove(&sid);
        let report = server.diagnostics();
        assert_eq!(report.session_count, 0);
        assert_eq!(report.total_pane_count, 0);
    }
}
