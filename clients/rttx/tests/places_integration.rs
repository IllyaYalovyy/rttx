use rttx::places::{self, Place};
use rttx::store::{ClientStore, StorePaths};
use tempfile::TempDir;

fn test_store() -> (TempDir, ClientStore) {
    let tmp = TempDir::new().unwrap();
    let paths = StorePaths::new(
        tmp.path().join("config"),
        tmp.path().join("state"),
        tmp.path().join("cache"),
    );
    (tmp, ClientStore::new(paths))
}

#[test]
fn place_from_cwd_roundtrips_through_store() {
    let (_tmp, store) = test_store();

    let place = Place::from_cwd("/home/user/projects/rttx", vec!["local".into()]);
    store.save_places(&[place]).unwrap();

    let loaded = store.load_places();
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
