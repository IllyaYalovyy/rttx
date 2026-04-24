use serde::{Deserialize, Serialize};

/// Reserved host key for the local machine.
pub const LOCAL_KEY: &str = "local";

/// Whether a host represents the local machine or a remote endpoint.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HostKind {
    #[default]
    Local,
    Remote,
}

/// Canonical identity for a local or remote endpoint.
///
/// All session, place, and command matching uses `key` — not display names.
/// A workspace for an unsaved host still has a stable key and the UI still works.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Host {
    pub key: String,
    pub name: String,
    pub kind: HostKind,
    #[serde(default)]
    pub ssh_target: Option<String>,
}

impl Host {
    /// The built-in local host. Not persisted — always available.
    #[must_use]
    pub fn local() -> Self {
        Self {
            key: LOCAL_KEY.into(),
            name: "Local".into(),
            kind: HostKind::Local,
            ssh_target: None,
        }
    }

    /// Create a remote host from an SSH target string.
    ///
    /// The key is derived by normalizing the SSH target. The display name
    /// defaults to the hostname portion of the target.
    #[must_use]
    pub fn remote(ssh_target: &str) -> Self {
        let key = normalize_ssh_key(ssh_target);
        let name = display_name_from_ssh(ssh_target);
        Self { key, name, kind: HostKind::Remote, ssh_target: Some(ssh_target.into()) }
    }

    #[must_use]
    pub const fn is_local(&self) -> bool {
        matches!(self.kind, HostKind::Local)
    }

    #[must_use]
    pub const fn is_remote(&self) -> bool {
        matches!(self.kind, HostKind::Remote)
    }
}

/// Derive a stable host key from an SSH target string.
///
/// Strips `user@` prefix and normalizes to lowercase so that
/// `deploy@example.com` and `root@example.com` resolve to the same host,
/// and `Example.COM` matches `example.com`.
#[must_use]
pub fn normalize_ssh_key(ssh_target: &str) -> String {
    let trimmed = ssh_target.trim();
    let host_part = strip_ssh_user(trimmed);
    host_part.to_ascii_lowercase()
}

/// Extract a display name from an SSH target.
///
/// Uses the short hostname (before the first dot) when available.
#[must_use]
pub fn display_name_from_ssh(ssh_target: &str) -> String {
    let trimmed = ssh_target.trim();
    let host_part = strip_ssh_user(trimmed);
    // Use short hostname for display
    host_part.split('.').next().unwrap_or(host_part).to_string()
}

/// Strip `user@` prefix from an SSH target, returning the host portion.
fn strip_ssh_user(target: &str) -> &str {
    // Handle `-p PORT user@host` style targets: find the last token
    // that contains `@` and strip the user part.
    let last_token = target.rsplit_once(' ').map_or(target, |(_, last)| last);
    last_token.split_once('@').map_or(last_token, |(_, h)| h)
}

/// Items affected by deleting a host record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionAffected {
    pub places: Vec<crate::places::Place>,
    pub commands: Vec<crate::commands::SavedCommand>,
}

impl DeletionAffected {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.places.is_empty() && self.commands.is_empty()
    }
}

/// Compute places and commands that reference `host_key` in their tags.
#[must_use]
pub fn deletion_affected(
    host_key: &str,
    places: &[crate::places::Place],
    commands: &[crate::commands::SavedCommand],
) -> DeletionAffected {
    DeletionAffected {
        places: places
            .iter()
            .filter(|p| p.host_tags.iter().any(|t| t == host_key))
            .cloned()
            .collect(),
        commands: commands
            .iter()
            .filter(|c| c.host_tags.iter().any(|t| t == host_key))
            .cloned()
            .collect(),
    }
}

/// Apply cleanup: remove the given place UUIDs and command UUIDs, then
/// remove the host from the saved hosts list.
///
/// Returns the updated `(hosts, places, commands)`.
#[must_use]
pub fn apply_deletion_cleanup(
    host_key: &str,
    hosts: &[Host],
    places: &[crate::places::Place],
    commands: &[crate::commands::SavedCommand],
    place_uuids_to_delete: &[String],
    command_uuids_to_delete: &[String],
) -> (Vec<Host>, Vec<crate::places::Place>, Vec<crate::commands::SavedCommand>) {
    let new_hosts: Vec<Host> = hosts.iter().filter(|h| h.key != host_key).cloned().collect();
    let new_places: Vec<crate::places::Place> =
        places.iter().filter(|p| !place_uuids_to_delete.contains(&p.uuid)).cloned().collect();
    let new_commands: Vec<crate::commands::SavedCommand> =
        commands.iter().filter(|c| !command_uuids_to_delete.contains(&c.uuid)).cloned().collect();
    (new_hosts, new_places, new_commands)
}

