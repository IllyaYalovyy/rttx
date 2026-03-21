use rttx::bookmarks::{self, Bookmark};
use tempfile::TempDir;

#[test]
fn bookmarks_roundtrip_all_fields() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("bookmarks.json");

    let mut bookmark = Bookmark::new("Remote Ops");
    bookmark.directory = Some("/srv/app".into());
    bookmark.ssh_target = Some("deploy@example.com".into());
    bookmark.tmux_session = Some("deploy".into());

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
