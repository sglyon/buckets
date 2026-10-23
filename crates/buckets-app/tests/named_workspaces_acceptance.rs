//! Named workspace persistence uses isolated local metadata and no providers.
#[allow(dead_code)]
#[path = "../src/named_workspaces.rs"]
mod named_workspaces;

use named_workspaces::{
    Column, NamedWorkspaceStore, PaneLayout, PaneView, SortKey, WorkspacePane, WorkspaceSnapshot,
};
use serde_json::{Value, json};
use std::fs;

fn snapshot() -> WorkspaceSnapshot {
    WorkspaceSnapshot {
        workspace_id: "workspace-test-roots".into(),
        panes: vec![WorkspacePane {
            location: "/tmp/offline project".into(),
            weight: 1.,
            view: PaneView::default(),
        }],
        ..WorkspaceSnapshot::default()
    }
}

#[test]
fn full_provider_identity_and_layout_survive_restart_and_metadata_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspaces.json");
    let mut store = NamedWorkspaceStore::load(&path).unwrap();
    assert!(store.entries().is_empty());
    assert!(!path.exists(), "Reading should not write defaults");
    let locations = [
        "/tmp/offline drive/café 100%".to_owned(),
        "ssh://sglyon@research/srv/data%20space/100%25".into(),
        "s3://research-data/exports/caf%C3%A9%20100%25/?endpoint=https%3A%2F%2Fobjects.example.invalid%3A9000".into(),
        "git:///tmp/repo%20space/%25literal?ref=refs%2Fheads%2Ffeature&path=src%2Fcaf%C3%A9.rs".into(),
        "plugin://offline-provider/docs/hello%20world".into(),
    ];
    let mut value = snapshot();
    value.active = 3;
    value.layout = PaneLayout::Vertical;
    value.sidebar = false;
    value.hidden = true;
    value.panes = locations
        .iter()
        .enumerate()
        .map(|(index, location)| WorkspacePane {
            location: location.clone(),
            weight: (index + 1) as f32,
            view: PaneView {
                sort: SortKey::Modified,
                descending: true,
                directories_first: false,
                columns: vec![Column::Name, Column::Modified],
            },
        })
        .collect();
    let id = store.save("  Research  ", value.clone()).unwrap();
    let source_identity = value.workspace_id.clone();
    value.workspace_id = store.get(&id).unwrap().snapshot.workspace_id.clone();
    assert_ne!(value.workspace_id, source_identity);
    assert_eq!(store.entries()[0].name, "Research");
    drop(store);
    let mut restored = NamedWorkspaceStore::load(&path).unwrap();
    assert_eq!(restored.get(&id).unwrap().snapshot, value);
    restored.rename(&id, " Teaching ").unwrap();
    assert_eq!(restored.get(&id).unwrap().name, "Teaching");
    value.active = 1;
    assert_eq!(restored.save("teaching", value.clone()).unwrap(), id);
    assert_eq!(restored.entries().len(), 1);
    assert_eq!(
        restored.get(&id).unwrap().snapshot.workspace_id,
        value.workspace_id
    );
    restored.remove(&id).unwrap();
    assert!(
        NamedWorkspaceStore::load(&path)
            .unwrap()
            .entries()
            .is_empty()
    );
    assert!(
        !dir.path().join("offline drive").exists(),
        "Metadata operations must not create provider roots"
    );
}

#[test]
fn concurrent_edit_and_external_deletion_never_overwrite_another_writer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspaces.json");
    let mut first = NamedWorkspaceStore::load(&path).unwrap();
    let mut stale = NamedWorkspaceStore::load(&path).unwrap();
    let id = first.save("First", snapshot()).unwrap();
    assert!(
        stale
            .save("Second", snapshot())
            .unwrap_err()
            .to_string()
            .contains("externally")
    );
    stale.reload().unwrap();
    first.rename(&id, "Renamed elsewhere").unwrap();
    let original = fs::read(&path).unwrap();
    assert!(stale.remove(&id).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    stale.reload().unwrap();
    fs::remove_file(&path).unwrap();
    assert!(stale.rename(&id, "Must not recreate").is_err());
    assert!(!path.exists());
    assert_eq!(stale.get(&id).unwrap().name, "Renamed elsewhere");
}

