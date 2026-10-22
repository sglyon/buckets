//! Independent saved-preference to runtime appearance contract checks.
#[allow(dead_code)]
#[path = "../src/appearance.rs"]
mod appearance;
#[allow(dead_code)]
#[path = "../src/keybindings.rs"]
mod keybindings;
#[allow(dead_code)]
#[path = "../src/settings.rs"]
mod settings;
#[allow(dead_code)]
#[path = "../src/theme.rs"]
mod theme;

use serde_json::json;
use settings::{Settings, SettingsStore};
use std::fs;
use theme::{BUNDLED_FONT, RuntimeTheme};

#[test]
fn unavailable_fonts_fall_back_without_rewriting_saved_requests_and_resolve_when_installed() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    store
        .replace_json(
            &json!({"appearance":{
                "font_family":"Future UI Face",
                "terminal_font_family":"Future Terminal Face",
                "colors":{"accent":"#A1b2C3"},
                "future":{"retain":"λ"}
            }})
            .to_string(),
        )
        .unwrap();
    let bytes = fs::read(&path).unwrap();
    let restarted = SettingsStore::load(&path, Settings::default()).unwrap();
    let fallback = RuntimeTheme::resolve(&restarted.settings().appearance, &[]);
    assert_eq!(fallback.ui_font.as_ref(), BUNDLED_FONT);
    assert_eq!(fallback.terminal.font_family, BUNDLED_FONT);
    let notice = fallback.font_notice.unwrap();
    assert!(notice.contains("UI font 'Future UI Face'"));
    assert!(notice.contains("Terminal font 'Future Terminal Face'"));
    assert_eq!(fallback.palette.accent, 0xa1b2c3);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(
        restarted.settings().appearance.font_family,
        "Future UI Face"
    );
    assert_eq!(
        restarted.settings().appearance.terminal_font_family,
        "Future Terminal Face"
    );
    assert_eq!(
        restarted.settings().appearance.extra["future"],
        json!({"retain":"λ"})
    );

    // A previously unavailable preference takes effect on a later machine/run.
    let installed = vec![
        "future ui face".to_owned(),
        "FUTURE TERMINAL FACE".to_owned(),
    ];
    let available = RuntimeTheme::resolve(&restarted.settings().appearance, &installed);
    assert_eq!(available.ui_font.as_ref(), "future ui face");
    assert_eq!(available.terminal.font_family, "FUTURE TERMINAL FACE");
    assert!(available.font_notice.is_none());
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert!(
        RuntimeTheme::resolve(&Default::default(), &[])
            .font_notice
            .is_none()
    );
}

#[test]
fn saved_host_palette_keeps_browser_row_and_terminal_caret_semantics_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut store = SettingsStore::load(&path, Settings::default()).unwrap();
    let ansi: Vec<String> = (0..16)
        .map(|index| format!("#{:06x}", 0x100000 + index * 0x010203))
        .collect();
    store
        .replace_json(
            &json!({"appearance":{
                "font_family":" Menlo ",
                "terminal_font_family":"menlo",
                "colors":{
                    "background":"#10151f", "text":"#ddeeff", "accent":"#e5a50a",
                    "cursor":"#203040", "border":"#607080", "surface":"#112233", "danger":"#ff5500"
                },
                "terminal_ansi":ansi
            }})
            .to_string(),
        )
        .unwrap();
    let restarted = SettingsStore::load(&path, Settings::default()).unwrap();
    let runtime = RuntimeTheme::resolve(&restarted.settings().appearance, &["Menlo".into()]);
    assert_eq!(runtime.ui_font.as_ref(), "Menlo");
    assert_eq!(runtime.terminal.font_family, "Menlo");
    assert!(runtime.font_notice.is_none());
    assert_eq!(runtime.palette.cursor, 0x203040);
    assert_eq!(runtime.terminal.cursor, 0xe5a50a);
    assert_ne!(runtime.palette.cursor, runtime.terminal.cursor);
    assert_eq!(runtime.terminal.background, 0x10151f);
    assert_eq!(runtime.terminal.foreground, 0xddeeff);
    assert_eq!(runtime.terminal.selection_background, 0x607080);
    assert_eq!(runtime.terminal.surface, 0x112233);
    assert_eq!(runtime.terminal.danger, 0xff5500);
    for (index, color) in runtime.terminal.ansi.iter().enumerate() {
        assert_eq!(*color, 0x100000 + index as u32 * 0x010203);
    }
    let default = RuntimeTheme::resolve(&Default::default(), &[]);
    assert_eq!(default.terminal.font_family, BUNDLED_FONT);
    assert_ne!(default.terminal.ansi, runtime.terminal.ansi);
    assert_eq!(
        runtime.terminal.cursor, 0xe5a50a,
        "resolving another theme changed an existing snapshot"
    );
}
