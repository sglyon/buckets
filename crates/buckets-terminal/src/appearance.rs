//! Host-supplied terminal styling, independent of the application's settings schema.
use crate::engine::palette;
use alacritty_terminal::{term::color::Colors, vte::ansi::Color};

#[derive(Clone, Debug, PartialEq)]
pub struct TerminalAppearance {
    pub font_family: String,
    pub background: u32,
    pub foreground: u32,
    pub selection_background: u32,
    pub cursor: u32,
    pub surface: u32,
    pub danger: u32,
    pub accent: u32,
    pub ansi: [u32; 16],
}
impl Default for TerminalAppearance {
    fn default() -> Self {
        Self {
            font_family: "JetBrains Mono".into(),
            background: 0x1a1b26,
            foreground: 0xc0caf5,
            selection_background: 0x343b58,
            cursor: 0x7aa2f7,
            surface: 0x24283b,
            danger: 0xf7768e,
            accent: 0x7aa2f7,
            ansi: std::array::from_fn(palette),
        }
    }
}
impl TerminalAppearance {
    pub(crate) fn palette(&self, index: usize) -> u32 {
        match index {
            0..=15 => self.ansi[index],
            256 | 267 => self.foreground,
            257 => self.background,
            258 => self.cursor,
            259..=266 => dim(self.ansi[index - 259]),
            268 => dim(self.foreground),
            _ => palette(index),
        }
    }
    pub(crate) fn color(&self, color: Color, colors: &Colors) -> u32 {
        let index = match color {
            Color::Spec(c) => return rgb(c),
            Color::Named(c) => c as usize,
            Color::Indexed(i) => i as usize,
        };
        self.indexed_color(index, colors)
    }
    pub(crate) fn indexed_color(&self, index: usize, colors: &Colors) -> u32 {
        colors[index]
            .map(rgb)
            .unwrap_or_else(|| self.palette(index))
    }
}
fn rgb(c: alacritty_terminal::vte::ansi::Rgb) -> u32 {
    ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
}
fn dim(color: u32) -> u32 {
    (((color >> 16 & 255) * 2 / 3) << 16)
        | (((color >> 8 & 255) * 2 / 3) << 8)
        | ((color & 255) * 2 / 3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::vte::ansi::{NamedColor, Rgb};

    #[test]
    fn configured_palette_defaults_and_extended_cube() {
        let mut appearance = TerminalAppearance {
            foreground: 0x123456,
            background: 0x654321,
            cursor: 0xfedcba,
            ..Default::default()
        };
        appearance.ansi[1] = 0xff2200;
        let colors = Colors::default();
        assert_eq!(appearance.color(Color::Indexed(1), &colors), 0xff2200);
        assert_eq!(
            appearance.color(Color::Named(NamedColor::Foreground), &colors),
            0x123456
        );
        assert_eq!(
            appearance.color(Color::Named(NamedColor::Background), &colors),
            0x654321
        );
        assert_eq!(
            appearance.color(Color::Named(NamedColor::Cursor), &colors),
            0xfedcba
        );
        assert_eq!(appearance.color(Color::Indexed(196), &colors), 0xff0000);
        assert_eq!(appearance.color(Color::Indexed(255), &colors), 0xeeeeee);
    }

    #[test]
    fn program_truecolor_and_dynamic_palette_override_theme() {
        let appearance = TerminalAppearance::default();
        let mut colors = Colors::default();
        colors[1] = Some(Rgb { r: 1, g: 2, b: 3 });
        colors[NamedColor::Background] = Some(Rgb { r: 4, g: 5, b: 6 });
        assert_eq!(appearance.color(Color::Indexed(1), &colors), 0x010203);
        assert_eq!(
            appearance.color(Color::Named(NamedColor::Background), &colors),
            0x040506
        );
        assert_eq!(
            appearance.color(Color::Spec(Rgb { r: 7, g: 8, b: 9 }), &colors),
            0x070809
        );
        colors[1] = None;
        assert_eq!(
            appearance.color(Color::Indexed(1), &colors),
            appearance.ansi[1]
        );
    }
}
