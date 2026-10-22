#[cfg(test)]
use gpui::Keystroke;

pub const DEFAULT_POINTS: f32 = 13.;
const MIN_POINTS: f32 = 9.;
const MAX_POINTS: f32 = 24.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontSize(f32);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Adjustment {
    Increase,
    Decrease,
    Reset,
}

impl FontSize {
    pub fn from_saved(points: Option<f32>) -> Self {
        Self(
            points
                .filter(|value| value.is_finite())
                .unwrap_or(DEFAULT_POINTS)
                .clamp(MIN_POINTS, MAX_POINTS),
        )
    }

    pub fn points(self) -> f32 {
        self.0
    }

    pub fn scale(self) -> f32 {
        self.0 / DEFAULT_POINTS
    }

    pub fn adjust(&mut self, adjustment: Adjustment) {
        self.0 = match adjustment {
            Adjustment::Increase => (self.0 + 1.).min(MAX_POINTS),
            Adjustment::Decrease => (self.0 - 1.).max(MIN_POINTS),
            Adjustment::Reset => DEFAULT_POINTS,
        };
    }
}

/// Both Command-= and Command-Shift-= are conventional zoom-in shortcuts.
/// key_char also covers layouts which produce + from another physical key.
#[cfg(test)]
pub fn shortcut(stroke: &Keystroke) -> Option<Adjustment> {
    let modifiers = stroke.modifiers;
    if !modifiers.platform || modifiers.alt || modifiers.control {
        return None;
    }
    match stroke.key.as_str() {
        "=" | "+" => Some(Adjustment::Increase),
        "-" => Some(Adjustment::Decrease),
        "0" if !modifiers.shift => Some(Adjustment::Reset),
        _ if stroke.key_char.as_deref() == Some("+") => Some(Adjustment::Increase),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_size_has_bounded_steps_and_a_stable_reset() {
        let mut size = FontSize::from_saved(None);
        for _ in 0..100 {
            size.adjust(Adjustment::Increase);
        }
        assert_eq!(size.points(), MAX_POINTS);
        for _ in 0..100 {
            size.adjust(Adjustment::Decrease);
        }
        assert_eq!(size.points(), MIN_POINTS);
        size.adjust(Adjustment::Reset);
        assert_eq!(size.points(), DEFAULT_POINTS);
        assert_eq!(size.scale(), 1.);
    }

    #[test]
    fn restored_values_are_validated_and_old_sessions_keep_the_default() {
        assert_eq!(FontSize::from_saved(Some(18.)).points(), 18.);
        assert_eq!(FontSize::from_saved(Some(100.)).points(), MAX_POINTS);
        assert_eq!(FontSize::from_saved(Some(-10.)).points(), MIN_POINTS);
        assert_eq!(
            FontSize::from_saved(Some(f32::NAN)).points(),
            DEFAULT_POINTS
        );
        let old: super::super::Session =
            serde_json::from_str(r#"{"locations":[],"active":0,"hidden":false}"#).unwrap();
        assert_eq!(FontSize::from_saved(old.font_size).points(), DEFAULT_POINTS);
        let saved = super::super::Session {
            font_size: Some(17.),
            ..Default::default()
        };
        let restored: super::super::Session =
            serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(FontSize::from_saved(restored.font_size).points(), 17.);
    }

    #[test]
    fn zoom_shortcuts_handle_equals_plus_and_input_modifiers() {
        for key in ["cmd-=", "cmd-shift-=", "cmd-+", "cmd-shift-+"] {
            assert_eq!(
                shortcut(&Keystroke::parse(key).unwrap()),
                Some(Adjustment::Increase)
            );
            assert!(!super::super::valid_plugin_shortcut(key));
        }
        assert_eq!(
            shortcut(&Keystroke::parse("cmd--").unwrap()),
            Some(Adjustment::Decrease)
        );
        assert_eq!(
            shortcut(&Keystroke::parse("cmd-0").unwrap()),
            Some(Adjustment::Reset)
        );
        for key in ["-", "=", "ctrl-=", "cmd-alt-=", "cmd-ctrl--"] {
            assert_eq!(shortcut(&Keystroke::parse(key).unwrap()), None);
        }
        let mut international = Keystroke::parse("cmd-shift-1").unwrap();
        international.key_char = Some("+".into());
        assert_eq!(shortcut(&international), Some(Adjustment::Increase));
    }
}
