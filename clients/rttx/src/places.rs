use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::config;
use crate::host::{self, LOCAL_KEY};
use crate::session::PaneTarget;
use crate::shell_quote;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Place {
    pub uuid: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub host_tags: Vec<String>,
}

impl Place {
    #[must_use]
    pub fn new(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            uuid: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            path: path.into(),
            host_tags: Vec::new(),
        }
    }

    #[must_use]
    pub const fn is_global(&self) -> bool {
        self.host_tags.is_empty()
    }

    #[must_use]
    pub fn matches_host(&self, host_key: &str) -> bool {
        self.is_global() || self.host_tags.iter().any(|tag| tag == host_key)
    }

    /// Display name, auto-derived from the last path component when empty.
    #[must_use]
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            std::path::Path::new(&self.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&self.path)
        } else {
            &self.name
        }
    }

    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        parts.push(self.path.clone());
        if !self.host_tags.is_empty() {
            let tags = self.host_tags.join(", ");
            parts.push(format!("[{tags}]"));
        }
        parts.join(" ")
    }

    #[must_use]
    pub fn is_local(&self) -> bool {
        self.host_tags.is_empty()
            || (self.host_tags.len() == 1 && self.host_tags[0] == LOCAL_KEY)
    }

    /// Command to execute in the current pane.
    #[must_use]
    pub fn command(&self, active_host_key: &str) -> Option<String> {
        if self.path.is_empty() {
            return None;
        }
        let cd = format!("cd {}", shell_quote(&self.path));
        if active_host_key == LOCAL_KEY && self.is_local() {
            return Some(cd);
        }
        // If the place is tagged for a remote host and we're on that host, just cd
        if self.matches_host(active_host_key) {
            return Some(cd);
        }
        // If the place is tagged for a specific remote host and we're local, ssh + cd
        if active_host_key == LOCAL_KEY
            && let Some(ssh_target) = self.primary_remote_ssh_target()
        {
            return Some(format!("ssh -t {ssh_target} {}", shell_quote(&cd)));
        }
        Some(cd)
    }

    /// SSH target for the first remote host tag, resolved via saved hosts.
    fn primary_remote_ssh_target(&self) -> Option<String> {
        let tag = self.host_tags.first()?;
        if tag == LOCAL_KEY {
            return None;
        }
        let saved = host::load();
        let host = host::resolve(tag, &saved);
        host.ssh_target.or_else(|| Some(tag.clone()))
    }

    /// Initial working directory for the terminal when opening as a new workspace.
    #[must_use]
    pub fn session_initial_cwd(&self) -> Option<&str> {
        if self.is_local() && !self.path.is_empty() { Some(&self.path) } else { None }
    }

    /// Startup command for opening as a new workspace.
    #[must_use]
    pub fn session_startup_command(&self) -> Option<String> {
        if self.is_local() || self.path.is_empty() {
            return None;
        }
        let ssh_target = self.primary_remote_ssh_target()?;
        Some(format!("ssh -t {ssh_target} {}", shell_quote(&format!("cd {}", shell_quote(&self.path)))))
    }

    /// Remote host SSH target for workspace creation.
    #[must_use]
    pub fn remote_host(&self) -> Option<String> {
        if self.is_local() {
            return None;
        }
        self.primary_remote_ssh_target()
    }

    /// Tooltip for the "New workspace" button.
    #[must_use]
    pub fn new_workspace_tooltip(&self) -> String {
        self.remote_host().map_or_else(
            || "New workspace from place".into(),
            |host| format!("New workspace on {host}"),
        )
    }

    /// Icon name for the "New workspace" button.
    #[must_use]
    pub fn new_workspace_icon(&self) -> &'static str {
        if self.remote_host().is_some() { "network-server-symbolic" } else { "window-new-symbolic" }
    }

    /// Command to run on a remote pane already connected to the place's host.
    #[must_use]
    pub fn remote_command(&self) -> Option<String> {
        if self.is_local() || self.path.is_empty() {
            return None;
        }
        Some(format!("cd {}", shell_quote(&self.path)))
    }

    #[must_use]
    pub fn pane_target(&self) -> Option<PaneTarget> {
        if self.path.is_empty() {
            return None;
        }
        if self.is_local() {
            return Some(PaneTarget::LocalFolder { path: self.path.clone() });
        }
        let ssh_target = self.primary_remote_ssh_target()?;
        Some(PaneTarget::RemoteShell { ssh_target, remote_folder: Some(self.path.clone()) })
    }
}

// ── Built-in global places ──────────────────────────────────────

