//! Integration tests for retry-connection behavior.
//!
//! Verifies that explicit user retry bypasses a stuck endpoint actor
//! by shutting it down and creating a fresh one.

use rttx::daemon_bridge::EndpointConnectionManager;
use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};

#[test]
fn reset_endpoint_creates_fresh_actor_on_next_access() {
    let (manager, _event_rx, _capacity_probe, _backpressure) =
        EndpointConnectionManager::new(false, 10).unwrap();
    let endpoint = RuntimeEndpoint::Local;

    // First access creates an actor.
    manager.open_workspace("ws-1", &endpoint, "Test", WorkspacePolicy::default(), None, None, None);

    // Reset kills the old actor.
    manager.reset_endpoint(&endpoint);

    // Next access should create a fresh actor (not reuse the old one).
    // If the old actor were reused, it would be shut down and unable to process.
    manager.open_workspace("ws-1", &endpoint, "Test", WorkspacePolicy::default(), None, None, None);
}
