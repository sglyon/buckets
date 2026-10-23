//! Appearance validation and persistence without a native window.
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

use appearance::{Appearance, COLOR_TOKENS};
use serde_json::{Value, json};
use settings::{Settings, SettingsStore};
use std::fs;

#[test]
fn partial_overrides_inherit_every_other_token_and_font() {
    let settings = Settings::parse(
        r##"{"appearance":{"colors":{"accent":"#A1b2C3","background":"#000000"}}}"##,
    )
    .unwrap();
    assert_eq!(settings.appearance.font_family, "JetBrains Mono");
    assert_eq!(settings.appearance.terminal_font_family, "JetBrains Mono");
    assert_eq!(settings.appearance.color("accent"), 0xa1b2c3);
    assert_eq!(settings.appearance.color("background"), 0);
    for &(key, _, default) in COLOR_TOKENS {
        if key != "accent" && key != "background" {
            assert_eq!(settings.appearance.color(key), default, "{key}");
        }
    }
    assert_eq!(settings.appearance.ansi_palette()[0], 0x15161e);
    assert_eq!(settings.appearance.ansi_palette()[15], 0xc0caf5);
}

#[test]
fn invalid_colors_tokens_fonts_and_palette_lengths_are_rejected() {
    for color in [
        "#12345", "123456", "#1234567", "#gg1234", " #123456", "#12é34",
    ] {
        let source = json!({"appearance":{"colors":{"accent":color}}}).to_string();
        assert!(Settings::parse(&source).is_err(), "accepted {color}");
    }
    for font in [
        "".to_owned(),
        "   ".to_owned(),
        "Mono\nOther".to_owned(),
        "a".repeat(129),
    ] {
        for key in ["font_family", "terminal_font_family"] {
            let source = json!({"appearance":{key:&font}}).to_string();
            assert!(Settings::parse(&source).is_err(), "accepted {key} {font:?}");
        }
    }
    assert!(Settings::parse(r##"{"appearance":{"colors":{"accnet":"#ffffff"}}}"##).is_err());
    for count in [0, 15, 17] {
        let source = json!({"appearance":{"terminal_ansi":vec!["#123456";count]}}).to_string();
        assert!(Settings::parse(&source).is_err());
    }
    let mut palette = vec!["#123456"; 16];
    palette[7] = "invalid";
    let source = json!({"appearance":{"terminal_ansi":palette}}).to_string();
    assert!(
        Settings::parse(&source)
            .unwrap_err()
            .to_string()
            .contains("terminal_ansi[7]")
    );
}

#[test]
fn overrides_and_nested_metadata_survive_unrelated_edits_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let source = json!({
        "appearance":{
            "font_family":"A Future Font",
            "terminal_font_family":"Other Mono",
            "colors":{"accent":"#112233"},
            "terminal_ansi":vec!["#445566";16],
            "future":{"theme_source":"my-theme","versions":[1,2]}
        },
        "future_top":true
    });
    fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    store.update(|settings| settings.font_size = 17.).unwrap();
    let disk: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(disk["appearance"], source["appearance"]);
    assert_eq!(disk["future_top"], true);
    let restored = SettingsStore::load(&path, Settings::default()).unwrap();
    assert_eq!(restored.settings().appearance.color("accent"), 0x112233);
    assert_eq!(
        restored.settings().appearance.ansi_palette(),
        [0x445566; 16]
    );
    assert_eq!(restored.settings().appearance.font_family, "A Future Font");
}

#[test]
fn invalid_theme_save_keeps_both_saved_bytes_and_effective_preferences() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    store
        .update(|settings| {
            settings
                .appearance
                .colors
                .insert("accent".into(), "#123456".into());
        })
        .unwrap();
    let bytes = fs::read(&path).unwrap();
    assert!(
        store
            .update(|settings| {
                settings
                    .appearance
                    .colors
                    .insert("accent".into(), "#oops".into());
            })
            .is_err()
    );
    assert_eq!(store.settings().appearance.color("accent"), 0x123456);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(
        store
            .replace_json(r##"{"appearance":{"colors":{"accent":"red"}}}"##)
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn removing_an_override_restores_default_without_losing_other_preferences() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    store
        .update(|settings| {
            settings.show_hidden = true;
            settings.appearance.font_family = "Other Mono".into();
            settings
                .appearance
                .colors
                .insert("accent".into(), "#123456".into());
            settings
                .appearance
                .colors
                .insert("text".into(), "#abcdef".into());
        })
        .unwrap();
    store
        .update(|settings| {
            settings.appearance.colors.remove("accent");
        })
        .unwrap();
    assert_eq!(
        store.settings().appearance.color("accent"),
        Appearance::default().color("accent")
    );
    assert_eq!(store.settings().appearance.color("text"), 0xabcdef);
    store
        .update(|settings| settings.appearance = Appearance::default())
        .unwrap();
    assert_eq!(store.settings().appearance, Appearance::default());
    assert!(store.settings().show_hidden);
    assert_eq!(
        SettingsStore::load(&path, Settings::default())
            .unwrap()
            .settings(),
        store.settings()
    );
}