#[must_use]
pub fn builtin_places() -> Vec<Place> {
    let home_path = std::env::var("HOME").unwrap_or_else(|_| "~".into());
    vec![
        Place {
            uuid: "builtin-home".into(),
            name: "Home".into(),
            path: home_path,
            host_tags: Vec::new(),
        },
        Place {
            uuid: "builtin-root".into(),
            name: "Root".into(),
            path: "/".into(),
            host_tags: Vec::new(),
        },
    ]
}

#[must_use]
pub fn is_builtin(uuid: &str) -> bool {
    uuid == "builtin-home" || uuid == "builtin-root"
}

// ── Query matching ──────────────────────────────────────────────

#[must_use]
pub fn matches_query(place: &Place, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }
    let query = query.to_ascii_lowercase();
    place.display_name().to_ascii_lowercase().contains(&query)
        || place.path.to_ascii_lowercase().contains(&query)
        || place.summary().to_ascii_lowercase().contains(&query)
}

// ── Persistence ─────────────────────────────────────────────────

fn places_path() -> PathBuf {
    let mut path = config::config_dir_path();
    path.push("places.json");
    path
}

#[must_use]
pub fn load() -> Vec<Place> {
    load_from(&places_path())
}

pub fn save(places: &[Place]) -> Result<(), Box<dyn std::error::Error>> {
    save_to(places, &places_path())
}

#[must_use]
pub fn load_from(path: &Path) -> Vec<Place> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|data| serde_json::from_str::<Vec<Place>>(&data).ok())
        .unwrap_or_default()
}

