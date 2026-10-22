//! Portable appearance preferences. Missing overrides inherit Tokyo Night.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Stable settings keys, human-readable labels, and default RGB values.
pub const COLOR_TOKENS: &[(&str, &str, u32)] = &[
    ("background", "Background", 0x1a1b26),
    ("chrome", "Chrome", 0x16161e),
    ("surface", "Surface", 0x24283b),
    ("border", "Border", 0x343b58),
    ("text", "Text", 0xc0caf5),
    ("muted", "Muted text", 0x9aa5ce),
    ("accent", "Accent", 0x7aa2f7),
    ("directory", "Directories", 0x7dcfff),
    ("success", "Success", 0x9ece6a),
    ("warning", "Warning", 0xe0af68),
    ("danger", "Danger", 0xf7768e),
    ("cursor", "Active row", 0x2c3654),
    ("marked", "Marked row", 0x253746),
];

const DEFAULT_ANSI: [u32; 16] = [
    0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, 0x414868,
    0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub font_family: String,
    pub terminal_font_family: String,
    pub colors: BTreeMap<String, String>,
    pub terminal_ansi: Option<[String; 16]>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            font_family: "JetBrains Mono".into(),
            terminal_font_family: "JetBrains Mono".into(),
            colors: Default::default(),
            terminal_ansi: None,
            extra: Default::default(),
        }
    }
}

impl Appearance {
    pub fn validate(&self) -> Result<()> {
        for (key, family) in [
            ("font_family", &self.font_family),
            ("terminal_font_family", &self.terminal_font_family),
        ] {
            ensure!(
                !family.trim().is_empty()
                    && family.chars().count() <= 128
                    && !family.chars().any(char::is_control),
                "appearance.{key} must contain 1..128 characters without control characters"
            );
        }
        for (key, value) in &self.colors {
            ensure!(
                COLOR_TOKENS.iter().any(|(name, _, _)| *name == key),
                "Unknown appearance color token {key}"
            );
            ensure!(
                parse_color(value).is_some(),
                "appearance.colors.{key} must be a #RRGGBB color"
            );
        }
        if let Some(colors) = &self.terminal_ansi {
            for (index, value) in colors.iter().enumerate() {
                ensure!(
                    parse_color(value).is_some(),
                    "appearance.terminal_ansi[{index}] must be a #RRGGBB color"
                );
            }
        }
        Ok(())
    }

    /// Resolve a known token. Invalid unsaved values fall back to the default;
    /// persistence rejects them through `validate` before applying preferences.
    pub fn color(&self, key: &str) -> u32 {
        let default = COLOR_TOKENS
            .iter()
            .find(|(name, _, _)| *name == key)
            .expect("unknown appearance color token")
            .2;
        self.colors
            .get(key)
            .and_then(|value| parse_color(value))
            .unwrap_or(default)
    }

    pub fn ansi_palette(&self) -> [u32; 16] {
        match &self.terminal_ansi {
            Some(colors) => std::array::from_fn(|index| {
                parse_color(&colors[index]).unwrap_or(DEFAULT_ANSI[index])
            }),
            None => DEFAULT_ANSI,
        }
    }
}

fn parse_color(value: &str) -> Option<u32> {
    let digits = value.strip_prefix('#')?;
    (digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(digits, 16).ok())
        .flatten()
}
