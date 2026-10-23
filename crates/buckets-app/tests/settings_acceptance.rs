//! Independent persistence tests. This binary crate's store is intentionally
//! exercised without creating a window or touching the user's settings file.
#[allow(dead_code)]
#[path = "../src/appearance.rs"]
mod appearance;
#[allow(dead_code)]
#[path = "../src/keybindings.rs"]
mod keybindings;
#[allow(dead_code)]
#[path = "../src/places.rs"]
mod places;
#[path = "../src/settings.rs"]
mod settings;

use buckets_plugin::{CommandResult, PluginRegistry};
use serde_json::{Value, json};
use settings::{Settings, SettingsStore};
use std::{fs, path::Path};

fn path(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("settings.json")
}
fn disk(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn absent_settings_migrates_legacy_once_and_restart_uses_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = path(&dir);
    let legacy = Settings {
        font_size: 18.,
        show_hidden: true,
        sidebar: false,
        preview: false,
        ..Default::default()
    };
    let store = SettingsStore::load(&path, legacy).unwrap();
    assert_eq!(store.path(), path);
    assert_eq!(store.settings().font_size, 18.);
    assert!(store.settings().show_hidden);
    let bytes = fs::read(&path).unwrap();
    let restored = SettingsStore::load(&path, Settings::default()).unwrap();
    assert_eq!(restored.settings().font_size, 18.);
    assert!(!restored.settings().sidebar);
    assert!(!restored.settings().preview);
    assert_eq!(
        fs::read(path).unwrap(),
        bytes,
        "loading existing settings rewrote bytes"
    );
}

#[test]
fn updating_one_preference_preserves_unknown_global_plugin_and_advanced_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = path(&dir);
    let original = json!({"font_size":14,"future":{"array":[1,"界",null]},"plugins":{"future-plugin":{"enabled":false,"config":{"embedding":{"model":"local","dimensions":384},"roots":["/tmp/a b"]},"future_permission":true}}});
    fs::write(&path, serde_json::to_vec_pretty(&original).unwrap()).unwrap();
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    store.update(|settings| settings.font_size = 19.).unwrap();
    let saved = disk(&path);
    assert_eq!(saved["future"], original["future"]);
    assert_eq!(saved["plugins"], original["plugins"]);
    let restored = SettingsStore::load(&path, Settings::default()).unwrap();
    assert_eq!(restored.settings().font_size, 19.);
    assert_eq!(
        serde_json::from_str::<Value>(&restored.raw_json()).unwrap(),
        saved
    );
}

#[test]
fn malformed_file_is_never_implicitly_replaced_and_can_be_explicitly_repaired() {
    let dir = tempfile::tempdir().unwrap();
    let path = path(&dir);
    let malformed = b"{\n  \"font_size\": 18, broken user edit\n";
    fs::write(&path, malformed).unwrap();
    assert!(SettingsStore::load(&path, Settings::default()).is_err());
    let (mut store, error) = SettingsStore::recover(&path, Settings::default()).unwrap();
    assert!(error.is_some(), "malformed settings were silently accepted");
    assert_eq!(fs::read(&path).unwrap(), malformed);
    assert_eq!(
        store.raw_json().as_bytes(),
        malformed,
        "repair editor lost the user's actual text"
    );
    assert!(
        store.update(|settings| settings.font_size = 20.).is_err(),
        "a form control silently replaced malformed JSON"
    );
    assert!(store.replace_json("{invalid}").is_err());
    assert_eq!(fs::read(&path).unwrap(), malformed);
    store
        .replace_json(r#"{"font_size":17,"repair_note":"kept"}"#)
        .unwrap();
    assert_eq!(store.settings().font_size, 17.);
    assert_eq!(disk(&path)["repair_note"], "kept");
}

#[test]
fn simultaneous_stores_cannot_both_commit_and_leak_no_temporary_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = path(&dir);
    let first = SettingsStore::load(&path, Settings::default()).unwrap();
    let second = SettingsStore::load(&path, Settings::default()).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles = [first, second]
        .into_iter()
        .enumerate()
        .map(|(index, mut store)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store
                    .update(|settings| settings.font_size = 18. + index as f32)
                    .is_ok()
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|success| *success)
            .count(),
        1
    );
    assert!([18., 19.].contains(&disk(&path)["font_size"].as_f64().unwrap()));
    let names = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert!(
        names
            .iter()
            .all(|name| name == "settings.json" || name == ".settings.json.lock"),
        "temporary settings files leaked: {names:?}"
    );
}

#[test]
fn killed_lock_holder_cannot_permanently_block_future_settings_saves() {
    if let Some(root) = std::env::var_os("BUCKETS_SETTINGS_LOCK_CHILD") {
        let root = std::path::PathBuf::from(root);
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.join(".settings.json.lock"))
            .unwrap();
        lock.try_lock().unwrap();
        fs::write(root.join("LOCKED"), "ready").unwrap();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = path(&dir);
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    let original = fs::read(&path).unwrap();
    let mut child = Child(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "killed_lock_holder_cannot_permanently_block_future_settings_saves",
                "--nocapture",
            ])
            .env("BUCKETS_SETTINGS_LOCK_CHILD", dir.path())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !dir.path().join("LOCKED").exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        dir.path().join("LOCKED").exists(),
        "lock holder never acquired OS lock"
    );
    assert!(
        store.update(|settings| settings.font_size = 18.).is_err(),
        "save bypassed another process's lock"
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    store.update(|settings| settings.font_size = 18.).unwrap();
    assert_eq!(disk(&path)["font_size"], 18.);
}