pub fn save_to(places: &[Place], path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(places)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Move the item with `source_uuid` to the position of `target_uuid`.
pub fn reorder(items: &mut Vec<Place>, source_uuid: &str, target_uuid: &str) {
    let Some(src) = items.iter().position(|p| p.uuid == source_uuid) else {
        return;
    };
    let Some(tgt) = items.iter().position(|p| p.uuid == target_uuid) else {
        return;
    };
    let item = items.remove(src);
    items.insert(tgt, item);
}

/// All places visible for a given host: built-ins + user places matching the host.
#[must_use]
pub fn places_for_host(user_places: &[Place], host_key: &str) -> Vec<Place> {
    let mut result = builtin_places();
    result.extend(user_places.iter().filter(|p| p.matches_host(host_key)).cloned());
    result
}

// ── Migration from bookmarks ────────────────────────────────────

/// Migrate legacy bookmarks to places. Returns the migrated places and any
/// new hosts that should be saved.
///
/// Migration rules (from RFC-016):
/// - bookmark with only `directory` → local-tagged Place
/// - bookmark with only `ssh_target` → Host record (not a Place)
/// - bookmark with `ssh_target` + `directory` → Place tagged with that host key
/// - bookmark with `tmux_session` → dropped (already removed)
#[must_use]
pub fn migrate_bookmarks(bookmarks: &[crate::bookmarks::Bookmark]) -> (Vec<Place>, Vec<crate::host::Host>) {
    let mut places = Vec::new();
    let mut new_hosts = Vec::new();

    for bookmark in bookmarks {
        let directory = bookmark.directory.as_deref().map(str::trim).filter(|d| !d.is_empty());
        let ssh_target = bookmark.ssh_target.as_deref().map(str::trim).filter(|s| !s.is_empty());

        match (directory, ssh_target) {
            (Some(dir), None) => {
                places.push(Place {
                    uuid: bookmark.uuid.clone(),
                    name: bookmark.name.clone(),
                    path: dir.to_string(),
                    host_tags: vec![LOCAL_KEY.into()],
                });
            }
            (None, Some(target)) => {
                new_hosts.push(crate::host::Host::remote(target));
            }
            (Some(dir), Some(target)) => {
                let host = crate::host::Host::remote(target);
                let host_key = host.key.clone();
                new_hosts.push(host);
                places.push(Place {
                    uuid: bookmark.uuid.clone(),
                    name: bookmark.name.clone(),
                    path: dir.to_string(),
                    host_tags: vec![host_key],
                });
            }
            (None, None) => {}
        }
    }

    (places, new_hosts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    #[test]
    fn new_place_is_global_by_default() {
        let place = Place::new("Work", "/home/user/work");
        assert!(place.is_global());
        assert!(place.matches_host(LOCAL_KEY));
        assert!(place.matches_host("example.com"));
    }

    #[test]
    fn tagged_place_matches_only_tagged_hosts() {
        let place = Place {
            host_tags: vec!["example.com".into()],
            ..Place::new("Remote", "/srv/app")
        };
        assert!(!place.is_global());
        assert!(place.matches_host("example.com"));
        assert!(!place.matches_host("other.com"));
    }

    #[test]
    fn display_name_uses_explicit_name() {
        let place = Place::new("My Work", "/home/user/work");
        assert_eq!(place.display_name(), "My Work");
    }

    #[test]
    fn display_name_auto_derives_from_path() {
        let place = Place { name: String::new(), ..Place::new("", "/home/user/projects/rttx") };
        assert_eq!(place.display_name(), "rttx");
    }

    #[test]
    fn display_name_falls_back_to_full_path() {
        let place = Place { name: String::new(), ..Place::new("", "/") };
        assert_eq!(place.display_name(), "/");
    }

    #[test]
    fn local_place_command_is_cd() {
        let place = Place::new("Work", "/home/user/work");
        assert_eq!(place.command(LOCAL_KEY).as_deref(), Some("cd '/home/user/work'"));
    }

    #[test]
    fn local_tagged_place_command_is_cd() {
        let place = Place {
            host_tags: vec![LOCAL_KEY.into()],
            ..Place::new("Work", "/home/user/work")
        };
        assert_eq!(place.command(LOCAL_KEY).as_deref(), Some("cd '/home/user/work'"));
    }

    #[test]
    fn empty_path_returns_no_command() {
        let place = Place::new("Empty", "");
        assert_eq!(place.command(LOCAL_KEY), None);
    }

    #[test]
    fn session_initial_cwd_for_local_place() {
        let place = Place::new("Work", "/home/user/work");
        assert_eq!(place.session_initial_cwd(), Some("/home/user/work"));
    }

    #[test]
    fn session_initial_cwd_none_for_remote_place() {
        let place = Place {
            host_tags: vec!["example.com".into()],
            ..Place::new("Remote", "/srv/app")
        };
        assert_eq!(place.session_initial_cwd(), None);
    }

    #[test]
    fn pane_target_local_folder() {
        let place = Place::new("Work", "/home/user/work");
        assert_eq!(place.pane_target(), Some(PaneTarget::LocalFolder { path: "/home/user/work".into() }));
    }

    #[test]
    fn pane_target_empty_path_is_none() {
        let place = Place::new("Empty", "");
        assert_eq!(place.pane_target(), None);
    }

    #[test]
    fn is_builtin_identifies_builtin_uuids() {
        assert!(is_builtin("builtin-home"));
        assert!(is_builtin("builtin-root"));
        assert!(!is_builtin("user-place-123"));
    }

    #[test]
    fn builtin_places_has_home_and_root() {
        let builtins = builtin_places();
        assert_eq!(builtins.len(), 2);
        assert_eq!(builtins[0].name, "Home");
        assert_eq!(builtins[1].name, "Root");
        assert_eq!(builtins[1].path, "/");
        assert!(builtins[0].is_global());
        assert!(builtins[1].is_global());
    }

    #[test]
    fn matches_query_checks_name_and_path() {
        let place = Place::new("Work Projects", "/home/user/projects");
        assert!(matches_query(&place, "work"));
        assert!(matches_query(&place, "projects"));
        assert!(!matches_query(&place, "staging"));
    }

    #[test]
    fn matches_query_treats_blank_as_match_all() {
        let place = Place::new("Anything", "/tmp");
        assert!(matches_query(&place, ""));
        assert!(matches_query(&place, "   "));
    }

    #[test]
    fn roundtrip_via_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("places.json");
        let place = Place {
            host_tags: vec!["example.com".into()],
            ..Place::new("Remote", "/srv/app")
        };

        save_to(std::slice::from_ref(&place), &path).unwrap();
        assert_eq!(load_from(&path), vec![place]);
    }

    #[test]
    fn missing_file_returns_empty_list() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("missing.json");
        assert!(load_from(&path).is_empty());
    }

    #[test]
    fn reorder_moves_item_to_target_position() {
        let mut items = vec![
            Place { uuid: "a".into(), ..Place::new("A", "/a") },
            Place { uuid: "b".into(), ..Place::new("B", "/b") },
            Place { uuid: "c".into(), ..Place::new("C", "/c") },
        ];

        reorder(&mut items, "c", "a");
        let uuids: Vec<&str> = items.iter().map(|p| p.uuid.as_str()).collect();
        assert_eq!(uuids, vec!["c", "a", "b"]);
    }

    #[test]
    fn reorder_noop_for_unknown_uuid() {
        let mut items = vec![
            Place { uuid: "a".into(), ..Place::new("A", "/a") },
            Place { uuid: "b".into(), ..Place::new("B", "/b") },
        ];

        reorder(&mut items, "z", "a");
        let uuids: Vec<&str> = items.iter().map(|p| p.uuid.as_str()).collect();
        assert_eq!(uuids, vec!["a", "b"]);
    }

    #[test]
    fn places_for_host_includes_builtins_and_matching_user_places() {
        let user_places = vec![
            Place { host_tags: vec![LOCAL_KEY.into()], ..Place::new("Local", "/home") },
            Place { host_tags: vec!["example.com".into()], ..Place::new("Remote", "/srv") },
            Place::new("Global", "/tmp"),
        ];

        let local = places_for_host(&user_places, LOCAL_KEY);
        assert_eq!(local.len(), 4); // 2 builtins + Local + Global

        let remote = places_for_host(&user_places, "example.com");
        assert_eq!(remote.len(), 4); // 2 builtins + Remote + Global
    }

    // ── Migration tests ─────────────────────────────────────────

    #[test]
    fn migrate_directory_only_bookmark_to_local_place() {
        let mut bookmark = crate::bookmarks::Bookmark::new("Work");
        bookmark.directory = Some("/home/user/work".into());

        let (places, hosts) = migrate_bookmarks(&[bookmark]);
        assert_eq!(places.len(), 1);
        assert!(hosts.is_empty());
        assert_eq!(places[0].name, "Work");
        assert_eq!(places[0].path, "/home/user/work");
        assert_eq!(places[0].host_tags, vec![LOCAL_KEY]);
    }

    #[test]
    fn migrate_ssh_only_bookmark_to_host() {
        let mut bookmark = crate::bookmarks::Bookmark::new("Prod");
        bookmark.ssh_target = Some("deploy@example.com".into());

        let (places, hosts) = migrate_bookmarks(&[bookmark]);
        assert!(places.is_empty());
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].key, "example.com");
    }

    #[test]
    fn migrate_combined_bookmark_to_tagged_place_and_host() {
        let mut bookmark = crate::bookmarks::Bookmark::new("Remote Ops");
        bookmark.directory = Some("/srv/app".into());
        bookmark.ssh_target = Some("deploy@example.com".into());

        let (places, hosts) = migrate_bookmarks(&[bookmark]);
        assert_eq!(places.len(), 1);
        assert_eq!(hosts.len(), 1);
        assert_eq!(places[0].path, "/srv/app");
        assert_eq!(places[0].host_tags, vec!["example.com"]);
    }

    #[test]
    fn migrate_empty_bookmark_is_dropped() {
        let bookmark = crate::bookmarks::Bookmark::new("Empty");
        let (places, hosts) = migrate_bookmarks(&[bookmark]);
        assert!(places.is_empty());
        assert!(hosts.is_empty());
    }

    #[test]
    fn migrate_preserves_bookmark_uuid() {
        let mut bookmark = crate::bookmarks::Bookmark::new("Work");
        bookmark.uuid = "original-uuid".into();
        bookmark.directory = Some("/work".into());

        let (places, _) = migrate_bookmarks(&[bookmark]);
        assert_eq!(places[0].uuid, "original-uuid");
    }

    #[test]
    fn summary_includes_path_and_tags() {
        let place = Place {
            host_tags: vec!["example.com".into()],
            ..Place::new("Remote", "/srv/app")
        };
        assert_eq!(place.summary(), "/srv/app [example.com]");
    }

    #[test]
    fn summary_for_global_place_is_just_path() {
        let place = Place::new("Work", "/home/user/work");
        assert_eq!(place.summary(), "/home/user/work");
    }

    #[test]
    fn new_workspace_tooltip_for_local() {
        let place = Place::new("Work", "/home/user/work");
        assert_eq!(place.new_workspace_tooltip(), "New workspace from place");
    }

    #[test]
    fn new_workspace_icon_for_local() {
        let place = Place::new("Work", "/home/user/work");
        assert_eq!(place.new_workspace_icon(), "window-new-symbolic");
    }

    #[test]
    fn remote_command_for_local_is_none() {
        let place = Place::new("Work", "/home/user/work");
        assert!(place.remote_command().is_none());
    }

    #[test]
    fn remote_command_for_tagged_remote() {
        let place = Place {
            host_tags: vec!["example.com".into()],
            ..Place::new("Remote", "/srv/app")
        };
        assert_eq!(place.remote_command().as_deref(), Some("cd '/srv/app'"));
    }

    #[test]
    fn is_local_for_global_place() {
        let place = Place::new("Global", "/tmp");
        assert!(place.is_local());
    }

    #[test]
    fn is_local_for_local_tagged() {
        let place = Place {
            host_tags: vec![LOCAL_KEY.into()],
            ..Place::new("Local", "/home")
        };
        assert!(place.is_local());
    }

    #[test]
    fn is_local_false_for_remote_tagged() {
        let place = Place {
            host_tags: vec!["example.com".into()],
            ..Place::new("Remote", "/srv")
        };
        assert!(!place.is_local());
    }
}
