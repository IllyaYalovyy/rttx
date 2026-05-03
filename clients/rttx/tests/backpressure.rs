//! Integration tests for backpressure between the GTK poller and daemon bridge.

use std::sync::atomic::Ordering;

use rttx::daemon_bridge::EndpointConnectionManager;

/// The backpressure flag returned by `EndpointConnectionManager::new` starts
/// cleared, and the capacity probe reports the full channel capacity.
#[test]
fn manager_backpressure_flag_starts_inactive() {
    let (_manager, _rx, capacity_probe, backpressure) =
        EndpointConnectionManager::new(false, 10).unwrap();

    assert!(!backpressure.load(Ordering::Acquire), "backpressure must start inactive");

    // An empty channel should have capacity well above any reasonable
    // low watermark threshold.
    assert!(capacity_probe.capacity() > 3000, "empty channel must report near-full capacity");
}

/// Setting the backpressure flag externally (as the GTK poller would) does
/// not panic or interfere with the manager's ability to accept commands.
#[test]
fn backpressure_flag_toggle_does_not_block_manager_commands() {
    let (manager, _rx, _capacity_probe, backpressure) =
        EndpointConnectionManager::new(false, 10).unwrap();

    // Activate backpressure.
    backpressure.store(true, Ordering::Release);
    assert!(backpressure.load(Ordering::Acquire));

    // The manager should still accept fire-and-forget commands while
    // backpressure is active — commands go through the command channel,
    // not the event channel.
    let endpoint = rttx::runtime::RuntimeEndpoint::Local;
    manager.open_workspace(
        "ws-bp",
        &endpoint,
        "Backpressure Test",
        rttx::runtime::WorkspacePolicy::default(),
        None,
        None,
        None,
    );

    // Release backpressure.
    backpressure.store(false, Ordering::Release);
    assert!(!backpressure.load(Ordering::Acquire));
}