#[cfg(unix)]
#[test]
fn editing_symlinked_settings_refuses_to_replace_link_or_target() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("shared.json");
    fs::write(&target, r#"{"font_size":15}"#).unwrap();
    let path = path(&dir);
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    assert!(store.update(|settings| settings.font_size = 16.).is_err());
    assert!(
        fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), r#"{"font_size":15}"#);
}

#[test]
fn invalid_fields_leave_previous_memory_and_disk_intact() {
    let dir = tempfile::tempdir().unwrap();
    let path = path(&dir);
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    let before = fs::read(&path).unwrap();
    for invalid in [
        r#"{"font_size":8}"#,
        r#"{"font_size":25}"#,
        r#"{"font_size":"big"}"#,
        r#"{"show_hidden":"yes"}"#,
        r#"{"plugins":{"p":{"config":[]}}}"#,
        "[]",
        "null",
    ] {
        assert!(store.replace_json(invalid).is_err(), "accepted {invalid}");
        assert_eq!(store.settings().font_size, 13.);
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    assert!(
        store
            .update(|settings| settings.font_size = f32::NAN)
            .is_err()
    );
    assert_eq!(store.settings().font_size, 13.);
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn stale_store_and_external_deletion_cannot_clobber_another_edit() {
    let dir = tempfile::tempdir().unwrap();
    let path = path(&dir);
    let mut first = SettingsStore::load(&path, Settings::default()).unwrap();
    let mut stale = SettingsStore::load(&path, Settings::default()).unwrap();
    first.update(|settings| settings.font_size = 20.).unwrap();
    let externally_saved = fs::read(&path).unwrap();
    assert!(
        stale
            .update(|settings| settings.show_hidden = true)
            .is_err()
    );
    assert!(stale.replace_json(r#"{"font_size":22}"#).is_err());
    assert_eq!(fs::read(&path).unwrap(), externally_saved);
    assert_eq!(stale.settings().font_size, 13.);
    stale.reload().unwrap();
    assert_eq!(stale.settings().font_size, 20.);
    stale
        .update(|settings| settings.show_hidden = true)
        .unwrap();
    assert!(disk(&path)["show_hidden"].as_bool().unwrap());
    fs::remove_file(&path).unwrap();
    assert!(stale.update(|settings| settings.font_size = 21.).is_err());
    assert!(!path.exists(), "external deletion was silently undone");
}

#[test]
fn invalid_external_reload_keeps_last_good_and_requires_reread_before_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = path(&dir);
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    store.update(|settings| settings.font_size = 16.).unwrap();
    let invalid = b"{\"font_size\": false}";
    fs::write(&path, invalid).unwrap();
    assert!(store.reload().is_err());
    assert_eq!(store.settings().font_size, 16.);
    assert!(store.update(|settings| settings.preview = false).is_err());
    assert_eq!(fs::read(&path).unwrap(), invalid);
    fs::write(&path, r#"{"font_size":21,"external":true}"#).unwrap();
    store.reload().unwrap();
    assert_eq!(store.settings().font_size, 21.);
    store.update(|settings| settings.preview = false).unwrap();
    assert_eq!(disk(&path)["external"], true);
}

#[test]
fn saved_file_controls_effective_native_plugin_and_invalid_metadata_refuses_save() {
    let dir = tempfile::tempdir().unwrap();
    let plugin_dir = dir.path().join("plugin");
    fs::create_dir(&plugin_dir).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/native-tools");
    fs::copy(source.join("plugin.py"), plugin_dir.join("plugin.py")).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(source.join("plugin.json")).unwrap()).unwrap();
    manifest["permissions"]["read_roots"] = json!([dir.path()]);
    let original_manifest = serde_json::to_vec_pretty(&manifest).unwrap();
    fs::write(plugin_dir.join("plugin.json"), &original_manifest).unwrap();
    let catalog = PluginRegistry::discover(&plugin_dir).unwrap();
    let path = path(&dir);
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    store.set_plugin_catalog(&catalog);
    store.replace_json(r#"{"plugins":{"native-tools":{"config":{"terminal_target":"embedded","nvim":"/bin/echo"}}}}"#).unwrap();
    let saved = fs::read(&path).unwrap();
    let restarted = SettingsStore::load(&path, Settings::default()).unwrap();
    let effective =
        PluginRegistry::discover_with_settings(&plugin_dir, &restarted.settings().plugins).unwrap();
    let result = effective
        .invoke_command_result(
            "native-tools",
            "open_nvim",
            json!({"location":dir.path(),"selected":[]}),
        )
        .unwrap();
    let CommandResult::Terminal(request) = result else {
        panic!("saved embedded target was not used")
    };
    assert_eq!(request.program, "/bin/echo");
    assert_eq!(request.args, vec!["--"]);
    assert!(
        store
            .replace_json(
                r#"{"plugins":{"native-tools":{"config":{"terminal_target":"unknown"}}}}"#
            )
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), saved);
    assert_eq!(
        fs::read(plugin_dir.join("plugin.json")).unwrap(),
        original_manifest
    );
}
