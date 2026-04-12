use rttx::places::{self, Place};
use tempfile::TempDir;

#[test]
fn place_from_cwd_roundtrips_through_persistence() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("places.json");

    let place = Place::from_cwd("/home/user/projects/rttx", vec!["local".into()]);
    places::save_to(std::slice::from_ref(&place), &path).unwrap();

    let loaded = places::load_from(&path);
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].name, "rttx");
    assert_eq!(loaded[0].path, "/home/user/projects/rttx");
    assert_eq!(loaded[0].host_tags, vec!["local"]);
}

#[test]
fn place_from_cwd_visible_for_tagged_host() {
    let place = Place::from_cwd("/srv/app", vec!["example.com".into()]);
    let visible = places::visible_for_host(&[place], "example.com");
    // Built-ins (Home, Root) + the saved place
    assert_eq!(visible.len(), 3);
    assert_eq!(visible[2].name, "app");
}

#[test]
fn place_from_cwd_not_visible_for_other_host() {
    let place = Place::from_cwd("/srv/app", vec!["example.com".into()]);
    let visible = places::visible_for_host(&[place], "other.com");
    // Only built-ins
    assert_eq!(visible.len(), 2);
}
