//! Leader-prefix command shortcut resolution.
//!
//! After the leader key is pressed, subsequent keystrokes are matched against
//! per-command shortcut sequences. Resolution is host/context-aware: only
//! commands visible on the current host are candidates, and the first match
//! wins. Duplicate sequences across hosts are intentional.

use crate::commands::{self, SavedCommand};

/// Result of attempting to advance a leader key sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaderMatch {
    /// No command matches the accumulated keys — abort leader mode.
    NoMatch,
    /// At least one command has a longer sequence starting with the accumulated keys.
    Partial,
    /// Exactly one command matched fully.
    Complete(String),
}

/// Resolve a key sequence against visible commands for the given host.
///
/// Returns the UUID of the first command whose `shortcut_keys` exactly matches
/// `keys`, considering only commands visible on `host_key`. When `host_key` is
/// `None`, all commands are candidates (the "All Hosts" view).
#[must_use]
pub fn resolve(commands: &[SavedCommand], keys: &[String], host_key: Option<&str>) -> LeaderMatch {
    let mut has_partial = false;

    for cmd in commands {
        if cmd.shortcut_keys.is_empty() {
            continue;
        }
        if let Some(hk) = host_key
            && !commands::is_visible_on(cmd, hk)
        {
            continue;
        }

        if cmd.shortcut_keys == keys {
            return LeaderMatch::Complete(cmd.uuid.clone());
        }
        if cmd.shortcut_keys.len() > keys.len() && cmd.shortcut_keys.starts_with(keys) {
            has_partial = true;
        }
    }

    if has_partial { LeaderMatch::Partial } else { LeaderMatch::NoMatch }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::SavedCommand;

    fn cmd(uuid: &str, keys: &[&str], host_tags: &[&str]) -> SavedCommand {
        let mut c = SavedCommand::new(uuid, "echo test");
        c.uuid = uuid.into();
        c.shortcut_keys = keys.iter().map(|s| (*s).to_string()).collect();
        c.host_tags = host_tags.iter().map(|s| (*s).to_string()).collect();
        c
    }

    #[test]
    fn resolve_exact_match_returns_complete() {
        let commands = vec![cmd("a", &["d", "k"], &[])];
        let keys = vec!["d".into(), "k".into()];
        assert_eq!(resolve(&commands, &keys, None), LeaderMatch::Complete("a".into()));
    }

    #[test]
    fn resolve_partial_prefix_returns_partial() {
        let commands = vec![cmd("a", &["d", "k"], &[])];
        let keys = vec!["d".into()];
        assert_eq!(resolve(&commands, &keys, None), LeaderMatch::Partial);
    }

    #[test]
    fn resolve_no_match_returns_no_match() {
        let commands = vec![cmd("a", &["d", "k"], &[])];
        let keys = vec!["x".into()];
        assert_eq!(resolve(&commands, &keys, None), LeaderMatch::NoMatch);
    }

    #[test]
    fn resolve_host_scoped_filters_invisible_commands() {
        let commands =
            vec![cmd("local-cmd", &["d"], &["local"]), cmd("remote-cmd", &["d"], &["example.com"])];
        let keys = vec!["d".into()];

        // From local context: only local-cmd matches
        assert_eq!(
            resolve(&commands, &keys, Some("local")),
            LeaderMatch::Complete("local-cmd".into())
        );
        // From remote context: only remote-cmd matches
        assert_eq!(
            resolve(&commands, &keys, Some("example.com")),
            LeaderMatch::Complete("remote-cmd".into())
        );
    }

    #[test]
    fn resolve_duplicate_sequences_first_visible_wins() {
        let commands = vec![cmd("first", &["d"], &["local"]), cmd("second", &["d"], &["local"])];
        let keys = vec!["d".into()];
        assert_eq!(resolve(&commands, &keys, Some("local")), LeaderMatch::Complete("first".into()));
    }

    #[test]
    fn resolve_global_command_visible_on_any_host() {
        let commands = vec![cmd("global", &["g"], &[])];
        let keys = vec!["g".into()];
        assert_eq!(
            resolve(&commands, &keys, Some("example.com")),
            LeaderMatch::Complete("global".into())
        );
    }

    #[test]
    fn resolve_no_host_key_means_all_commands_visible() {
        let commands = vec![
            cmd("local-only", &["d"], &["local"]),
            cmd("remote-only", &["r"], &["example.com"]),
        ];
        assert_eq!(
            resolve(&commands, &["d".into()], None),
            LeaderMatch::Complete("local-only".into())
        );
        assert_eq!(
            resolve(&commands, &["r".into()], None),
            LeaderMatch::Complete("remote-only".into())
        );
    }

    #[test]
    fn resolve_empty_keys_never_matches() {
        let commands = vec![cmd("a", &["d"], &[])];
        let keys: Vec<String> = vec![];
        assert_eq!(resolve(&commands, &keys, None), LeaderMatch::Partial);
    }

    #[test]
    fn resolve_command_without_shortcut_keys_is_skipped() {
        let commands = vec![cmd("no-shortcut", &[], &[])];
        let keys = vec!["d".into()];
        assert_eq!(resolve(&commands, &keys, None), LeaderMatch::NoMatch);
    }

    #[test]
    fn resolve_longer_input_than_command_sequence_no_match() {
        let commands = vec![cmd("a", &["d"], &[])];
        let keys = vec!["d".into(), "k".into()];
        assert_eq!(resolve(&commands, &keys, None), LeaderMatch::NoMatch);
    }

    #[test]
    fn resolve_mixed_partial_and_complete_prefers_complete() {
        let commands = vec![cmd("short", &["d"], &[]), cmd("long", &["d", "k"], &[])];
        let keys = vec!["d".into()];
        // "short" matches completely first
        assert_eq!(resolve(&commands, &keys, None), LeaderMatch::Complete("short".into()));
    }

    #[test]
    fn resolve_host_scoped_duplicate_different_hosts() {
        // Same sequence on different hosts — each resolves to its own command
        let commands = vec![
            cmd("deploy-prod", &["d", "p"], &["prod-host"]),
            cmd("deploy-dev", &["d", "p"], &["dev-host"]),
        ];
        let keys = vec!["d".into(), "p".into()];
        assert_eq!(
            resolve(&commands, &keys, Some("prod-host")),
            LeaderMatch::Complete("deploy-prod".into())
        );
        assert_eq!(
            resolve(&commands, &keys, Some("dev-host")),
            LeaderMatch::Complete("deploy-dev".into())
        );
    }
}
