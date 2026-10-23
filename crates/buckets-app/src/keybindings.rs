//! Application command bindings. Native text editing and terminal input remain
//! owned by their components; overrides only change the scopes declared here.
use anyhow::{Result, bail, ensure};
use gpui::Keystroke;
use std::collections::{BTreeMap, BTreeSet};

pub type Overrides = BTreeMap<String, Vec<String>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Files,
    Workspace,
    Global,
}

#[derive(Clone, Copy, Debug)]
pub struct ActionSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub scope: Scope,
    pub defaults: &'static [&'static str],
}

macro_rules! action {
    ($id:literal, $title:literal, $scope:ident, [$($key:literal),*]) => {
        ActionSpec { id: $id, title: $title, scope: Scope::$scope, defaults: &[$($key),*] }
    };
}

pub static ACTIONS: &[ActionSpec] = &[
    action!(
        "view.operations",
        "Show operations and recovery",
        Global,
        ["cmd-shift-j"]
    ),
    action!("workspace.save", "Save named workspace", Workspace, []),
    action!("workspace.reload", "Reload named workspaces", Workspace, []),
    action!("pane.grow", "Grow active pane", Workspace, ["cmd-alt-]"]),
    action!(
        "pane.shrink",
        "Shrink active pane",
        Workspace,
        ["cmd-alt-["]
    ),
    action!("pane.equalize", "Equalize pane sizes", Workspace, []),
    action!("pane.sort_name", "Sort pane by name", Files, []),
    action!("pane.sort_size", "Sort pane by size", Files, []),
    action!(
        "pane.sort_modified",
        "Sort pane by modified date",
        Files,
        []
    ),
    action!("pane.sort_type", "Sort pane by type", Files, []),
    action!(
        "pane.sort_reverse",
        "Reverse pane sort direction",
        Files,
        []
    ),
    action!(
        "pane.directories_first",
        "Toggle directories first",
        Files,
        []
    ),
    action!("pane.column_size", "Toggle Size column", Files, []),
    action!("pane.column_modified", "Toggle Modified column", Files, []),
    action!("pane.column_type", "Toggle Type column", Files, []),
    action!(
        "pane.layout_horizontal",
        "Arrange panes side by side",
        Workspace,
        []
    ),
    action!(
        "pane.layout_vertical",
        "Arrange panes top to bottom",
        Workspace,
        []
    ),
    action!("pane.split", "Split pane", Workspace, ["cmd-t"]),
    action!(
        "pane.close",
        "Close current pane or surface",
        Global,
        ["cmd-w"]
    ),
    action!("pane.next", "Focus next pane", Workspace, ["cmd-alt-right"]),
    action!(
        "pane.previous",
        "Focus previous pane",
        Workspace,
        ["cmd-alt-left"]
    ),
    action!("app.settings", "Open Settings", Global, ["cmd-,"]),
    action!("app.quit", "Quit Buckets", Global, ["cmd-q"]),
    action!("app.palette", "Open command palette", Workspace, ["cmd-k"]),
    action!("app.command_line", "Open command line", Files, ["shift-;"]),
    action!("app.help", "Keyboard reference", Files, ["?", "f1"]),
    action!(
        "font.increase",
        "Increase text size",
        Global,
        ["cmd-=", "cmd-shift-="]
    ),
    action!("font.decrease", "Decrease text size", Global, ["cmd--"]),
    action!("font.reset", "Reset text size", Global, ["cmd-0"]),
    action!(
        "location.open",
        "Go to location / connect",
        Files,
        ["cmd-l"]
    ),
    action!("location.back", "Go back", Files, ["cmd-["]),
    action!("location.forward", "Go forward", Files, ["cmd-]"]),
    action!("location.parent", "Go to parent", Files, []),
    action!(
        "location.home",
        "Go to home directory",
        Files,
        ["~", "cmd-shift-h"]
    ),
    action!("location.desktop", "Go to Desktop", Files, ["cmd-shift-d"]),
    action!(
        "location.downloads",
        "Go to Downloads",
        Files,
        ["cmd-alt-l"]
    ),
    action!("location.workspace", "Go to launch directory", Files, []),
    action!("location.root", "Go to filesystem root", Files, []),
    action!("file.open", "Open selected file", Files, ["enter"]),
    action!("file.rename", "Rename selected file", Files, ["f2"]),
    action!("file.new", "New file", Files, ["cmd-n"]),
    action!("file.new_folder", "New folder", Files, ["cmd-shift-n"]),
    action!("file.copy", "Copy selection", Files, ["cmd-c"]),
    action!("file.cut", "Cut selection", Files, ["cmd-x"]),
    action!("file.paste", "Paste into this pane", Files, ["cmd-v"]),
    action!("file.copy_to", "Copy to directory", Files, ["f5"]),
    action!("file.move_to", "Move to directory", Files, ["f6"]),
    action!("file.duplicate", "Duplicate selection", Files, ["cmd-d"]),
    action!(
        "file.trash",
        "Move selection to Trash",
        Files,
        ["cmd-backspace", "delete"]
    ),
    action!("file.refresh", "Refresh directory", Files, ["cmd-r"]),
    action!("file.select_all", "Select all files", Files, ["cmd-a"]),
    action!("file.filter", "Filter names / search files", Files, ["/"]),
    action!(
        "file.quick_look",
        "Quick Look selected file",
        Files,
        ["space"]
    ),
    action!("file.toggle_hidden", "Toggle hidden files", Files, ["."]),
    action!("view.preview", "Toggle file preview", Files, ["cmd-p"]),
    action!("view.sidebar", "Toggle Places sidebar", Files, ["cmd-b"]),
    action!("bookmark.add", "Save to Places", Files, ["cmd-shift-b"]),
    action!("bookmark.open", "Open Places", Files, []),
    action!("plugin.directory", "Open plugins folder", Files, []),
    action!("plugin.reload", "Reload plugins", Files, []),
];