#[test]
fn bad_versions_unknown_executable_fields_and_malformed_files_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspaces.json");
    for source in [
        "{bad json".to_owned(),
        json!({"version":99,"workspaces":[]}).to_string(),
        json!({"version":1,"workspaces":[],"commands":["rm -rf /"]}).to_string(),
        json!({"version":1,"workspaces":[{"id":"one","name":"One","snapshot":{"workspace_id":"stable","panes":[{"location":"/tmp","command":"nvim"}]}}]}).to_string(),
    ] {
        fs::write(&path,&source).unwrap();
        assert!(NamedWorkspaceStore::load(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), source);
    }
    fs::write(&path, vec![b' '; 1_048_577]).unwrap();
    assert!(NamedWorkspaceStore::load(&path).is_err());
    assert_eq!(fs::metadata(&path).unwrap().len(), 1_048_577);
}

#[test]
fn credentials_relative_paths_and_encoded_controls_cannot_enter_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspaces.json");
    let mut store = NamedWorkspaceStore::load(&path).unwrap();
    for location in [
        "relative/path",
        "~/project",
        "ssh://user:secret@host/srv",
        "ssh://host/srv?token=secret",
        "s3://user:secret@bucket/key",
        "s3://bucket/key?endpoint=https%3A%2F%2Fuser%3Asecret%40objects.invalid",
        "s3://bucket/key?endpoint=https%3A%2F%2Fobjects.invalid%3Ftoken%3Dsecret",
        "s3://bucket/key?endpoint=https%3A%2F%2Fobjects.invalid%23secret",
        "s3://bucket/key?token=secret",
        "s3://bucket/key?endpoint=https%3A%2F%2Fa.invalid&endpoint=https%3A%2F%2Fb.invalid",
        "plugin://user:secret@provider/root",
        "ssh://host/%0aroot",
        "git:///tmp/repo?ref=hello%0Aworld",
    ] {
        let mut value = snapshot();
        value.panes[0].location = location.into();
        assert!(store.save("Private", value).is_err(), "Accepted {location}");
        assert!(!path.exists());
        assert!(store.entries().is_empty());
    }
}

#[test]
fn bounds_duplicate_names_and_invalid_views_leave_the_last_good_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspaces.json");
    let mut store = NamedWorkspaceStore::load(&path).unwrap();
    let first = store.save("First", snapshot()).unwrap();
    let second = store.save("Second", snapshot()).unwrap();
    let good = fs::read(&path).unwrap();
    assert!(store.rename(&second, "FIRST").is_err());
    assert!(store.rename(&first, "\n").is_err());
    assert!(store.rename(&first, &"x".repeat(129)).is_err());
    let mut bad = snapshot();
    bad.active = 1;
    assert!(store.save("Bad", bad).is_err());
    let mut bad = snapshot();
    bad.panes = vec![bad.panes[0].clone(); 7];
    assert!(store.save("Bad", bad).is_err());
    let mut bad = snapshot();
    bad.panes[0].weight = f32::NAN;
    assert!(store.save("Bad", bad).is_err());
    let mut bad = snapshot();
    bad.panes[0].view.columns = vec![Column::Name, Column::Name];
    assert!(store.save("Bad", bad).is_err());
    let mut bad = snapshot();
    bad.panes[0].view.columns = vec![Column::Size];
    assert!(store.save("Bad", bad).is_err());
    assert_eq!(fs::read(&path).unwrap(), good);
    for index in 2..64 {
        store.save(&format!("Saved {index}"), snapshot()).unwrap();
    }
    assert!(store.save("Overflow", snapshot()).is_err());
    assert_eq!(store.entries().len(), 64);
}