/// Resolve a host key to a `Host`, checking saved hosts first, then
/// returning the built-in local host or an ad-hoc remote host.
#[must_use]
pub fn resolve(key: &str, saved: &[Host]) -> Host {
    if let Some(host) = saved.iter().find(|h| h.key == key) {
        return host.clone();
    }
    if key == LOCAL_KEY {
        return Host::local();
    }
    // Ad-hoc remote: the key itself is the normalized SSH target
    Host {
        key: key.into(),
        name: key.split('.').next().unwrap_or(key).to_string(),
        kind: HostKind::Remote,
        ssh_target: Some(key.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    // ── Host construction ───────────────────────────────────────

    #[test]
    fn local_host_uses_reserved_key() {
        let host = Host::local();
        assert_eq!(host.key, LOCAL_KEY);
        assert_eq!(host.name, "Local");
        assert!(host.is_local());
        assert!(!host.is_remote());
        assert_eq!(host.ssh_target, None);
    }

    #[test]
    fn remote_host_derives_key_from_ssh_target() {
        let host = Host::remote("deploy@example.com");
        assert_eq!(host.key, "example.com");
        assert_eq!(host.name, "example");
        assert!(host.is_remote());
        assert!(!host.is_local());
        assert_eq!(host.ssh_target.as_deref(), Some("deploy@example.com"));
    }

    // ── Key normalization ───────────────────────────────────────

    #[test]
    fn normalize_strips_user_prefix() {
        assert_eq!(normalize_ssh_key("deploy@example.com"), "example.com");
        assert_eq!(normalize_ssh_key("root@example.com"), "example.com");
    }

    #[test]
    fn normalize_lowercases_hostname() {
        assert_eq!(normalize_ssh_key("Example.COM"), "example.com");
        assert_eq!(normalize_ssh_key("Deploy@Example.COM"), "example.com");
    }

    #[test]
    fn normalize_handles_bare_hostname() {
        assert_eq!(normalize_ssh_key("dev-box"), "dev-box");
    }

    #[test]
    fn normalize_handles_ssh_options() {
        assert_eq!(normalize_ssh_key("-p 2222 deploy@example.com"), "example.com");
    }

    #[test]
    fn normalize_trims_whitespace() {
        assert_eq!(normalize_ssh_key("  example.com  "), "example.com");
    }

    #[test]
    fn different_users_same_host_produce_same_key() {
        let key1 = normalize_ssh_key("deploy@example.com");
        let key2 = normalize_ssh_key("root@example.com");
        assert_eq!(key1, key2);
    }

    // ── Display name extraction ─────────────────────────────────

    #[test]
    fn display_name_uses_short_hostname() {
        assert_eq!(display_name_from_ssh("deploy@builder.example.com"), "builder");
    }

    #[test]
    fn display_name_for_bare_hostname() {
        assert_eq!(display_name_from_ssh("dev-box"), "dev-box");
    }

    // ── Serde ─────────────────────────────────────────────────────

    #[test]
    fn serde_roundtrip() {
        let hosts = vec![Host::remote("deploy@example.com")];
        let json = serde_json::to_string(&hosts).unwrap();
        let loaded: Vec<Host> = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, hosts);
    }

    #[test]
    fn local_host_not_in_remote_list() {
        let hosts = [Host::remote("example.com")];
        assert!(!hosts.iter().any(Host::is_local));
    }

    // ── Resolve ─────────────────────────────────────────────────

    #[test]
    fn resolve_finds_saved_host() {
        let mut host = Host::remote("deploy@example.com");
        host.name = "My Server".into();
        let saved = vec![host];

        let resolved = resolve("example.com", &saved);
        assert_eq!(resolved.name, "My Server");
        assert_eq!(resolved.ssh_target.as_deref(), Some("deploy@example.com"));
    }

    #[test]
    fn resolve_returns_local_for_local_key() {
        let resolved = resolve(LOCAL_KEY, &[]);
        assert!(resolved.is_local());
        assert_eq!(resolved.name, "Local");
    }

    #[test]
    fn resolve_creates_adhoc_remote_for_unknown_key() {
        let resolved = resolve("unknown.example.com", &[]);
        assert!(resolved.is_remote());
        assert_eq!(resolved.key, "unknown.example.com");
        assert_eq!(resolved.name, "unknown");
        assert_eq!(resolved.ssh_target.as_deref(), Some("unknown.example.com"));
    }

    // ── Serde backward compatibility ────────────────────────────

    #[test]
    fn deserialize_without_ssh_target_defaults_to_none() {
        let json = r#"{"key":"local","name":"Local","kind":"local"}"#;
        let host: Host = serde_json::from_str(json).unwrap();
        assert_eq!(host.ssh_target, None);
        assert!(host.is_local());
    }

    #[test]
    fn serialize_roundtrip_preserves_all_fields() {
        let host = Host::remote("deploy@example.com");
        let json = serde_json::to_string(&host).unwrap();
        let deserialized: Host = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, host);
    }

    // ── Deletion affected ───────────────────────────────────────

    #[test]
    fn deletion_affected_finds_tagged_places_and_commands() {
        let mut place = crate::places::Place::new("rttx", "~/pro/rttx");
        place.host_tags = vec!["example.com".into()];
        let global_place = crate::places::Place::new("tmp", "/tmp");

        let mut cmd = crate::commands::SavedCommand::new("Deploy", "cargo build");
        cmd.host_tags = vec!["example.com".into()];
        let global_cmd = crate::commands::SavedCommand::new("Global", "echo hi");

        let affected = deletion_affected(
            "example.com",
            &[place.clone(), global_place],
            &[cmd.clone(), global_cmd],
        );
        assert_eq!(affected.places, vec![place]);
        assert_eq!(affected.commands, vec![cmd]);
    }

    #[test]
    fn deletion_affected_empty_when_no_tags_match() {
        let place = crate::places::Place::new("rttx", "~/pro/rttx");
        let cmd = crate::commands::SavedCommand::new("Build", "cargo build");
        let affected = deletion_affected("example.com", &[place], &[cmd]);
        assert!(affected.is_empty());
    }

    #[test]
    fn deletion_affected_multi_tagged_item_included() {
        let mut place = crate::places::Place::new("shared", "/shared");
        place.host_tags = vec!["local".into(), "example.com".into()];
        let affected = deletion_affected("example.com", &[place], &[]);
        assert_eq!(affected.places.len(), 1);
    }

    // ── Apply deletion cleanup ──────────────────────────────────

    #[test]
    fn apply_cleanup_removes_selected_items_and_host() {
        let host = Host::remote("deploy@example.com");
        let mut place = crate::places::Place::new("rttx", "~/pro/rttx");
        place.host_tags = vec!["example.com".into()];
        let mut cmd = crate::commands::SavedCommand::new("Deploy", "cargo build");
        cmd.host_tags = vec!["example.com".into()];
        let place_uuid = place.uuid.clone();
        let cmd_uuid = cmd.uuid.clone();

        let (new_hosts, new_places, new_commands) = apply_deletion_cleanup(
            "example.com",
            &[host],
            &[place],
            &[cmd],
            &[place_uuid],
            &[cmd_uuid],
        );
        assert!(new_hosts.is_empty());
        assert!(new_places.is_empty());
        assert!(new_commands.is_empty());
    }

    #[test]
    fn apply_cleanup_keeps_unchecked_items() {
        let host = Host::remote("deploy@example.com");
        let mut place = crate::places::Place::new("keep", "/keep");
        place.host_tags = vec!["example.com".into()];
        let mut cmd = crate::commands::SavedCommand::new("Keep", "echo keep");
        cmd.host_tags = vec!["example.com".into()];

        let (new_hosts, new_places, new_commands) = apply_deletion_cleanup(
            "example.com",
            &[host],
            &[place.clone()],
            &[cmd.clone()],
            &[],
            &[],
        );
        assert!(new_hosts.is_empty());
        assert_eq!(new_places, vec![place]);
        assert_eq!(new_commands, vec![cmd]);
    }

    #[test]
    fn apply_cleanup_preserves_unrelated_items() {
        let host = Host::remote("deploy@example.com");
        let other_host = Host::remote("other@other.com");
        let global_place = crate::places::Place::new("tmp", "/tmp");
        let mut tagged_place = crate::places::Place::new("rttx", "~/pro/rttx");
        tagged_place.host_tags = vec!["example.com".into()];
        let tagged_uuid = tagged_place.uuid.clone();

        let (new_hosts, new_places, _) = apply_deletion_cleanup(
            "example.com",
            &[host, other_host.clone()],
            &[global_place.clone(), tagged_place],
            &[],
            &[tagged_uuid],
            &[],
        );
        assert_eq!(new_hosts, vec![other_host]);
        assert_eq!(new_places, vec![global_place]);
    }
}
