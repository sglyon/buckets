//! Persistence checks use isolated settings files and never connect to providers.
#[allow(dead_code)]
#[path = "../src/appearance.rs"]
mod appearance;
#[allow(dead_code)]
#[path = "../src/keybindings.rs"]
mod keybindings;
#[allow(dead_code)]
#[path = "../src/settings.rs"]
mod settings;

use buckets_core::Location;
use serde_json::{Value, json};
use settings::{Bookmark, Settings, SettingsStore};
use std::{fs, path::PathBuf};

#[test]
fn bookmarks_restart_with_complete_provider_locations_and_opaque_metadata() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    let locations = [
        Location::local(root.path().join("offline drive/café 100%")),
        Location::Ssh {
            host: "sglyon@research".into(),
            path: "/srv/data space/100%".into(),
        },
        Location::S3 {
            bucket: "research-data".into(),
            prefix: "exports/café 100%/".into(),
            endpoint: Some("https://objects.example.invalid:9000".into()),
        },
        Location::Git {
            repo: PathBuf::from("/tmp/repo space/%literal"),
            reference: "refs/heads/feature".into(),
            path: "src/café.rs".into(),
        },
        Location::Plugin {
            plugin: "offline-provider".into(),
            path: "/docs/hello world".into(),
        },
    ];
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    let mut bookmarks = locations
        .iter()
        .enumerate()
        .map(|(i, location)| Bookmark::new(format!(" Place {i} "), location.display()).unwrap())
        .collect::<Vec<_>>();
    bookmarks[0]
        .extra
        .insert("future_view".into(), json!({"columns":["name","modified"]}));
    store
        .update(|settings| settings.bookmarks = bookmarks.clone())
        .unwrap();
    drop(store);
    let restored = SettingsStore::load(&path, Settings::default()).unwrap();
    assert_eq!(restored.settings().bookmarks, bookmarks);
    for (bookmark, expected) in restored.settings().bookmarks.iter().zip(locations) {
        assert_eq!(Location::parse(&bookmark.location).unwrap(), expected);
        assert_eq!(bookmark.canonical_location().unwrap(), bookmark.location);
        assert!(!bookmark.name.starts_with(' '));
    }
    assert_eq!(
        restored.settings().bookmarks[0].extra["future_view"],
        json!({"columns":["name","modified"]})
    );
}

#[test]
fn legacy_settings_gain_empty_bookmarks_without_rewrite_and_unknown_fields_survive_updates() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    let original=br#"{"font_size":17,"future":{"keep":true},"bookmarks":[{"name":"Data","location":"ssh://research/srv/data","future":{"color":"cyan"}}]}"#;
    fs::write(&path, original).unwrap();
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    store.update(|settings| settings.sidebar = false).unwrap();
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["future"], json!({"keep":true}));
    assert_eq!(saved["bookmarks"][0]["future"], json!({"color":"cyan"}));
    fs::write(&path, b"{\"font_size\":18}").unwrap();
    let empty = SettingsStore::load(&path, Settings::default()).unwrap();
    assert!(empty.settings().bookmarks.is_empty());
    assert_eq!(fs::read(&path).unwrap(), b"{\"font_size\":18}");
}

#[test]
fn invalid_bookmark_edits_preserve_last_good_memory_and_disk() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    store
        .update(|settings| {
            settings
                .bookmarks
                .push(Bookmark::new("Valid", "ssh://research/srv/data").unwrap())
        })
        .unwrap();
    let previous = fs::read(&path).unwrap();
    for invalid in [
        json!({"bookmarks":[{"name":"","location":"/tmp"}]}),
        json!({"bookmarks":[{"name":"bad\nname","location":"/tmp"}]}),
        json!({"bookmarks":[{"name":"Data","location":"ssh://user:password@host/path"}]}),
        json!({"bookmarks":[{"name":"Data","location":"relative/path"}]}),
        json!({"bookmarks":[{"name":"Data","location":"sftp://host/path"}]}),
        json!({"bookmarks":[{"name":"Data","location":"/tmp\u{0000}bad"}]}),
        json!({"bookmarks":"invalid"}),
        json!({"bookmarks":[{"name":"Missing location"}]}),
    ] {
        assert!(
            store.replace_json(&invalid.to_string()).is_err(),
            "accepted {invalid}"
        );
        assert_eq!(fs::read(&path).unwrap(), previous);
        assert_eq!(store.settings().bookmarks[0].name, "Valid");
    }
    assert!(
        store
            .update(
                |settings| settings.bookmarks = vec![Bookmark::new("Data", "/tmp").unwrap(); 257]
            )
            .is_err()
    );
    assert!(Bookmark::new("x".repeat(129), "/tmp").is_err());
    assert!(Bookmark::new("Too long", format!("/{}", "x".repeat(8192))).is_err());
    assert_eq!(fs::read(&path).unwrap(), previous);
}

#[test]
fn bookmarks_cannot_clobber_external_changes_and_can_be_renamed_or_removed_after_reload() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    let external = json!({"font_size":16,"bookmarks":[{"name":"External","location":"ssh://host/srv/project"}],"future":true});
    let bytes = serde_json::to_vec(&external).unwrap();
    fs::write(&path, &bytes).unwrap();
    assert!(
        store
            .update(|settings| settings
                .bookmarks
                .push(Bookmark::new("Mine", "/tmp").unwrap()))
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    store.reload().unwrap();
    store
        .update(|settings| settings.bookmarks[0].name = "Renamed".into())
        .unwrap();
    assert_eq!(
        SettingsStore::load(&path, Settings::default())
            .unwrap()
            .settings()
            .bookmarks[0]
            .name,
        "Renamed"
    );
    store
        .update(|settings| {
            settings.bookmarks.remove(0);
        })
        .unwrap();
    let saved: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(saved["bookmarks"], json!([]));
    assert_eq!(saved["future"], true);
}

#[test]
fn s3_bookmarks_reject_endpoint_credentials_and_signed_queries_but_preserve_plain_endpoints() {
    for endpoint in [
        "https://user:password@objects.example.invalid/bucket",
        "https://user@objects.example.invalid",
        "https://objects.example.invalid?X-Amz-Signature=secret",
        "https://objects.example.invalid/#token",
    ] {
        let location = Location::S3 {
            bucket: "data".into(),
            prefix: "archive/".into(),
            endpoint: Some(endpoint.into()),
        };
        let error = Bookmark::new("Private object store", location.display()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("keep credentials in your AWS configuration")
        );
        assert!(!error.to_string().contains("password@"));
    }
    let location = Location::S3 {
        bucket: "data".into(),
        prefix: "archive/".into(),
        endpoint: Some("https://objects.example.invalid:9000/api/s3".into()),
    };
    let bookmark = Bookmark::new("Object store", location.display()).unwrap();
    assert_eq!(Location::parse(&bookmark.location).unwrap(), location);
}
