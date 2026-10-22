//! Input policy for the inline shortcut recorder. Persistence and scope/conflict
//! validation stay with Settings; key-up latches prevent held keys from applying.
use gpui::Keystroke;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ignored,
    Updated,
    Apply,
    Clear,
    Cancel,
}

pub struct Recorder {
    chord: Option<String>,
    control: usize,
    error: Option<String>,
    held: BTreeSet<String>,
}
impl Recorder {
    pub fn new(activation: Option<&Keystroke>) -> Self {
        Self {
            chord: None,
            control: 0,
            error: None,
            held: activation.map(key_identity).into_iter().collect(),
        }
    }
    pub fn chord(&self) -> Option<&str> {
        self.chord.as_deref()
    }
    pub fn control(&self) -> usize {
        self.control
    }
    pub fn set_control(&mut self, control: usize) {
        self.control = control.min(3);
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn held_keys(&self) -> BTreeSet<String> {
        self.held.clone()
    }
    pub fn key_up(&mut self, stroke: &Keystroke) {
        self.held.remove(&key_identity(stroke));
    }
    pub fn key_down(&mut self, stroke: &Keystroke) -> Outcome {
        if matches!(
            stroke.key.as_str(),
            "shift" | "control" | "platform" | "alt" | "function" | "fn" | "cmd" | "ctrl" | "super"
        ) {
            return Outcome::Ignored;
        }
        if !self.held.insert(key_identity(stroke)) {
            return Outcome::Ignored;
        }
        let modifiers = stroke.modifiers;
        if stroke.key == "escape"
            && !modifiers.platform
            && !modifiers.control
            && !modifiers.alt
            && !modifiers.shift
            && !modifiers.function
        {
            return Outcome::Cancel;
        }
        if stroke.key == "tab" && !modifiers.platform && !modifiers.control && !modifiers.alt {
            self.control = (self.control as isize + if modifiers.shift { -1 } else { 1 })
                .rem_euclid(4) as usize;
            return Outcome::Updated;
        }
        if self.control > 0
            && matches!(stroke.key.as_str(), "enter" | "space")
            && !modifiers.platform
            && !modifiers.control
            && !modifiers.alt
            && !modifiers.shift
        {
            return match self.control {
                1 => Outcome::Apply,
                2 => Outcome::Clear,
                _ => Outcome::Cancel,
            };
        }
        self.control = 0;
        match crate::keybindings::normalize(&stroke.unparse()) {
            Ok(chord) => {
                self.chord = Some(chord);
                self.error = None;
            }
            Err(error) => {
                self.chord = None;
                self.error = Some(error.to_string());
            }
        }
        Outcome::Updated
    }
}

/// Key-up may arrive after Command/Shift was released. Track the physical key
/// independent of modifiers; native plus and equals share the same key latch.
pub fn key_identity(stroke: &Keystroke) -> String {
    if stroke.key == "+" {
        "=".into()
    } else {
        stroke.key.clone()
    }
}
