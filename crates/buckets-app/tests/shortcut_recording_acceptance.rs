//! Recording transactions against the real settings store; no GPUI window needed.
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
#[allow(dead_code)]
#[path = "../src/shortcut_recorder.rs"]
mod shortcut_recorder;

use gpui::Keystroke;
use settings::{Settings, SettingsStore};
use shortcut_recorder::{Outcome, Recorder};
use std::fs;

fn stroke(key: &str) -> Keystroke {
    Keystroke::parse(key).unwrap()
}
fn press(recorder: &mut Recorder, key: &str) -> Outcome {
    let key = stroke(key);
    let result = recorder.key_down(&key);
    recorder.key_up(&key);
    result
}
fn apply_split(store: &mut SettingsStore, recorder: &Recorder) -> anyhow::Result<()> {
    let chord = recorder
        .chord()
        .ok_or_else(|| anyhow::anyhow!("No recorded combination"))?
        .to_owned();
    store.update(|settings| {
        settings
            .keybindings
            .insert("pane.split".into(), vec![chord]);
    })
}

#[test]
fn capturing_never_persists_until_an_explicit_apply_and_restart_uses_the_new_key() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    let original = fs::read(&path).unwrap();
    let mut recorder = Recorder::new(None);
    assert_eq!(press(&mut recorder, "cmd-y"), Outcome::Updated);
    assert_eq!(recorder.chord(), Some("cmd-y"));
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(
        keybindings::resolved(&store.settings().keybindings, "pane.split"),
        ["cmd-t"]
    );
    assert_eq!(press(&mut recorder, "tab"), Outcome::Updated);
    assert_eq!(recorder.control(), 1);
    assert_eq!(press(&mut recorder, "enter"), Outcome::Apply);
    apply_split(&mut store, &recorder).unwrap();
    let restarted = SettingsStore::load(&path, Settings::default()).unwrap();
    assert_eq!(
        keybindings::resolved(&restarted.settings().keybindings, "pane.split"),
        ["cmd-y"]
    );
}

#[test]
fn application_command_chords_remain_candidates_and_cancel_preserves_exact_saved_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let store = SettingsStore::load(&path, Settings::default()).unwrap();
    let original = fs::read(&path).unwrap();
    for key in ["cmd-q", "cmd-w", "cmd-,", "cmd-+", "cmd-0", "cmd-escape"] {
        let mut recorder = Recorder::new(None);
        assert_eq!(press(&mut recorder, key), Outcome::Updated, "{key}");
        assert_eq!(
            recorder.chord(),
            Some(keybindings::normalize(key).unwrap().as_str())
        );
        assert_eq!(press(&mut recorder, "escape"), Outcome::Cancel);
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(store.settings().keybindings.is_empty());
    }
}

#[test]
fn rejected_recorded_conflict_retains_last_good_map_and_candidate_can_be_corrected() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    store
        .update(|settings| {
            settings
                .keybindings
                .insert("pane.split".into(), vec!["cmd-y".into()]);
        })
        .unwrap();
    let original = fs::read(&path).unwrap();
    let mut recorder = Recorder::new(None);
    press(&mut recorder, "cmd-k");
    press(&mut recorder, "tab");
    assert_eq!(press(&mut recorder, "enter"), Outcome::Apply);
    let error = apply_split(&mut store, &recorder).unwrap_err().to_string();
    assert!(error.contains("pane.split") && error.contains("app.palette"));
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(recorder.chord(), Some("cmd-k"));
    assert_eq!(press(&mut recorder, "cmd-u"), Outcome::Updated);
    assert_eq!(recorder.control(), 0);
    press(&mut recorder, "tab");
    assert_eq!(press(&mut recorder, "enter"), Outcome::Apply);
    apply_split(&mut store, &recorder).unwrap();
    assert_eq!(
        keybindings::resolved(&store.settings().keybindings, "pane.split"),
        ["cmd-u"]
    );
}

#[test]
fn clear_requires_its_explicit_control_and_persists_disabled_instead_of_inherited() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    let mut recorder = Recorder::new(None);
    press(&mut recorder, "cmd-y");
    press(&mut recorder, "tab");
    press(&mut recorder, "tab");
    assert_eq!(recorder.control(), 2);
    assert_eq!(press(&mut recorder, "space"), Outcome::Clear);
    store
        .update(|settings| {
            settings.keybindings.insert("pane.split".into(), vec![]);
        })
        .unwrap();
    let restarted = SettingsStore::load(&path, Settings::default()).unwrap();
    assert!(keybindings::resolved(&restarted.settings().keybindings, "pane.split").is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).unwrap()["keybindings"]
            ["pane.split"],
        serde_json::json!([])
    );
}

#[test]
fn activation_and_held_key_repeats_cannot_apply_before_release_and_modifiers_are_ignored() {
    let enter = stroke("enter");
    let mut recorder = Recorder::new(Some(&enter));
    assert_eq!(recorder.key_down(&enter), Outcome::Ignored);
    assert_eq!(recorder.chord(), None);
    recorder.key_up(&enter);
    assert_eq!(recorder.key_down(&enter), Outcome::Updated);
    assert_eq!(recorder.chord(), Some("enter"));
    recorder.set_control(1);
    assert_eq!(recorder.key_down(&enter), Outcome::Ignored);
    recorder.key_up(&enter);
    assert_eq!(recorder.key_down(&enter), Outcome::Apply);
    recorder.key_up(&enter);
    for modifier in ["shift", "control", "platform", "alt", "function"] {
        let mut event = enter.clone();
        event.key = modifier.into();
        event.modifiers = Default::default();
        event.key_char = None;
        assert_eq!(recorder.key_down(&event), Outcome::Ignored);
        assert_eq!(recorder.chord(), Some("enter"));
    }
    assert!(recorder.held_keys().is_empty());
}

#[test]
fn native_plus_canonicalizes_and_key_up_without_modifiers_releases_the_same_physical_latch() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    let mut recorder = Recorder::new(None);
    let plus = stroke("cmd-+");
    assert_eq!(recorder.key_down(&plus), Outcome::Updated);
    assert_eq!(recorder.chord(), Some("cmd-shift-="));
    assert_eq!(recorder.key_down(&plus), Outcome::Ignored);
    recorder.key_up(&stroke("="));
    assert_eq!(recorder.key_down(&plus), Outcome::Updated);
    recorder.key_up(&stroke("="));
    press(&mut recorder, "tab");
    assert_eq!(press(&mut recorder, "enter"), Outcome::Apply);
    let chord = recorder.chord().unwrap().to_owned();
    store
        .update(|settings| {
            settings
                .keybindings
                .insert("font.increase".into(), vec![chord]);
        })
        .unwrap();
    let restarted = SettingsStore::load(&path, Settings::default()).unwrap();
    assert_eq!(
        keybindings::resolved(&restarted.settings().keybindings, "font.increase"),
        [keybindings::normalize("cmd-shift-=").unwrap()]
    );
}