#[test]
fn minimal_version_one_defaults_are_safe_and_serialize_no_runtime_payload() {
    let value = json!({"version":1,"workspaces":[{"id":"saved-one","name":"One","snapshot":{"workspace_id":"workspace-stable","panes":[{"location":"/tmp"}]}}]});
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspaces.json");
    fs::write(&path, value.to_string()).unwrap();
    let mut store = NamedWorkspaceStore::load(&path).unwrap();
    let snapshot = store.entries()[0].snapshot.clone();
    assert_eq!(snapshot.layout, PaneLayout::Horizontal);
    assert_eq!(snapshot.panes[0].view, PaneView::default());
    assert_eq!(snapshot.panes[0].weight, 1.);
    store.rename("saved-one", "New name").unwrap();
    let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        saved["workspaces"][0]["snapshot"]["workspace_id"],
        "workspace-stable"
    );
    let source = fs::read_to_string(&path).unwrap();
    for excluded in [
        "command",
        "environment",
        "transcript",
        "prompt",
        "access_token",
    ] {
        assert!(!source.contains(excluded));
    }
}

#[cfg(unix)]
#[test]
fn symlink_data_and_lock_paths_are_refused_without_following_them() {
    use std::os::unix::{
        fs::{PermissionsExt, symlink},
        prelude::MetadataExt,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspaces.json");
    let outside = dir.path().join("outside");
    fs::write(&outside, "keep outside").unwrap();
    symlink(&outside, &path).unwrap();
    assert!(NamedWorkspaceStore::load(&path).is_err());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "keep outside");
    fs::remove_file(&path).unwrap();
    let mut store = NamedWorkspaceStore::load(&path).unwrap();
    symlink(&outside, dir.path().join(".workspaces.json.lock")).unwrap();
    assert!(store.save("Blocked", snapshot()).is_err());
    assert!(!path.exists());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "keep outside");
    fs::remove_file(dir.path().join(".workspaces.json.lock")).unwrap();
    store.save("Safe", snapshot()).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(dir.path().join(".workspaces.json.lock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let inode = fs::metadata(dir.path().join(".workspaces.json.lock"))
        .unwrap()
        .ino();
    store
        .rename(&store.entries()[0].id.clone(), "Again")
        .unwrap();
    assert_eq!(
        fs::metadata(dir.path().join(".workspaces.json.lock"))
            .unwrap()
            .ino(),
        inode
    );
    assert!(!fs::read_dir(dir.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")
    }));
}

#[test]
fn new_names_isolate_notes_namespaces_while_updates_and_rename_retain_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspaces.json");
    let original_notes = dir.path().join("original-session-notes.txt");
    fs::write(&original_notes, "Keep my original notes").unwrap();
    let mut store = NamedWorkspaceStore::load(&path).unwrap();
    let source = snapshot();
    let teaching = store.save("Teaching", source.clone()).unwrap();
    let client = store.save("Client", source.clone()).unwrap();
    let teaching_identity = store.get(&teaching).unwrap().snapshot.workspace_id.clone();
    let client_identity = store.get(&client).unwrap().snapshot.workspace_id.clone();
    assert_ne!(teaching_identity, client_identity);
    assert_ne!(teaching_identity, source.workspace_id);
    assert_ne!(client_identity, source.workspace_id);
    assert_eq!(teaching_identity, format!("workspace-{teaching}"));
    let mut changed = source.clone();
    changed.workspace_id = "workspace-different-active-session".into();
    changed.panes[0].weight = 2.;
    assert_eq!(store.save("teaching", changed).unwrap(), teaching);
    store.rename(&teaching, "School").unwrap();
    drop(store);
    let restored = NamedWorkspaceStore::load(&path).unwrap();
    assert_eq!(
        restored.get(&teaching).unwrap().snapshot.workspace_id,
        teaching_identity
    );
    assert_eq!(
        restored.get(&client).unwrap().snapshot.workspace_id,
        client_identity
    );
    assert_eq!(
        fs::read_to_string(&original_notes).unwrap(),
        "Keep my original notes"
    );
    assert!(!dir.path().join("agents").exists());
}
