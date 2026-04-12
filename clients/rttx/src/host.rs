use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::config;

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
        Self {
            key,
            name,
            kind: HostKind::Remote,
            ssh_target: Some(ssh_target.into()),
        }
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

// ── Persistence ─────────────────────────────────────────────────

fn hosts_path() -> PathBuf {
    let mut path = config::config_dir_path();
    path.push("hosts.json");
    path
}

#[must_use]
pub fn load() -> Vec<Host> {
    load_from(&hosts_path())
}

pub fn save(hosts: &[Host]) -> Result<(), Box<dyn std::error::Error>> {
    save_to(hosts, &hosts_path())
}

#[must_use]
pub fn load_from(path: &Path) -> Vec<Host> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|data| serde_json::from_str::<Vec<Host>>(&data).ok())
        .unwrap_or_default()
}

pub fn save_to(hosts: &[Host], path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(hosts)?;
    std::fs::write(path, json)?;
    Ok(())
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
    use tempfile::TempDir;

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

    // ── Persistence ─────────────────────────────────────────────

    #[test]
    fn roundtrip_via_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("hosts.json");
        let hosts = vec![Host::remote("deploy@example.com")];

        save_to(&hosts, &path).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded, hosts);
    }

    #[test]
    fn missing_file_returns_empty_list() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("missing.json");
        assert!(load_from(&path).is_empty());
    }

    #[test]
    fn local_host_not_persisted_in_saved_list() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("hosts.json");
        let hosts = vec![Host::remote("example.com")];
        save_to(&hosts, &path).unwrap();

        let loaded = load_from(&path);
        assert!(!loaded.iter().any(Host::is_local));
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
}
