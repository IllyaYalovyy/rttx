use serde::{Deserialize, Serialize};
use std::path::Path;

/// A saved navigation target — a directory path scoped to one or more hosts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Place {
    pub uuid: String,
    pub name: String,
    pub path: String,
    /// Host keys this place is scoped to. Empty means global.
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

    /// Create a place from a working directory path.
    ///
    /// The display name is derived from the last path component.
    /// `host_tags` scopes the place to specific hosts (empty = global).
    #[must_use]
    pub fn from_cwd(path: &str, host_tags: Vec<String>) -> Self {
        let name = Path::new(path)
            .file_name()
            .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned());
        Self { uuid: uuid::Uuid::new_v4().to_string(), name, path: path.to_string(), host_tags }
    }

    /// Display string: name with path in parentheses when they differ.
    #[must_use]
    pub fn display_label(&self) -> String {
        if self.name == self.path || self.path.is_empty() {
            self.name.clone()
        } else {
            format!("{} ({})", self.name, self.path)
        }
    }
}

/// Built-in global place: user home directory.
#[must_use]
pub fn builtin_home() -> Place {
    Place { uuid: "builtin:home".into(), name: "Home".into(), path: "~".into(), host_tags: vec![] }
}

/// Built-in global place: filesystem root.
#[must_use]
pub fn builtin_root() -> Place {
    Place { uuid: "builtin:root".into(), name: "Root".into(), path: "/".into(), host_tags: vec![] }
}

/// All built-in global places, in display order.
#[must_use]
pub fn builtins() -> Vec<Place> {
    vec![builtin_home(), builtin_root()]
}

/// Returns `true` if the place should be visible for the given host key.
///
/// Global places (empty `host_tags`) are visible everywhere.
/// Tagged places are visible only when `host_key` matches one of their tags.
#[must_use]
pub fn is_visible_on(place: &Place, host_key: &str) -> bool {
    place.host_tags.is_empty() || place.host_tags.iter().any(|tag| tag == host_key)
}

/// Collect places visible on `host_key`: built-ins first, then matching saved places.
#[must_use]
pub fn visible_for_host(saved: &[Place], host_key: &str) -> Vec<Place> {
    let mut result = builtins();
    result.extend(saved.iter().filter(|p| is_visible_on(p, host_key)).cloned());
    result
}

#[must_use]
pub fn matches_query(place: &Place, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }
    let query = query.to_ascii_lowercase();
    place.name.to_ascii_lowercase().contains(&query)
        || place.path.to_ascii_lowercase().contains(&query)
}

