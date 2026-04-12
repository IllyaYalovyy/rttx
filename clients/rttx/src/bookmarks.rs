use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::config;
use crate::session::PaneTarget;
use crate::shell_quote;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bookmark {
    pub uuid: String,
    pub name: String,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default)]
    pub ssh_target: Option<String>,
}

impl Bookmark {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            uuid: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            directory: None,
            ssh_target: None,
        }
    }

    #[must_use]
    pub fn command(&self) -> Option<String> {
        let directory = non_empty(self.directory.as_deref());
        let ssh_target = non_empty(self.ssh_target.as_deref());
        let local_command = directory.map(|d| format!("cd {}", shell_quote(d)));

        match (ssh_target, local_command) {
            (Some(target), Some(command)) => {
                Some(format!("ssh -t {target} {}", shell_quote(&command)))
            }
            (Some(target), None) => Some(format!("ssh {target}")),
            (None, Some(command)) => Some(command),
            (None, None) => None,
        }
    }

    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(directory) = non_empty(self.directory.as_deref()) {
            parts.push(directory.to_string());
        }
        if let Some(target) = non_empty(self.ssh_target.as_deref()) {
            parts.push(format!("ssh {target}"));
        }

        if parts.is_empty() { "Empty bookmark".into() } else { parts.join(" | ") }
    }

    #[must_use]
    pub fn is_actionable(&self) -> bool {
        self.command().is_some()
    }

    /// Initial working directory for the terminal when opening as a new session.
    /// Only set for local bookmarks (no SSH host); SSH bookmarks must always start in home.
    #[must_use]
    pub fn session_initial_cwd(&self) -> Option<&str> {
        if self.ssh_target.is_none() { non_empty(self.directory.as_deref()) } else { None }
    }

    /// Startup command to send to the shell when opening as a new session.
    /// None for local directory-only bookmarks — `session_initial_cwd` handles the directory.
    #[must_use]
    pub fn session_startup_command(&self) -> Option<String> {
        let ssh_target = non_empty(self.ssh_target.as_deref());

        ssh_target.map(|target| {
            let directory = non_empty(self.directory.as_deref());
            directory.map_or_else(
                || format!("ssh {target}"),
                |dir| {
                    format!("ssh -t {target} {}", shell_quote(&format!("cd {}", shell_quote(dir))))
                },
            )
        })
    }

    /// SSH host for remote workspace creation, if this bookmark targets a remote host.
    #[must_use]
    pub fn remote_host(&self) -> Option<&str> {
        non_empty(self.ssh_target.as_deref())
    }

    /// Tooltip for the "New workspace" button.
    #[must_use]
    pub fn new_workspace_tooltip(&self) -> String {
        self.remote_host().map_or_else(
            || "New workspace from bookmark".into(),
            |host| format!("New workspace on {host}"),
        )
    }

    /// Icon name for the "New workspace" button.
    #[must_use]
    pub fn new_workspace_icon(&self) -> &'static str {
        if self.remote_host().is_some() { "network-server-symbolic" } else { "window-new-symbolic" }
    }

    /// Command to run on a remote pane that is already connected to the bookmark's host.
    /// Returns the inner command (cd, etc.) without the SSH wrapper.
    /// None if the bookmark has no SSH target or no inner command beyond just connecting.
    #[must_use]
    pub fn remote_command(&self) -> Option<String> {
        non_empty(self.ssh_target.as_deref())?;
        non_empty(self.directory.as_deref()).map(|d| format!("cd {}", shell_quote(d)))
    }

    #[must_use]
    pub fn pane_target(&self) -> Option<PaneTarget> {
        let directory = non_empty(self.directory.as_deref()).map(str::to_string);
        let ssh_target = non_empty(self.ssh_target.as_deref()).map(str::to_string);

        match (directory, ssh_target) {
            (Some(path), None) => Some(PaneTarget::LocalFolder { path }),
            (None, Some(ssh_target)) => {
                Some(PaneTarget::RemoteShell { ssh_target, remote_folder: None })
            }
            (Some(path), Some(ssh_target)) => {
                Some(PaneTarget::RemoteShell { ssh_target, remote_folder: Some(path) })
            }
            (None, None) => None,
        }
    }
}

#[must_use]
pub fn matches_query(bookmark: &Bookmark, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }

    let query = query.to_ascii_lowercase();
    bookmark.name.to_ascii_lowercase().contains(&query)
        || bookmark.summary().to_ascii_lowercase().contains(&query)
        || bookmark
            .command()
            .as_deref()
            .is_some_and(|command| command.to_ascii_lowercase().contains(&query))
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn bookmarks_path() -> PathBuf {
    let mut path = config::config_dir_path();
    path.push("bookmarks.json");
    path
}

#[must_use]
pub fn load() -> Vec<Bookmark> {
    load_from(&bookmarks_path())
}

pub fn save(bookmarks: &[Bookmark]) -> Result<(), Box<dyn std::error::Error>> {
    save_to(bookmarks, &bookmarks_path())
}