pub fn spec(id: &str) -> Option<&'static ActionSpec> {
    ACTIONS.iter().find(|action| action.id == id)
}

/// Normalize with GPUI's parser, then constrain its permissive named-key parser
/// to actual keys. A literal plus is the shifted equals key, including in
/// conflict detection; Command and Control deliberately remain distinct.
pub fn normalize(source: &str) -> Result<String> {
    ensure!(
        !source.is_empty() && source.len() <= 128 && !source.chars().any(char::is_whitespace),
        "Use one keystroke such as cmd-t; key sequences and whitespace are not supported"
    );
    ensure!(
        !source.contains("->"),
        "Synthetic key characters are not supported"
    );
    let mut key = Keystroke::parse(source).map_err(|error| anyhow::anyhow!("{error}"))?;
    ensure!(
        valid_key(&key.key),
        "Unknown key {}; use a character, space, enter, tab, an arrow or a function key",
        key.key
    );
    if key.key == "+" {
        key.key = "=".into();
        key.modifiers.shift = true;
    }
    if key.key == ":" {
        key.key = ";".into();
        key.modifiers.shift = true;
    }
    Ok(key.unparse())
}

fn valid_key(key: &str) -> bool {
    key.chars().count() == 1 && !key.chars().any(char::is_control)
        || matches!(
            key,
            "space"
                | "enter"
                | "tab"
                | "escape"
                | "backspace"
                | "delete"
                | "left"
                | "right"
                | "up"
                | "down"
                | "home"
                | "end"
                | "pageup"
                | "pagedown"
                | "insert"
        )
        || key.strip_prefix('f').is_some_and(|n| {
            n.parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)) && !n.starts_with('0')
        })
}

fn function_key(key: &str) -> bool {
    key.strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=24).contains(&n))
}

fn valid_plugin_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("plugin.") else {
        return false;
    };
    rest.split_once('/').is_some_and(|(plugin, command)| {
        [plugin, command].iter().all(|part| {
            !part.is_empty()
                && part.len() <= 128
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        })
    })
}

