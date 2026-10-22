use alacritty_terminal::term::TermMode;
use gpui::Keystroke;

/// Text keys must reach GPUI's native input handler; special keys must bypass it.
/// macOS can attach `key_char` even to Return and Tab, which are control sequences.
pub(crate) fn is_text_input(key: &Keystroke) -> bool {
    let mods = key.modifiers;
    !mods.control
        && !mods.alt
        && !mods.platform
        && !matches!(
            key.key.as_str(),
            "enter" | "escape" | "tab" | "backspace" | "delete"
        )
        && (key.key_char.is_some() || key.key.chars().count() == 1 || key.key == "space")
}

/// Encode xterm keys, respecting the application's cursor mode and modifiers.
/// Command shortcuts belong to the workspace and never become terminal input.
pub fn encode_key(key: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let m = key.modifiers;
    if m.platform {
        return None;
    }
    let modifier = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.control);
    let final_code = match key.key.as_str() {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        _ => None,
    };
    if let Some(code) = final_code {
        return Some(
            if modifier > 1 {
                format!("\x1b[1;{modifier}{code}")
            } else if mode.contains(TermMode::APP_CURSOR) {
                format!("\x1bO{code}")
            } else {
                format!("\x1b[{code}")
            }
            .into_bytes(),
        );
    }
    let special = match key.key.as_str() {
        "enter" => Some("\r".into()),
        "escape" => Some("\x1b".into()),
        "backspace" => Some(if m.control { "\x08" } else { "\x7f" }.into()),
        "tab" => Some(if m.shift { "\x1b[Z" } else { "\t" }.into()),
        "insert" => Some(tilde(2, modifier)),
        "delete" => Some(tilde(3, modifier)),
        "pageup" => Some(tilde(5, modifier)),
        "pagedown" => Some(tilde(6, modifier)),
        "f1" | "f2" | "f3" | "f4" => {
            let code = match key.key.as_str() {
                "f1" => 'P',
                "f2" => 'Q',
                "f3" => 'R',
                _ => 'S',
            };
            Some(if modifier == 1 {
                format!("\x1bO{code}")
            } else {
                format!("\x1b[1;{modifier}{code}")
            })
        }
        "f5" => Some(tilde(15, modifier)),
        "f6" => Some(tilde(17, modifier)),
        "f7" => Some(tilde(18, modifier)),
        "f8" => Some(tilde(19, modifier)),
        "f9" => Some(tilde(20, modifier)),
        "f10" => Some(tilde(21, modifier)),
        "f11" => Some(tilde(23, modifier)),
        "f12" => Some(tilde(24, modifier)),
        _ => None,
    };
    let mut bytes = if let Some(text) = special {
        text.into_bytes()
    } else if m.control {
        let ch = if key.key == "space" {
            ' '
        } else {
            key.key.chars().next()?
        };
        let code = match ch {
            'a'..='z' => ch as u8 - b'a' + 1,
            'A'..='Z' => ch as u8 - b'A' + 1,
            ' ' | '@' | '2' => 0,
            '[' | '3' => 27,
            '\\' | '4' => 28,
            ']' | '5' => 29,
            '^' | '6' => 30,
            '_' | '7' | '/' => 31,
            '?' | '8' => 127,
            _ => return None,
        };
        vec![code]
    } else if m.alt {
        // Option is Meta in terminal panes. Use the underlying key rather than
        // macOS Option's composed character (Option-s is ESC s, never ESC ß).
        let text = if key.key == "space" {
            " ".into()
        } else if key.key.chars().count() == 1 {
            if m.shift {
                key.key.to_uppercase()
            } else {
                key.key.clone()
            }
        } else {
            return None;
        };
        text.into_bytes()
    } else {
        key.key_char
            .as_ref()
            .cloned()
            .or_else(|| {
                if key.key == "space" {
                    Some(" ".into())
                } else if key.key.chars().count() == 1 {
                    Some(key.key.clone())
                } else {
                    None
                }
            })?
            .into_bytes()
    };
    if m.alt && key.key != "escape" {
        bytes.insert(0, 27);
    }
    Some(bytes)
}
fn tilde(code: u8, modifier: u8) -> String {
    if modifier == 1 {
        format!("\x1b[{code}~")
    } else {
        format!("\x1b[{code};{modifier}~")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_return_with_a_character_is_still_a_terminal_control() {
        let mut enter = Keystroke::parse("enter").unwrap();
        enter.key_char = Some("\r".into());
        assert!(!is_text_input(&enter));
        assert_eq!(encode_key(&enter, TermMode::empty()), Some(vec![13]));
        let mut tab = Keystroke::parse("tab").unwrap();
        tab.key_char = Some("\t".into());
        assert!(!is_text_input(&tab));
        let mut text = Keystroke::parse("e").unwrap();
        text.key_char = Some("é".into());
        assert!(is_text_input(&text));
    }
    #[test]
    fn tui_navigation_respects_application_mode_and_modifiers() {
        assert_eq!(
            encode_key(&Keystroke::parse("up").unwrap(), TermMode::APP_CURSOR),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            encode_key(
                &Keystroke::parse("ctrl-left").unwrap(),
                TermMode::APP_CURSOR
            ),
            Some(b"\x1b[1;5D".to_vec())
        );
        assert_eq!(
            encode_key(&Keystroke::parse("shift-tab").unwrap(), TermMode::empty()),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            encode_key(&Keystroke::parse("f12").unwrap(), TermMode::empty()),
            Some(b"\x1b[24~".to_vec())
        );
    }
    #[test]
    fn controls_interrupt_and_option_prefix_without_stealing_workspace_commands() {
        assert_eq!(
            encode_key(&Keystroke::parse("ctrl-c").unwrap(), TermMode::empty()),
            Some(vec![3])
        );
        assert_eq!(
            encode_key(&Keystroke::parse("alt-b").unwrap(), TermMode::empty()),
            Some(b"\x1bb".to_vec())
        );
        assert_eq!(
            encode_key(&Keystroke::parse("cmd-w").unwrap(), TermMode::empty()),
            None
        );
    }
    #[test]
    fn text_uses_composed_unicode_not_physical_key_name() {
        let mut key = Keystroke::parse("e").unwrap();
        key.key_char = Some("é界".into());
        assert_eq!(
            encode_key(&key, TermMode::empty()),
            Some("é界".as_bytes().to_vec())
        );
    }
}
