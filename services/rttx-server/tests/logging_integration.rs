//! Integration tests for file logging infrastructure.

use rttx_server::logging::cleanup_old_logs;
use std::path::Path;

#[test]
fn cleanup_old_logs_keeps_correct_number_of_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();

    for day in 1..=7 {
        std::fs::write(dir.join(format!("rttx-server.log.2026-04-{day:02}")), "data").unwrap();
    }

    cleanup_old_logs(dir, "rttx-server.log", 3);

    let remaining: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(Result::ok).collect();

    // 3 + 1 = 4 files kept (keep_days + 1 because the current day's file counts).
    assert_eq!(remaining.len(), 4, "should keep 4 most recent log files");
}

#[test]
fn cleanup_old_logs_does_not_panic_on_missing_dir() {
    cleanup_old_logs(Path::new("/nonexistent/test/path"), "rttx-server.log", 3);
}