/// Keys whose non-command behavior is deliberately fixed in file navigation,
/// and macOS commands whose handlers are not application command overrides.
fn reserved(key: &str) -> bool {
    matches!(
        key,
        "escape"
            | "tab"
            | "shift-tab"
            | "j"
            | "k"
            | "h"
            | "l"
            | "left"
            | "right"
            | "up"
            | "down"
            | "shift-up"
            | "shift-down"
            | "pageup"
            | "pagedown"
            | "home"
            | "end"
            | "g"
            | "shift-g"
            | "x"
            | "backspace"
    ) || [
        "cmd-h",
        "cmd-alt-h",
        "cmd-m",
        "alt-up",
        "alt-down",
        "alt-j",
        "alt-k",
        "alt-pageup",
        "alt-pagedown",
    ]
    .iter()
    .any(|source| normalize(source).ok().as_deref() == Some(key))
}

fn validate_scope(key: &str, scope: Scope) -> Result<()> {
    ensure!(
        !reserved(key),
        "{key} is reserved for navigation or a native system command"
    );
    let parsed = Keystroke::parse(key).map_err(|error| anyhow::anyhow!("{error}"))?;
    if scope != Scope::Files {
        ensure!(
            parsed.modifiers.platform || function_key(&parsed.key),
            "Workspace and global shortcuts need Command or a function key to preserve terminal input"
        );
        ensure!(
            !["cmd-a", "cmd-c", "cmd-x", "cmd-v", "cmd-z", "cmd-shift-z"]
                .iter()
                .any(|source| normalize(source).ok().as_deref() == Some(key)),
            "{key} is reserved for native text editing and terminal clipboard commands"
        );
    }
    if scope == Scope::Global {
        ensure!(
            Some(key) != normalize("cmd-s").ok().as_deref(),
            "cmd-s is reserved for saving the settings JSON editor"
        );
    }
    Ok(())
}

/// Returns normalized effective alternatives; unknown IDs have no defaults.
pub fn resolved(overrides: &Overrides, id: &str) -> Vec<String> {
    let values = overrides.get(id).cloned().unwrap_or_else(|| {
        spec(id)
            .map(|action| action.defaults.iter().map(|s| (*s).to_owned()).collect())
            .unwrap_or_default()
    });
    values
        .iter()
        .filter_map(|key| normalize(key).ok())
        .collect()
}

pub fn plugin_id(plugin: &str, command: &str) -> String {
    format!("plugin.{plugin}/{command}")
}

pub fn resolved_plugin(overrides: &Overrides, id: &str, defaults: &[String]) -> Vec<String> {
    overrides.get(id).map_or_else(
        || {
            defaults
                .iter()
                .filter_map(|key| validate_plugin_shortcut(key).ok())
                .collect()
        },
        |_| resolved(overrides, id),
    )
}

pub fn validate_plugin_shortcut(source: &str) -> Result<String> {
    let key = normalize(source)?;
    validate_scope(&key, Scope::Files)?;
    Ok(key)
}

