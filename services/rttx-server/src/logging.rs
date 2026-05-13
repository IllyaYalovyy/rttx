//! File logging with daily rotation and automatic cleanup.

use std::path::Path;
use std::sync::Arc;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::flight::RingWriter;
use crate::metrics::DaemonMetrics;
use crate::profiling::ProfilingLayer;

/// Initialize file-based logging with daily rotation and profiling layer.
///
/// Composes the file-logging layer with the `ProfilingLayer` via the
/// tracing subscriber registry. The profiling layer records span events
/// to the ring buffer and updates `DaemonMetrics` histograms.
pub fn init_file_logging(log_dir: &Path, prefix: &str, dev_mode: bool) {
    let default_level = if dev_mode { "debug" } else { "info" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    let _ = std::fs::create_dir_all(log_dir);
    let file_appender = tracing_appender::rolling::daily(log_dir, format!("{prefix}.log"));
    cleanup_old_logs(log_dir, &format!("{prefix}.log"), 3);

    let fmt_layer = tracing_subscriber::fmt::layer().with_writer(file_appender).with_ansi(false);

    tracing_subscriber::registry().with(filter).with(fmt_layer).init();
}

/// Initialize file-based logging composed with the profiling layer.
///
/// The profiling layer writes span events to the ring buffer and updates
/// latency histograms in `DaemonMetrics`.
pub fn init_logging_with_profiling(
    log_dir: &Path,
    prefix: &str,
    dev_mode: bool,
    metrics: Arc<DaemonMetrics>,
    ring: Arc<RingWriter>,
) {
    let default_level = if dev_mode { "debug" } else { "info" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    let _ = std::fs::create_dir_all(log_dir);
    let file_appender = tracing_appender::rolling::daily(log_dir, format!("{prefix}.log"));
    cleanup_old_logs(log_dir, &format!("{prefix}.log"), 3);

    let fmt_layer = tracing_subscriber::fmt::layer().with_writer(file_appender).with_ansi(false);

    let profiling_layer = ProfilingLayer::new(metrics, ring);

    tracing_subscriber::registry().with(filter).with(fmt_layer).with(profiling_layer).init();
}

/// Remove rotated log files older than `keep_days`.
pub fn cleanup_old_logs(dir: &Path, prefix: &str, keep_days: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut log_files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with(prefix)
                && e.file_type().is_ok_and(|t| t.is_file())
        })
        .collect();
    log_files.sort_by_key(|e| std::cmp::Reverse(e.file_name()));
    for old in log_files.into_iter().skip(keep_days + 1) {
        let _ = std::fs::remove_file(old.path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_removes_oldest_files_beyond_keep_days() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();

        // Create 5 fake rotated log files.
        for day in 1..=5 {
            std::fs::write(dir.join(format!("rttx-server.log.2026-04-0{day}")), "log").unwrap();
        }

        cleanup_old_logs(dir, "rttx-server.log", 3);

        let remaining: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        // keep_days + 1 = 4 files (3 rotated + 1 current-ish).
        assert_eq!(remaining.len(), 4, "should keep 4 files, got: {remaining:?}");
        assert!(!remaining.contains(&"rttx-server.log.2026-04-01".to_string()));
    }

    #[test]
    fn cleanup_is_noop_when_fewer_files_than_limit() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();

        std::fs::write(dir.join("rttx-server.log.2026-04-05"), "log").unwrap();

        cleanup_old_logs(dir, "rttx-server.log", 3);

        let count = std::fs::read_dir(dir).unwrap().count();
        assert_eq!(count, 1);
    }

    #[test]
    fn cleanup_ignores_unrelated_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();

        for day in 1..=5 {
            std::fs::write(dir.join(format!("rttx-server.log.2026-04-0{day}")), "log").unwrap();
        }
        std::fs::write(dir.join("state.json"), "{}").unwrap();

        cleanup_old_logs(dir, "rttx-server.log", 2);

        let remaining: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        assert!(remaining.contains(&"state.json".to_string()));
        // 2 + 1 log files + state.json = 4
        assert_eq!(remaining.len(), 4);
    }

    #[test]
    fn cleanup_handles_missing_directory() {
        cleanup_old_logs(Path::new("/nonexistent/path"), "rttx.log", 3);
        // Should not panic.
    }
}
