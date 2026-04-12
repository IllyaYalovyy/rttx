use rttx::bookmarks::{self, Bookmark};
use tempfile::TempDir;

#[test]
fn bookmarks_roundtrip_all_fields() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("bookmarks.json");

    let mut bookmark = Bookmark::new("Remote Ops");
    bookmark.directory = Some("/srv/app".into());
    bookmark.ssh_target = Some("deploy@example.com".into());

    bookmarks::save_to(&[bookmark.clone()], &path).unwrap();
    assert_eq!(bookmarks::load_from(&path), vec![bookmark]);
}

#[test]
fn invalid_bookmark_json_returns_empty_list() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("bookmarks.json");

    std::fs::write(&path, "{not-json").unwrap();
    assert!(bookmarks::load_from(&path).is_empty());
}

#[test]
fn legacy_bookmark_with_tmux_session_field_loads_gracefully() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("bookmarks.json");

    std::fs::write(
        &path,
        r#"[{
            "uuid": "abc-123",
            "name": "Old Tmux Bookmark",
            "directory": "/work",
            "ssh_target": "host",
            "tmux_session": "dev"
        }]"#,
    )
    .unwrap();

    let bookmarks = bookmarks::load_from(&path);
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0].name, "Old Tmux Bookmark");
    assert_eq!(bookmarks[0].directory.as_deref(), Some("/work"));
    assert_eq!(bookmarks[0].ssh_target.as_deref(), Some("host"));
}
