//! Places preferences use private fixture files; projection never connects to providers.
#[allow(dead_code)]
#[path = "../src/appearance.rs"]
mod appearance;
#[allow(dead_code)]
#[path = "../src/keybindings.rs"]
mod keybindings;
#[allow(dead_code)]
#[path = "../src/places.rs"]
mod places;
#[allow(dead_code)]
#[path = "../src/settings.rs"]
mod settings;

use buckets_core::Location;
use places::{BuiltinPlace, Places};
use serde_json::{Value, json};
use settings::{Bookmark, Settings, SettingsStore};
use std::{fs, path::Path};

#[test]
fn legacy_settings_gain_default_places_without_rewriting_bookmarks_or_metadata() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    let original = br#"{"font_size":17,"future":{"keep":true},"bookmarks":[{"name":"Research","location":"ssh://sglyon@research/srv/data%20set","future":{"color":"cyan"}}]}"#;
    fs::write(&path, original).unwrap();
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    let bookmarks = store.settings().bookmarks.clone();
    assert_eq!(store.settings().places.builtins, BuiltinPlace::ALL);
    assert_eq!(fs::read(&path).unwrap(), original);
    store
        .update(|settings| settings.places.toggle(BuiltinPlace::Desktop))
        .unwrap();
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let original: Value = serde_json::from_slice(original).unwrap();
    assert_eq!(saved["bookmarks"], original["bookmarks"]);
    assert_eq!(saved["future"], original["future"]);
    let reopened = SettingsStore::load(&path, Settings::default()).unwrap();
    assert_eq!(reopened.settings().bookmarks, bookmarks);
    assert!(
        !reopened
            .settings()
            .places
            .builtins
            .contains(&BuiltinPlace::Desktop)
    );
}

#[test]
fn toggles_restart_with_configured_order_and_preserve_unknown_places_fields() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    fs::write(
        &path,
        br#"{"places":{"builtins":["root","home"],"future":{"pinned":true}},"bookmarks":[]}"#,
    )
    .unwrap();
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    store
        .update(|settings| {
            settings.places.toggle(BuiltinPlace::Home);
            settings.places.toggle(BuiltinPlace::Downloads);
        })
        .unwrap();
    let reopened = SettingsStore::load(&path, Settings::default()).unwrap();
    assert_eq!(
        reopened.settings().places.builtins,
        [BuiltinPlace::Root, BuiltinPlace::Downloads]
    );
    assert_eq!(
        reopened.settings().places.extra["future"],
        json!({"pinned":true})
    );
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["places"]["builtins"], json!(["root", "downloads"]));
    assert_eq!(
        serde_json::to_value(BuiltinPlace::LaunchDirectory).unwrap(),
        "launch_directory"
    );
}

#[test]
fn place_toggles_cannot_clobber_external_settings_or_other_writer() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    let mut first = SettingsStore::load(&path, Settings::default()).unwrap();
    let mut second = SettingsStore::load(&path, Settings::default()).unwrap();
    second
        .update(|settings| settings.places.toggle(BuiltinPlace::Root))
        .unwrap();
    let external = fs::read(&path).unwrap();
    assert!(
        first
            .update(|settings| settings.places.toggle(BuiltinPlace::Home))
            .is_err()
    );
    assert_eq!(first.settings().places, Places::default());
    assert_eq!(fs::read(&path).unwrap(), external);
    first.reload().unwrap();
    first
        .update(|settings| settings.places.toggle(BuiltinPlace::Home))
        .unwrap();
    let reopened = SettingsStore::load(&path, Settings::default()).unwrap();
    assert!(
        !reopened
            .settings()
            .places
            .builtins
            .contains(&BuiltinPlace::Root)
    );
    assert!(
        !reopened
            .settings()
            .places
            .builtins
            .contains(&BuiltinPlace::Home)
    );
}

#[test]
fn invalid_places_never_clobber_good_settings_and_broken_source_requires_repair() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    let good = fs::read(&path).unwrap();
    for invalid in [
        json!({"places":{"builtins":["home","home"]}}),
        json!({"places":{"builtins":["workspace"]}}),
        json!({"places":{"builtins":"home"}}),
        json!({"places":null}),
    ] {
        assert!(
            store.replace_json(&invalid.to_string()).is_err(),
            "accepted {invalid}"
        );
        assert_eq!(fs::read(&path).unwrap(), good);
        assert_eq!(store.settings().places, Places::default());
    }
    assert!(
        store
            .update(|settings| settings.places.builtins.push(BuiltinPlace::Home))
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), good);
    let broken = br#"{"places":{"builtins":["root","root"]},"future":true}"#;
    fs::write(&path, broken).unwrap();
    let (mut recovered, warning) = SettingsStore::recover(&path, Settings::default()).unwrap();
    assert!(warning.is_some());
    assert!(
        recovered
            .update(|settings| settings.places.toggle(BuiltinPlace::Root))
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), broken);
}

