//! Verifiable coverage for the user-visible connection status (#320).
//!
//! The remote-workspace lifecycle surfaces a status to the user (the sidebar
//! row / pane header: "Connecting", "Connected", retry countdown, …). This
//! exercises that presentation at the pure-state layer — no AT-SPI / a11y bus,
//! no display. The sidebar status icon and the status lifecycle transitions are
//! covered by unit tests in `runtime.rs` (`connection_icon_*`,
//! `advance_connection_status_*`).

use rttx::runtime::{ConnectionStatus, present_connection_status};

#[test]
fn connection_status_presentation_shows_expected_label_and_input_state() {
    // While connecting, the header shows "Connecting" and input is disabled.
    let connecting = present_connection_status(&ConnectionStatus::Connecting);
    assert_eq!(connecting.header_label, "Connecting");
    assert!(!connecting.input_enabled, "input must be disabled while connecting");

    // Once connected, the header shows "Connected" and input is enabled.
    let connected = present_connection_status(&ConnectionStatus::Connected);
    assert_eq!(connected.header_label, "Connected");
    assert!(connected.input_enabled, "input must be enabled once connected");

    // Reconnecting shows a retry countdown and keeps input disabled.
    let reconnecting = present_connection_status(&ConnectionStatus::Reconnecting {
        attempt: 1,
        retry_in_secs: 3,
    });
    assert_eq!(reconnecting.header_label, "Retry 3s");
    assert!(!reconnecting.input_enabled);

    // Disconnected keeps input disabled.
    let disconnected = present_connection_status(&ConnectionStatus::Disconnected);
    assert_eq!(disconnected.header_label, "Disconnected");
    assert!(!disconnected.input_enabled);
}
