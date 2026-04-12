use rttx::places::{self, Place};
use tempfile::TempDir;

#[test]
fn places_roundtrip_all_fields() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("places.json");

    let place = Place {
        host_tags: vec!["example.com".into()],
        ..Place::new("Remote Ops", "/srv/app")
    };

    places::save_to(std::slice::from_ref(&place), &path).unwrap();
    assert_eq!(places::load_from(&path), vec![place]);
}

#[test]
fn invalid_places_json_returns_empty_list() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("places.json");

    std::fs::write(&path, "{not-json").unwrap();
    assert!(places::load_from(&path).is_empty());
}

#[test]
fn place_without_host_tags_deserializes_as_global() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("places.json");

    std::fs::write(
        &path,
        r#"[{
            "uuid": "abc-123",
            "name": "Work",
            "path": "/home/user/work"
        }]"#,
    )
    .unwrap();

    let places = places::load_from(&path);
    assert_eq!(places.len(), 1);
    assert_eq!(places[0].name, "Work");
    assert!(places[0].is_global());
    assert!(places[0].host_tags.is_empty());
}

#[test]
fn migrate_bookmarks_converts_directory_only_to_local_place() {
    let mut bookmark = rttx::bookmarks::Bookmark::new("Work");
    bookmark.directory = Some("/home/user/work".into());

    let (places, hosts) = places::migrate_bookmarks(&[bookmark]);
    assert_eq!(places.len(), 1);
    assert!(hosts.is_empty());
    assert_eq!(places[0].path, "/home/user/work");
    assert_eq!(places[0].host_tags, vec!["local"]);
}

#[test]
fn migrate_bookmarks_converts_ssh_only_to_host() {
    let mut bookmark = rttx::bookmarks::Bookmark::new("Prod");
    bookmark.ssh_target = Some("deploy@example.com".into());

    let (places, hosts) = places::migrate_bookmarks(&[bookmark]);
    assert!(places.is_empty());
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].key, "example.com");
}

#[test]
fn migrate_bookmarks_converts_combined_to_tagged_place() {
    let mut bookmark = rttx::bookmarks::Bookmark::new("Remote Ops");
    bookmark.directory = Some("/srv/app".into());
    bookmark.ssh_target = Some("deploy@example.com".into());

    let (places, hosts) = places::migrate_bookmarks(&[bookmark]);
    assert_eq!(places.len(), 1);
    assert_eq!(hosts.len(), 1);
    assert_eq!(places[0].host_tags, vec!["example.com"]);
    assert_eq!(places[0].path, "/srv/app");
}