/// Validate effective bindings, rather than just overrides: assigning a key
/// already used by a default requires clearing or changing that action too.
/// Missing plugins retain their overrides and receive the same conflict checks.
pub fn validate(overrides: &Overrides) -> Result<()> {
    ensure!(
        overrides.len() <= 1024,
        "There are more than 1024 keybinding overrides"
    );
    for (id, keys) in overrides {
        ensure!(id.len() <= 264, "Keybinding action id exceeds 264 bytes");
        ensure!(
            spec(id).is_some() || valid_plugin_id(id),
            "Unknown shortcut action {id}"
        );
        ensure!(
            keys.len() <= 8,
            "{id} has more than 8 shortcut alternatives"
        );
        let scope = spec(id).map_or(Scope::Files, |action| action.scope);
        for key in keys {
            validate_scope(&normalize(key)?, scope)?;
        }
    }
    let mut assigned = BTreeMap::<String, String>::new();
    let ids = ACTIONS.iter().map(|action| action.id).chain(
        overrides
            .keys()
            .filter(|id| spec(id).is_none())
            .map(String::as_str),
    );
    for id in ids {
        let mut local = BTreeSet::new();
        for key in resolved(overrides, id) {
            ensure!(local.insert(key.clone()), "{id} repeats shortcut {key}");
            if let Some(previous) = assigned.insert(key.clone(), id.to_owned()) {
                bail!(
                    "Shortcut {key} is assigned to both {previous} and {id}; clear or change one binding"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_command_control_stay_distinct() {
        validate(&Overrides::new()).unwrap();
        assert_eq!(
            normalize("alt-super-T").unwrap(),
            normalize("cmd-alt-shift-t").unwrap()
        );
        assert_ne!(normalize("ctrl-t").unwrap(), normalize("cmd-t").unwrap());
        assert_eq!(
            normalize("cmd-+").unwrap(),
            normalize("cmd-shift-=").unwrap()
        );
        assert_eq!(
            normalize("cmd--").unwrap(),
            Keystroke::parse("cmd--").unwrap().unparse()
        );
    }

    #[test]
    fn remapping_disabling_and_resetting_replace_defaults() {
        let mut map = Overrides::new();
        assert_eq!(resolved(&map, "pane.split"), ["cmd-t"]);
        map.insert("pane.split".into(), vec!["cmd-\\".into()]);
        validate(&map).unwrap();
        assert_eq!(resolved(&map, "pane.split"), ["cmd-\\"]);
        map.insert("pane.split".into(), vec![]);
        validate(&map).unwrap();
        assert!(resolved(&map, "pane.split").is_empty());
        map.remove("pane.split");
        assert_eq!(resolved(&map, "pane.split"), ["cmd-t"]);
    }

    #[test]
    fn conflicts_include_inherited_defaults_and_aliases() {
        let mut map = Overrides::new();
        map.insert("pane.split".into(), vec!["cmd-k".into()]);
        assert!(
            validate(&map)
                .unwrap_err()
                .to_string()
                .contains("app.palette")
        );
        map.insert("app.palette".into(), vec![]);
        validate(&map).unwrap();
        map.insert("font.reset".into(), vec!["cmd-+".into()]);
        assert!(
            validate(&map)
                .unwrap_err()
                .to_string()
                .contains("font.increase")
        );
    }

    #[test]
    fn rejects_malformed_unsafe_and_reserved_shortcuts() {
        for key in [
            "",
            "cmd-k cmd-t",
            "cmd-not-a-key",
            "cmd-banana",
            "cmd",
            "cmd-k->x",
            "ctrl-t",
            "t",
            "cmd-c",
            "cmd-h",
        ] {
            let map = Overrides::from([("pane.split".into(), vec![key.into()])]);
            assert!(validate(&map).is_err(), "accepted {key}");
        }
        for id in ["app.settings", "pane.close"] {
            assert!(validate(&Overrides::from([(id.into(), vec!["cmd-s".into()])])).is_err());
        }
        assert!(
            validate(&Overrides::from([(
                "file.rename".into(),
                vec!["tab".into()]
            )]))
            .is_err()
        );
        assert!(validate(&Overrides::from([("pane.typo".into(), vec![])])).is_err());
        assert!(
            validate(&Overrides::from([(
                "file.rename".into(),
                vec!["alt-up".into()]
            )]))
            .is_err()
        );
    }

    #[test]
    fn plugin_overrides_survive_absence_and_cannot_conflict_with_core() {
        let dotted_plugin = plugin_id("org.foo", "bar");
        let dotted_command = plugin_id("org", "foo.bar");
        assert_ne!(dotted_plugin, dotted_command);
        validate(&Overrides::from([
            (dotted_plugin, vec!["ctrl-e".into()]),
            (dotted_command, vec!["ctrl-g".into()]),
        ]))
        .unwrap();
        assert!(validate(&Overrides::from([("plugin.org.foo.bar".into(), vec![])])).is_err());
        let id = plugin_id("native-tools", "nvim");
        let mut map = Overrides::from([(id.clone(), vec!["ctrl-e".into()])]);
        validate(&map).unwrap();
        assert_eq!(resolved_plugin(&map, &id, &["ctrl-g".into()]), ["ctrl-e"]);
        map.insert(id.clone(), vec![]);
        assert!(resolved_plugin(&map, &id, &["ctrl-g".into()]).is_empty());
        map.insert(id, vec!["cmd-t".into()]);
        assert!(validate(&map).is_err());
    }
}
