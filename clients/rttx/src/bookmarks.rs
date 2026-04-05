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
    #[serde(default)]
    pub tmux_session: Option<String>,
}

impl Bookmark {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            uuid: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            directory: None,
            ssh_target: None,
            tmux_session: None,
        }
    }

    #[must_use]
    pub fn command(&self) -> Option<String> {
        let directory = non_empty(self.directory.as_deref());
        let ssh_target = non_empty(self.ssh_target.as_deref());
        let tmux_session = non_empty(self.tmux_session.as_deref());
        let local_command = local_command(directory, tmux_session);

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
        if let Some(session) = non_empty(self.tmux_session.as_deref()) {
            parts.push(format!("tmux {session}"));
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
        let directory = non_empty(self.directory.as_deref());
        let ssh_target = non_empty(self.ssh_target.as_deref());
        let tmux_session = non_empty(self.tmux_session.as_deref());

        match (ssh_target, tmux_session) {
            (Some(target), _) => {
                let remote_cmd = local_command(directory, tmux_session);
                Some(remote_cmd.map_or_else(
                    || format!("ssh {target}"),
                    |cmd| format!("ssh -t {target} {}", shell_quote(&cmd)),
                ))
            }
            (None, Some(session)) => Some(tmux_command(session)),
            (None, None) => None,
        }
    }

    /// SSH host for remote workspace creation, if this bookmark targets a remote host.
    #[must_use]
    pub fn remote_host(&self) -> Option<&str> {
        non_empty(self.ssh_target.as_deref())
    }

    /// Command to run on a remote pane that is already connected to the bookmark's host.
    /// Returns the inner command (cd, tmux, etc.) without the SSH wrapper.
    /// None if the bookmark has no SSH target or no inner command beyond just connecting.
    #[must_use]
    pub fn remote_command(&self) -> Option<String> {
        non_empty(self.ssh_target.as_deref())?;
        local_command(non_empty(self.directory.as_deref()), non_empty(self.tmux_session.as_deref()))
    }

    #[must_use]
    pub fn pane_target(&self) -> Option<PaneTarget> {
        let directory = non_empty(self.directory.as_deref()).map(str::to_string);
        let ssh_target = non_empty(self.ssh_target.as_deref()).map(str::to_string);
        let tmux_session = non_empty(self.tmux_session.as_deref()).map(str::to_string);

        match (directory, ssh_target, tmux_session) {
            (Some(path), None, None) => Some(PaneTarget::LocalFolder { path }),
            (None, None, Some(session)) => Some(PaneTarget::LocalTmux { session }),
            (Some(_path), None, Some(session)) => Some(PaneTarget::LocalTmux { session }),
            (None, Some(ssh_target), None) => {
                Some(PaneTarget::RemoteShell { ssh_target, remote_folder: None })
            }
            (Some(path), Some(ssh_target), None) => {
                Some(PaneTarget::RemoteShell { ssh_target, remote_folder: Some(path) })
            }
            (None | Some(_), Some(ssh_target), Some(tmux_session)) => {
                Some(PaneTarget::RemoteTmux { ssh_target, tmux_session })
            }
            (None, None, None) => None,
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

fn local_command(directory: Option<&str>, tmux_session: Option<&str>) -> Option<String> {
    match (directory, tmux_session) {
        (Some(directory), Some(session)) => {
            Some(format!("cd {} && ({})", shell_quote(directory), tmux_command(session)))
        }
        (Some(directory), None) => Some(format!("cd {}", shell_quote(directory))),
        (None, Some(session)) => Some(tmux_command(session)),
        (None, None) => None,
    }
}

fn tmux_command(session: &str) -> String {
    let session = shell_quote(session);
    format!("tmux attach-session -t {session} || tmux new-session -s {session}")
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
    fn tmux_bookmark_builds_attach_or_create_command() {
        let mut bookmark = Bookmark::new("Ops");
        bookmark.tmux_session = Some("ops".into());

        assert_eq!(
            bookmark.command().as_deref(),
            Some("tmux attach-session -t 'ops' || tmux new-session -s 'ops'")
        );
    }

    #[test]
    fn combined_bookmark_builds_remote_cd_then_tmux_command() {
        let mut bookmark = Bookmark::new("Remote Ops");
        bookmark.directory = Some("/srv/app".into());
        bookmark.ssh_target = Some("deploy@example.com".into());
        bookmark.tmux_session = Some("deploy".into());

        assert_eq!(
            bookmark.command().as_deref(),
            Some(
                "ssh -t deploy@example.com 'cd '\"'\"'/srv/app'\"'\"' && (tmux attach-session -t '\"'\"'deploy'\"'\"' || tmux new-session -s '\"'\"'deploy'\"'\"')'"
            )
        );
    }

    #[test]
    fn pane_target_prefers_remote_tmux_for_ssh_tmux_bookmarks() {
        let mut bookmark = Bookmark::new("Remote Ops");
        bookmark.directory = Some("/srv/app".into());
        bookmark.ssh_target = Some("deploy@example.com".into());
        bookmark.tmux_session = Some("deploy".into());

        assert_eq!(
            bookmark.pane_target(),
            Some(PaneTarget::RemoteTmux {
                ssh_target: "deploy@example.com".into(),
                tmux_session: "deploy".into(),
            })
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
        bookmark.tmux_session = Some("web".into());

        assert!(matches_query(&bookmark, "prod"));
        assert!(matches_query(&bookmark, "srv/app"));
        assert!(matches_query(&bookmark, "deploy@example.com"));
        assert!(matches_query(&bookmark, "tmux attach-session"));
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
        bookmark.tmux_session = Some("main".into());

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
    fn local_dir_and_tmux_session_initial_cwd_is_dir_and_startup_command_is_tmux_only() {
        let mut bookmark = Bookmark::new("Local Dev");
        bookmark.directory = Some("/home/user/work".into());
        bookmark.tmux_session = Some("dev".into());

        assert_eq!(bookmark.session_initial_cwd(), Some("/home/user/work"));
        assert_eq!(
            bookmark.session_startup_command().as_deref(),
            Some("tmux attach-session -t 'dev' || tmux new-session -s 'dev'")
        );
    }

    #[test]
    fn ssh_with_dir_and_tmux_session_startup_command_includes_full_chain() {
        let mut bookmark = Bookmark::new("Remote Dev");
        bookmark.ssh_target = Some("deploy@example.com".into());
        bookmark.directory = Some("/srv/app".into());
        bookmark.tmux_session = Some("web".into());

        assert_eq!(bookmark.session_initial_cwd(), None);
        assert_eq!(
            bookmark.session_startup_command().as_deref(),
            Some(
                "ssh -t deploy@example.com 'cd '\"'\"'/srv/app'\"'\"' && (tmux attach-session -t '\"'\"'web'\"'\"' || tmux new-session -s '\"'\"'web'\"'\"')'"
            )
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
    fn remote_command_returns_inner_command_without_ssh() {
        let mut b = Bookmark::new("Deploy");
        b.ssh_target = Some("deploy@example.com".into());
        b.directory = Some("/srv/app".into());
        b.tmux_session = Some("web".into());

        let full = b.command().unwrap();
        assert!(full.starts_with("ssh"), "full command should start with ssh: {full}");

        let inner = b.remote_command().unwrap();
        assert!(!inner.contains("ssh"), "remote_command must not contain ssh: {inner}");
        assert!(inner.contains("/srv/app"), "remote_command must contain directory");
        assert!(inner.contains("tmux"), "remote_command must contain tmux");
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
}
