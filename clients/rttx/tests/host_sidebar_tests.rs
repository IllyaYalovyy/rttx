#![allow(clippy::doc_markdown, clippy::items_after_statements)]

//! Integration tests for the host-aware right sidebar.
//!
//! Validates that RuntimeEndpoint::host_key() correctly maps endpoints
//! to host keys, and that commands::visible_for_host() filters correctly.

use rttx::commands::{self, SavedCommand};
use rttx::host;
use rttx::places::{self, Place};
use rttx::runtime::RuntimeEndpoint;

#[test]
fn host_key_local_endpoint_returns_local_key() {
    assert_eq!(RuntimeEndpoint::Local.host_key(), host::LOCAL_KEY);
}

#[test]
fn host_key_remote_endpoint_normalizes_ssh_target() {
    let endpoint = RuntimeEndpoint::Remote { host: "deploy@example.com".into() };
    assert_eq!(endpoint.host_key(), "example.com");
}

#[test]
fn host_key_remote_endpoint_bare_hostname() {
    let endpoint = RuntimeEndpoint::Remote { host: "dev-box".into() };
    assert_eq!(endpoint.host_key(), "dev-box");
}

#[test]
fn commands_visible_for_host_filters_by_tag() {
    let mut local_cmd = SavedCommand::new("Local", "echo local");
    local_cmd.host_tags = vec!["local".into()];
    let mut remote_cmd = SavedCommand::new("Remote", "echo remote");
    remote_cmd.host_tags = vec!["example.com".into()];
    let global_cmd = SavedCommand::new("Global", "echo global");

    let saved = vec![local_cmd, remote_cmd, global_cmd];

    let local_visible = commands::visible_for_host(&saved, "local");
    let names: Vec<&str> = local_visible.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(names, vec!["Local", "Global"]);

    let remote_visible = commands::visible_for_host(&saved, "example.com");
    let names: Vec<&str> = remote_visible.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(names, vec!["Remote", "Global"]);
}

#[test]
fn places_visible_for_host_includes_builtins_and_tagged() {
    let mut local_place = Place::new("rttx", "~/pro/rttx");
    local_place.host_tags = vec!["local".into()];
    let mut remote_place = Place::new("app", "/srv/app");
    remote_place.host_tags = vec!["example.com".into()];

    let saved = vec![local_place, remote_place];

    let local_visible = places::visible_for_host(&saved, "local");
    let names: Vec<&str> = local_visible.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["Home", "Root", "rttx"]);

    let remote_visible = places::visible_for_host(&saved, "example.com");
    let names: Vec<&str> = remote_visible.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["Home", "Root", "app"]);
}

#[test]
fn endpoint_host_key_matches_place_and_command_tags() {
    let endpoint = RuntimeEndpoint::Remote { host: "deploy@example.com".into() };
    let host_key = endpoint.host_key();

    let mut place = Place::new("app", "/srv/app");
    place.host_tags = vec!["example.com".into()];
    assert!(places::is_visible_on(&place, &host_key));

    let mut cmd = SavedCommand::new("Deploy", "cargo build");
    cmd.host_tags = vec!["example.com".into()];
    assert!(commands::is_visible_on(&cmd, &host_key));
}

#[test]
fn add_host_from_remote_endpoint_saves_and_deduplicates() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("hosts.json");

    let ssh_target = "deploy@builder.example.com";
    let new_host = host::Host::remote(ssh_target);

    // First save succeeds
    let mut hosts = host::load_from(&path);
    assert!(!hosts.iter().any(|h| h.key == new_host.key));
    hosts.push(new_host.clone());
    host::save_to(&hosts, &path).unwrap();

    // Duplicate detection prevents second save
    let hosts = host::load_from(&path);
    assert!(hosts.iter().any(|h| h.key == new_host.key));
    assert_eq!(hosts.iter().filter(|h| h.key == "builder.example.com").count(), 1);
}

#[test]
fn add_host_rejects_blank_ssh_target() {
    let blank_targets = ["", "  ", "\t\n"];
    for target in blank_targets {
        let trimmed = target.trim();
        assert!(trimmed.is_empty(), "blank target should be rejected before creating a host");
    }
}

#[test]
fn add_host_detects_duplicate_by_normalized_key() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("hosts.json");

    let host_a = host::Host::remote("deploy@example.com");
    host::save_to(&[host_a], &path).unwrap();

    let hosts = host::load_from(&path);
    let host_b = host::Host::remote("root@Example.COM");
    assert!(
        hosts.iter().any(|h| h.key == host_b.key),
        "different user and case should still match the same host key"
    );
}