#[must_use]
pub fn load_from(path: &Path) -> Vec<Bookmark> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|data| serde_json::from_str::<Vec<Bookmark>>(&data).ok())
        .unwrap_or_default()
}

/// Move the item with `source_uuid` to the position of `target_uuid`.
pub fn reorder(items: &mut Vec<Bookmark>, source_uuid: &str, target_uuid: &str) {
    let Some(src) = items.iter().position(|b| b.uuid == source_uuid) else {
        return;
    };
    let Some(tgt) = items.iter().position(|b| b.uuid == target_uuid) else {
        return;
    };
    let item = items.remove(src);
    items.insert(tgt, item);
}

pub fn save_to(bookmarks: &[Bookmark], path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(bookmarks)?;
    std::fs::write(path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    #[test]
    fn folder_bookmark_builds_cd_command() {
        let mut bookmark = Bookmark::new("Code");
        bookmark.directory = Some("/home/user/Projects/rttx".into());

        assert_eq!(bookmark.command().as_deref(), Some("cd '/home/user/Projects/rttx'"));
    }

    #[test]
    fn ssh_bookmark_builds_ssh_command() {
        let mut bookmark = Bookmark::new("Prod");
        bookmark.ssh_target = Some("-p 2222 root@example.com".into());

        assert_eq!(bookmark.command().as_deref(), Some("ssh -p 2222 root@example.com"));
    }

    #[test]
    fn combined_bookmark_builds_remote_cd_command() {
        let mut bookmark = Bookmark::new("Remote Ops");
        bookmark.directory = Some("/srv/app".into());
        bookmark.ssh_target = Some("deploy@example.com".into());

        assert_eq!(
            bookmark.command().as_deref(),
            Some("ssh -t deploy@example.com 'cd '\"'\"'/srv/app'\"'\"''")
        );
    }

    #[test]
    fn pane_target_preserves_remote_folder_for_plain_ssh_bookmarks() {
        let mut bookmark = Bookmark::new("Remote Shell");
        bookmark.directory = Some("/srv/app".into());
        bookmark.ssh_target = Some("deploy@example.com".into());

        assert_eq!(
            bookmark.pane_target(),
            Some(PaneTarget::RemoteShell {
                ssh_target: "deploy@example.com".into(),
                remote_folder: Some("/srv/app".into()),
            })
        );
    }

    #[test]
    fn empty_bookmark_is_not_actionable() {
        let bookmark = Bookmark::new("Empty");
        assert!(!bookmark.is_actionable());
        assert_eq!(bookmark.command(), None);
    }

    #[test]
    fn matches_query_checks_title_summary_and_command() {
        let mut bookmark = Bookmark::new("Prod Web");
        bookmark.directory = Some("/srv/app".into());
        bookmark.ssh_target = Some("deploy@example.com".into());

        assert!(matches_query(&bookmark, "prod"));
        assert!(matches_query(&bookmark, "srv/app"));
        assert!(matches_query(&bookmark, "deploy@example.com"));
        assert!(!matches_query(&bookmark, "staging"));
    }

    #[test]
    fn matches_query_treats_blank_query_as_match_all() {
        let bookmark = Bookmark::new("Anything");
        assert!(matches_query(&bookmark, ""));
        assert!(matches_query(&bookmark, "   "));
    }

    #[test]
    fn bookmark_roundtrip_via_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bookmarks.json");
        let mut bookmark = Bookmark::new("Work");
        bookmark.directory = Some("/work".into());
        bookmark.ssh_target = Some("dev@example.com".into());

        save_to(&[bookmark.clone()], &path).unwrap();
        assert_eq!(load_from(&path), vec![bookmark]);
    }

    #[test]
    fn folder_bookmark_session_initial_cwd_is_the_directory() {
        let mut bookmark = Bookmark::new("Work");
        bookmark.directory = Some("/home/user/work".into());

        assert_eq!(bookmark.session_initial_cwd(), Some("/home/user/work"));
    }

    #[test]
    fn folder_bookmark_session_startup_command_is_none() {
        let mut bookmark = Bookmark::new("Work");
        bookmark.directory = Some("/home/user/work".into());

        assert_eq!(bookmark.session_startup_command(), None);
    }

    #[test]
    fn ssh_bookmark_session_initial_cwd_is_none() {
        let mut bookmark = Bookmark::new("Prod");
        bookmark.ssh_target = Some("deploy@example.com".into());
        bookmark.directory = Some("/srv/app".into());

        assert_eq!(bookmark.session_initial_cwd(), None);
    }

    #[test]
    fn ssh_bookmark_session_startup_command_is_ssh() {
        let mut bookmark = Bookmark::new("Prod");
        bookmark.ssh_target = Some("deploy@example.com".into());

        assert_eq!(bookmark.session_startup_command().as_deref(), Some("ssh deploy@example.com"));
    }

    #[test]
    fn ssh_with_dir_session_startup_command_includes_cd() {
        let mut bookmark = Bookmark::new("Remote Dev");
        bookmark.ssh_target = Some("deploy@example.com".into());
        bookmark.directory = Some("/srv/app".into());

        assert_eq!(bookmark.session_initial_cwd(), None);
        assert_eq!(
            bookmark.session_startup_command().as_deref(),
            Some("ssh -t deploy@example.com 'cd '\"'\"'/srv/app'\"'\"''")
        );
    }

    #[test]
    fn missing_bookmark_file_returns_empty_list() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("missing.json");
        assert!(load_from(&path).is_empty());
    }

    #[test]
    fn reorder_moves_item_to_target_position() {
        let mut items = vec![
            Bookmark { uuid: "a".into(), ..Bookmark::new("A") },
            Bookmark { uuid: "b".into(), ..Bookmark::new("B") },
            Bookmark { uuid: "c".into(), ..Bookmark::new("C") },
        ];

        reorder(&mut items, "c", "a");
        let uuids: Vec<&str> = items.iter().map(|b| b.uuid.as_str()).collect();
        assert_eq!(uuids, vec!["c", "a", "b"]);
    }

    #[test]
    fn reorder_noop_for_unknown_uuid() {
        let mut items = vec![
            Bookmark { uuid: "a".into(), ..Bookmark::new("A") },
            Bookmark { uuid: "b".into(), ..Bookmark::new("B") },
        ];

        reorder(&mut items, "z", "a");
        let uuids: Vec<&str> = items.iter().map(|b| b.uuid.as_str()).collect();
        assert_eq!(uuids, vec!["a", "b"]);
    }

    #[test]
    fn reorder_persists_through_save_and_load() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bookmarks.json");
        let mut items = vec![
            Bookmark { uuid: "a".into(), ..Bookmark::new("A") },
            Bookmark { uuid: "b".into(), ..Bookmark::new("B") },
            Bookmark { uuid: "c".into(), ..Bookmark::new("C") },
        ];

        reorder(&mut items, "c", "b");
        save_to(&items, &path).unwrap();

        let loaded = load_from(&path);
        let uuids: Vec<&str> = loaded.iter().map(|b| b.uuid.as_str()).collect();
        assert_eq!(uuids, vec!["a", "c", "b"]);
    }

    #[test]
    fn ssh_bookmark_reports_remote_host() {
        let mut b = Bookmark::new("Remote");
        b.ssh_target = Some("deploy@example.com".into());
        assert_eq!(b.remote_host(), Some("deploy@example.com"));
    }

    #[test]
    fn local_bookmark_reports_no_remote_host() {
        let mut b = Bookmark::new("Local");
        b.directory = Some("/home/user".into());
        assert_eq!(b.remote_host(), None);
    }

    #[test]
    fn empty_ssh_target_reports_no_remote_host() {
        let mut b = Bookmark::new("Empty");
        b.ssh_target = Some("  ".into());
        assert_eq!(b.remote_host(), None);
    }

    #[test]
    fn new_workspace_tooltip_includes_host_for_remote() {
        let mut b = Bookmark::new("Deploy");
        b.ssh_target = Some("deploy@example.com".into());
        assert_eq!(b.new_workspace_tooltip(), "New workspace on deploy@example.com");
    }

    #[test]
    fn new_workspace_tooltip_is_generic_for_local() {
        let b = Bookmark::new("Local");
        assert_eq!(b.new_workspace_tooltip(), "New workspace from bookmark");
    }

    #[test]
    fn new_workspace_icon_differs_for_remote() {
        let mut remote = Bookmark::new("Remote");
        remote.ssh_target = Some("host".into());
        let local = Bookmark::new("Local");
        assert_ne!(remote.new_workspace_icon(), local.new_workspace_icon());
    }

    #[test]
    fn remote_command_returns_inner_command_without_ssh() {
        let mut b = Bookmark::new("Deploy");
        b.ssh_target = Some("deploy@example.com".into());
        b.directory = Some("/srv/app".into());

        let full = b.command().unwrap();
        assert!(full.starts_with("ssh"), "full command should start with ssh: {full}");

        let inner = b.remote_command().unwrap();
        assert!(!inner.contains("ssh"), "remote_command must not contain ssh: {inner}");
        assert!(inner.contains("/srv/app"), "remote_command must contain directory");
    }

    #[test]
    fn remote_command_for_ssh_only_bookmark_is_none() {
        let mut b = Bookmark::new("Shell");
        b.ssh_target = Some("deploy@example.com".into());
        assert!(b.remote_command().is_none(), "SSH-only bookmark has no inner command to run");
    }

    #[test]
    fn remote_command_for_local_bookmark_is_none() {
        let mut b = Bookmark::new("Local");
        b.directory = Some("/home/user".into());
        assert!(b.remote_command().is_none());
    }

    #[test]
    fn legacy_bookmark_with_tmux_session_field_loads_without_error() {
        let json = r#"[{
            "uuid": "abc",
            "name": "Old",
            "directory": "/work",
            "ssh_target": "host",
            "tmux_session": "dev"
        }]"#;
        let bookmarks: Vec<Bookmark> = serde_json::from_str(json).unwrap();
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks[0].name, "Old");
        assert_eq!(bookmarks[0].directory.as_deref(), Some("/work"));
    }
}
