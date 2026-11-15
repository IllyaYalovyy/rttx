//! Integration test for `rttx-server config` subcommand.

use std::process::Command;

#[test]
fn config_prints_expected_fields() {
    let output = Command::new(env!("CARGO_BIN_EXE_rttx-server"))
        .arg("config")
        .env("RTTX_DEV_MODE", "")
        .output()
        .expect("failed to run rttx-server config");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Mode: production"));
    assert!(stdout.contains("Socket:"));
    assert!(stdout.contains("State:"));
    assert!(stdout.contains("Scrollback:"));
    assert!(stdout.contains("Logs:"));
    assert!(stdout.contains("Protocol version:"));
}

#[test]
fn config_dev_mode_shows_development() {
    let output = Command::new(env!("CARGO_BIN_EXE_rttx-server"))
        .arg("config")
        .env("RTTX_DEV_MODE", "1")
        .output()
        .expect("failed to run rttx-server config");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Mode: development"));
    assert!(stdout.contains("rttx-server-devel"));
}