// ── Persistence ─────────────────────────────────────────────────

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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    // ── Built-in places ─────────────────────────────────────────

    #[test]
    fn builtin_home_has_stable_uuid_and_global_tags() {
        let home = builtin_home();
        assert_eq!(home.uuid, "builtin:home");
        assert_eq!(home.name, "Home");
        assert_eq!(home.path, "~");
        assert!(home.host_tags.is_empty());
    }

    #[test]
    fn builtin_root_has_stable_uuid_and_global_tags() {
        let root = builtin_root();
        assert_eq!(root.uuid, "builtin:root");
        assert_eq!(root.name, "Root");
        assert_eq!(root.path, "/");
        assert!(root.host_tags.is_empty());
    }

    #[test]
    fn builtins_returns_home_then_root() {
        let items = builtins();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "Home");
        assert_eq!(items[1].name, "Root");
    }

    // ── Display label ───────────────────────────────────────────

    #[test]
    fn from_cwd_derives_name_from_last_component() {
        let place = Place::from_cwd("/home/user/projects/rttx", vec![]);
        assert_eq!(place.name, "rttx");
        assert_eq!(place.path, "/home/user/projects/rttx");
        assert!(place.host_tags.is_empty());
    }

    #[test]
    fn from_cwd_root_path_uses_full_path_as_name() {
        let place = Place::from_cwd("/", vec![]);
        assert_eq!(place.name, "/");
        assert_eq!(place.path, "/");
    }

    #[test]
    fn from_cwd_preserves_host_tags() {
        let place = Place::from_cwd("/srv/app", vec!["example.com".into()]);
        assert_eq!(place.host_tags, vec!["example.com"]);
    }

    #[test]
    fn display_label_shows_name_and_path_when_different() {
        let place = Place::new("rttx", "~/pro/rttx");
        assert_eq!(place.display_label(), "rttx (~/pro/rttx)");
    }

    #[test]
    fn display_label_shows_only_name_when_same_as_path() {
        let place = Place::new("/srv/app", "/srv/app");
        assert_eq!(place.display_label(), "/srv/app");
    }

    #[test]
    fn display_label_shows_only_name_when_path_empty() {
        let place = Place { path: String::new(), ..Place::new("Home", "") };
        assert_eq!(place.display_label(), "Home");
    }

    // ── Visibility ──────────────────────────────────────────────

    #[test]
    fn global_place_visible_on_any_host() {
        let place = Place::new("Global", "/tmp");
        assert!(is_visible_on(&place, "local"));
        assert!(is_visible_on(&place, "example.com"));
    }

    #[test]
    fn tagged_place_visible_only_on_matching_host() {
        let mut place = Place::new("Local only", "/home/user");
        place.host_tags = vec!["local".into()];
        assert!(is_visible_on(&place, "local"));
        assert!(!is_visible_on(&place, "example.com"));
    }

    #[test]
    fn multi_tagged_place_visible_on_all_tagged_hosts() {
        let mut place = Place::new("Multi", "/shared");
        place.host_tags = vec!["local".into(), "example.com".into()];
        assert!(is_visible_on(&place, "local"));
        assert!(is_visible_on(&place, "example.com"));
        assert!(!is_visible_on(&place, "other.com"));
    }

    // ── visible_for_host ────────────────────────────────────────

    #[test]
    fn visible_for_host_includes_builtins_and_matching_saved() {
        let mut local_place = Place::new("rttx", "~/pro/rttx");
        local_place.host_tags = vec!["local".into()];
        let mut remote_place = Place::new("app", "/srv/app");
        remote_place.host_tags = vec!["example.com".into()];
        let global_place = Place::new("tmp", "/tmp");

        let saved = vec![local_place, remote_place, global_place];
        let visible = visible_for_host(&saved, "local");

        let names: Vec<&str> = visible.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Home", "Root", "rttx", "tmp"]);
    }

    #[test]
    fn visible_for_host_with_no_saved_returns_only_builtins() {
        let visible = visible_for_host(&[], "local");
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].name, "Home");
        assert_eq!(visible[1].name, "Root");
    }

    // ── Search ──────────────────────────────────────────────────

    #[test]
    fn matches_query_by_name() {
        let place = Place::new("rttx", "~/pro/rttx");
        assert!(matches_query(&place, "rttx"));
        assert!(matches_query(&place, "RTT"));
        assert!(!matches_query(&place, "redis"));
    }

    #[test]
    fn matches_query_by_path() {
        let place = Place::new("rttx", "~/pro/rttx");
        assert!(matches_query(&place, "pro/rttx"));
    }

    #[test]
    fn blank_query_matches_all() {
        let place = Place::new("Anything", "/any");
        assert!(matches_query(&place, ""));
        assert!(matches_query(&place, "   "));
    }

    // ── Persistence ─────────────────────────────────────────────

    #[test]
    fn roundtrip_via_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("places.json");
        let mut place = Place::new("rttx", "~/pro/rttx");
        place.host_tags = vec!["local".into()];

        save_to(&[place.clone()], &path).unwrap();
        assert_eq!(load_from(&path), vec![place]);
    }

    #[test]
    fn missing_file_returns_empty_list() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("missing.json");
        assert!(load_from(&path).is_empty());
    }

    #[test]
    fn legacy_json_without_host_tags_deserializes_with_empty_vec() {
        let json = r#"[{
            "uuid": "abc",
            "name": "Work",
            "path": "/work"
        }]"#;
        let places: Vec<Place> = serde_json::from_str(json).unwrap();
        assert!(places[0].host_tags.is_empty());
    }
}
