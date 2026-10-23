//! Independent saved-keymap checks against the real settings persistence layer.
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

use keybindings::{Overrides, normalize, resolved, resolved_plugin};
use serde_json::{Value, json};
use settings::{Settings, SettingsStore};
use std::fs;

fn saved(path: &std::path::Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn saved_remap_disable_and_reset_each_replace_defaults_across_restarts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    assert_eq!(
        resolved(&store.settings().keybindings, "pane.split"),
        ["cmd-t"]
    );
    store
        .replace_json(r#"{"keybindings":{"pane.split":["super-y"]},"future":{"keep":"λ"}}"#)
        .unwrap();
    let mut restarted = SettingsStore::load(&path, Settings::default()).unwrap();
    assert_eq!(
        resolved(&restarted.settings().keybindings, "pane.split"),
        ["cmd-y"]
    );
    assert!(
        !resolved(&restarted.settings().keybindings, "pane.split").contains(&"cmd-t".to_string())
    );
    restarted
        .update(|settings| {
            settings.keybindings.insert("pane.split".into(), vec![]);
        })
        .unwrap();
    let mut disabled = SettingsStore::load(&path, Settings::default()).unwrap();
    assert!(resolved(&disabled.settings().keybindings, "pane.split").is_empty());
    assert_eq!(saved(&path)["keybindings"]["pane.split"], json!([]));
    disabled
        .update(|settings| {
            settings.keybindings.remove("pane.split");
        })
        .unwrap();
    let reset = SettingsStore::load(&path, Settings::default()).unwrap();
    assert_eq!(
        resolved(&reset.settings().keybindings, "pane.split"),
        ["cmd-t"]
    );
    assert!(saved(&path)["keybindings"].get("pane.split").is_none());
    assert_eq!(saved(&path)["future"], json!({"keep":"λ"}));
}

#[test]
fn malformed_unsafe_and_conflicting_keymaps_preserve_working_memory_and_exact_disk_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    store
        .replace_json(r#"{"keybindings":{"pane.split":["cmd-y"]},"note":"preserve me"}"#)
        .unwrap();
    let original = fs::read(&path).unwrap();
    for invalid in [
        r#"{"keybindings":{"pane.split":["cmd-k"]}}"#,
        r#"{"keybindings":{"pane.split":["super-y","cmd-y"]}}"#,
        r#"{"keybindings":{"app.settings":["cmd-s"]}}"#,
        r#"{"keybindings":{"pane.split":["ctrl-t"]}}"#,
        r#"{"keybindings":{"pane.split":["y"]}}"#,
        r#"{"keybindings":{"file.rename":["tab"]}}"#,
        r#"{"keybindings":{"pane.split":["cmd-not-a-real-key"]}}"#,
        r#"{"keybindings":{"pane.split":["cmd-k cmd-y"]}}"#,
        r#"{"keybindings":{"pane.split":"cmd-y"}}"#,
        r#"{"keybindings":{"pane.split":null}}"#,
        r#"{"keybindings":[]}"#,
        r#"{"keybindings":{"pane.typo":[]}}"#,
        r#"{"keybindings": invalid}"#,
    ] {
        assert!(store.replace_json(invalid).is_err(), "accepted {invalid}");
        assert_eq!(
            fs::read(&path).unwrap(),
            original,
            "changed saved bytes after {invalid}"
        );
        assert_eq!(
            resolved(&store.settings().keybindings, "pane.split"),
            ["cmd-y"]
        );
    }
    assert!(
        store
            .update(|settings| {
                settings
                    .keybindings
                    .insert("pane.split".into(), vec!["cmd-k".into()]);
            })
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(
        resolved(&store.settings().keybindings, "pane.split"),
        ["cmd-y"]
    );
}

#[test]
fn external_keymap_edits_are_not_clobbered_and_invalid_reload_retains_last_good_map() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut first = SettingsStore::load(&path, Settings::default()).unwrap();
    let mut stale = SettingsStore::load(&path, Settings::default()).unwrap();
    first
        .replace_json(r#"{"keybindings":{"pane.split":["cmd-y"]},"external":true}"#)
        .unwrap();
    let external = fs::read(&path).unwrap();
    assert!(
        stale
            .replace_json(r#"{"keybindings":{"pane.split":[]}}"#)
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), external);
    stale.reload().unwrap();
    assert_eq!(
        resolved(&stale.settings().keybindings, "pane.split"),
        ["cmd-y"]
    );
    let invalid = r#"{"keybindings":{"pane.split":["cmd-k"]},"external":"bad edit"}"#;
    fs::write(&path, invalid).unwrap();
    assert!(stale.reload().is_err());
    assert_eq!(
        resolved(&stale.settings().keybindings, "pane.split"),
        ["cmd-y"]
    );
    assert!(stale.update(|settings| settings.sidebar = false).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
}

#[test]
fn inactive_plugin_bindings_and_unknown_settings_survive_core_remap_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let id = keybindings::plugin_id("offline-provider", "open-editor");
    let source = json!({"keybindings":{id.clone():["ctrl-e"]},"plugins":{"offline-provider":{"enabled":false,"config":{"model":"future"}}},"unrecognized":{"retain":[1,2]}});
    fs::write(&path, serde_json::to_vec_pretty(&source).unwrap()).unwrap();
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    store
        .update(|settings| {
            settings
                .keybindings
                .insert("pane.split".into(), vec!["cmd-y".into()]);
        })
        .unwrap();
    let restored = SettingsStore::load(&path, Settings::default()).unwrap();
    assert_eq!(
        resolved_plugin(&restored.settings().keybindings, &id, &["ctrl-g".into()]),
        ["ctrl-e"]
    );
    assert_eq!(saved(&path)["plugins"], source["plugins"]);
    assert_eq!(saved(&path)["unrecognized"], source["unrecognized"]);
    store
        .update(|settings| {
            settings.keybindings.insert(id.clone(), vec![]);
        })
        .unwrap();
    let disabled = SettingsStore::load(&path, Settings::default()).unwrap();
    assert!(resolved_plugin(&disabled.settings().keybindings, &id, &["ctrl-g".into()]).is_empty());
    store
        .update(|settings| {
            settings.keybindings.remove(&id);
        })
        .unwrap();
    let inherited = SettingsStore::load(&path, Settings::default()).unwrap();
    assert_eq!(
        resolved_plugin(&inherited.settings().keybindings, &id, &["ctrl-g".into()]),
        ["ctrl-g"]
    );
}

#[test]
fn alias_collisions_are_rejected_but_control_command_and_shifted_keys_stay_distinct() {
    assert_eq!(
        normalize("super-Y").unwrap(),
        normalize("cmd-shift-y").unwrap()
    );
    assert_ne!(normalize("cmd-y").unwrap(), normalize("ctrl-y").unwrap());
    assert_ne!(normalize("cmd-y").unwrap(), normalize("cmd-Y").unwrap());
    let distinct = Overrides::from([
        ("pane.split".into(), vec!["cmd-y".into()]),
        ("file.rename".into(), vec!["ctrl-y".into()]),
        ("file.duplicate".into(), vec!["cmd-Y".into()]),
    ]);
    keybindings::validate(&distinct).unwrap();
    let collision = Overrides::from([
        ("pane.split".into(), vec!["super-Y".into()]),
        ("file.rename".into(), vec!["cmd-shift-y".into()]),
    ]);
    assert!(keybindings::validate(&collision).is_err());
    let duplicate = Overrides::from([
        (
            "pane.split".into(),
            vec!["cmd-+".into(), "cmd-shift-=".into()],
        ),
        ("font.increase".into(), vec![]),
    ]);
    assert!(keybindings::validate(&duplicate).is_err());
}

#[test]
fn plugin_and_core_conflicts_require_an_explicit_disabled_binding_before_reuse() {
    let id = keybindings::plugin_id("not-installed", "terminal");
    let mut overrides = Overrides::from([(id.clone(), vec!["cmd-t".into()])]);
    assert!(keybindings::validate(&overrides).is_err());
    overrides.insert("pane.split".into(), vec![]);
    keybindings::validate(&overrides).unwrap();
    assert_eq!(resolved_plugin(&overrides, &id, &[]), ["cmd-t"]);
    overrides.insert(
        keybindings::plugin_id("another-plugin", "terminal"),
        vec!["super-t".into()],
    );
    assert!(keybindings::validate(&overrides).is_err());
}