#[test]
fn projection_preserves_saved_order_and_full_remote_targets_before_optional_builtins() {
    let locations = [
        Location::Ssh {
            host: "sglyon@offline-host".into(),
            path: "/srv/café data".into(),
        },
        Location::S3 {
            bucket: "research".into(),
            prefix: "exports/100%/".into(),
            endpoint: Some("https://objects.example.invalid:9000/storage".into()),
        },
        Location::Git {
            repo: "/missing/repo space".into(),
            reference: "refs/heads/feature".into(),
            path: "src/café.rs".into(),
        },
        Location::Plugin {
            plugin: "offline-provider".into(),
            path: "/saved/path".into(),
        },
    ];
    let bookmarks = locations
        .iter()
        .enumerate()
        .map(|(i, location)| Bookmark::new(format!("Favorite {i}"), location.display()).unwrap())
        .collect::<Vec<_>>();
    let places = Places {
        builtins: vec![BuiltinPlace::Downloads, BuiltinPlace::Home],
        ..Default::default()
    };
    let rows = places.project(
        &bookmarks,
        Path::new("/missing/home"),
        Path::new("/missing/launch"),
    );
    assert_eq!(rows.len(), 6);
    for (i, expected) in locations.into_iter().enumerate() {
        assert_eq!(rows[i].location, expected);
        assert_eq!(rows[i].label, format!("Favorite {i}"));
        assert_eq!(rows[i].bookmark_index, Some(i));
        assert_eq!(rows[i].builtin, None);
    }
    assert_eq!(rows[4].label, "Downloads");
    assert_eq!(rows[5].location, Location::local("/missing/home"));
    assert_eq!(rows[5].builtin, Some(BuiltinPlace::Home));
    assert_eq!(
        Places {
            builtins: vec![],
            ..Default::default()
        }
        .project(&bookmarks, Path::new("/"), Path::new("/"))
        .len(),
        bookmarks.len()
    );
}

#[test]
fn projection_suppresses_redundant_builtins_without_merging_saved_favorites() {
    let home = Path::new("/missing/home space");
    let bookmarks = vec![
        Bookmark {
            name: "My home".into(),
            location: "file:///missing/home%20space".into(),
            ..Default::default()
        },
        Bookmark::new("Also my home", home.to_str().unwrap()).unwrap(),
    ];
    let rows = Places::default().project(&bookmarks, home, Path::new("/"));
    assert_eq!(
        rows.iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>(),
        [
            "My home",
            "Also my home",
            "Desktop",
            "Downloads",
            "Launch directory"
        ]
    );
    assert_eq!(rows[0].location, rows[1].location);
    assert_eq!(rows.last().unwrap().location, Location::local("/"));
    assert_eq!(
        rows.last().unwrap().builtin,
        Some(BuiltinPlace::LaunchDirectory)
    );
    let rows = Places::default().project(&[], Path::new("/"), Path::new("/"));
    assert_eq!(
        rows.iter()
            .filter(|row| row.location == Location::local("/"))
            .count(),
        1
    );
    assert_eq!(rows[0].label, "Home");
}

#[test]
fn local_path_identity_deduplicates_builtin_separators_without_rewriting_saved_urls() {
    let bookmarks = vec![
        Bookmark::new("Trailing slash", "/home/user/").unwrap(),
        Bookmark::new("Repeated separator", "/home//user").unwrap(),
        Bookmark::new("Remote home", "ssh://offline-host/home/user/").unwrap(),
    ];
    let original = serde_json::to_value(&bookmarks).unwrap();
    let rows = Places::default().project(
        &bookmarks,
        Path::new("/home/user"),
        Path::new("/home//user/"),
    );
    assert_eq!(
        rows.iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>(),
        [
            "Trailing slash",
            "Repeated separator",
            "Remote home",
            "Desktop",
            "Downloads",
            "Root"
        ]
    );
    assert_eq!(serde_json::to_value(&bookmarks).unwrap(), original);
    assert_eq!(bookmarks[0].location, "/home/user/");
    assert_eq!(bookmarks[1].location, "/home//user");
    assert_eq!(
        rows[2].location,
        Location::Ssh {
            host: "offline-host".into(),
            path: "/home/user/".into(),
        }
    );
    let rows = Places::default().project(&[], Path::new("/home/user"), Path::new("/home//user/"));
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].builtin, Some(BuiltinPlace::Home));
    assert!(
        rows.iter()
            .all(|row| row.builtin != Some(BuiltinPlace::LaunchDirectory))
    );
}
