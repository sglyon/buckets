//! Resolved live theme shared by app rendering, components and terminal panes.
use crate::appearance::Appearance;
use buckets_terminal::TerminalAppearance;
use gpui::{App, Global, SharedString, px, rgb, rgba};
use gpui_component::Theme;

pub const BUNDLED_FONT: &str = "JetBrains Mono";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub bg: u32,
    pub chrome: u32,
    pub surface: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub directory: u32,
    pub success: u32,
    pub warning: u32,
    pub danger: u32,
    pub cursor: u32,
    pub marked: u32,
}
impl From<&Appearance> for Palette {
    fn from(value: &Appearance) -> Self {
        Self {
            bg: value.color("background"),
            chrome: value.color("chrome"),
            surface: value.color("surface"),
            border: value.color("border"),
            text: value.color("text"),
            muted: value.color("muted"),
            accent: value.color("accent"),
            directory: value.color("directory"),
            success: value.color("success"),
            warning: value.color("warning"),
            danger: value.color("danger"),
            cursor: value.color("cursor"),
            marked: value.color("marked"),
        }
    }
}

#[derive(Clone)]
pub struct RuntimeTheme {
    pub palette: Palette,
    pub ui_font: SharedString,
    pub terminal: TerminalAppearance,
    pub font_notice: Option<String>,
}
impl Global for RuntimeTheme {}
impl Default for RuntimeTheme {
    fn default() -> Self {
        Self::resolve(&Appearance::default(), &[])
    }
}

/// Resolve installed names without changing the user's configured preference.
/// The bundled face is always available, including before platform enumeration.
pub fn resolve_font(requested: &str, installed: &[String]) -> (String, bool) {
    let requested = requested.trim();
    if requested.eq_ignore_ascii_case(BUNDLED_FONT) {
        return (BUNDLED_FONT.to_owned(), false);
    }
    installed
        .iter()
        .find(|name| name.eq_ignore_ascii_case(requested))
        .map(|name| (name.clone(), false))
        .unwrap_or_else(|| (BUNDLED_FONT.to_owned(), true))
}
impl RuntimeTheme {
    pub fn resolve(appearance: &Appearance, installed: &[String]) -> Self {
        let palette = Palette::from(appearance);
        let (ui_font, ui_fallback) = resolve_font(&appearance.font_family, installed);
        let (terminal_font, terminal_fallback) =
            resolve_font(&appearance.terminal_font_family, installed);
        let mut notices = vec![];
        if ui_fallback {
            notices.push(format!(
                "UI font '{}' is unavailable; using {BUNDLED_FONT}",
                appearance.font_family
            ));
        }
        if terminal_fallback {
            notices.push(format!(
                "Terminal font '{}' is unavailable; using {BUNDLED_FONT}",
                appearance.terminal_font_family
            ));
        }
        Self {
            palette,
            ui_font: ui_font.into(),
            terminal: TerminalAppearance {
                font_family: terminal_font,
                background: palette.bg,
                foreground: palette.text,
                selection_background: palette.border,
                cursor: palette.accent,
                surface: palette.surface,
                danger: palette.danger,
                accent: palette.accent,
                ansi: appearance.ansi_palette(),
            },
            font_notice: (!notices.is_empty()).then(|| notices.join(" · ")),
        }
    }
}
pub fn palette(cx: &App) -> Palette {
    cx.try_global::<RuntimeTheme>()
        .map(|theme| theme.palette)
        .unwrap_or_else(|| Palette::from(&Appearance::default()))
}
pub fn ui_font(cx: &App) -> SharedString {
    cx.try_global::<RuntimeTheme>()
        .map(|theme| theme.ui_font.clone())
        .unwrap_or_else(|| BUNDLED_FONT.into())
}
pub fn terminal(cx: &App) -> TerminalAppearance {
    cx.try_global::<RuntimeTheme>()
        .map(|theme| theme.terminal.clone())
        .unwrap_or_default()
}
pub fn font_notice(cx: &App) -> Option<String> {
    cx.try_global::<RuntimeTheme>()
        .and_then(|theme| theme.font_notice.clone())
}

/// Apply only after settings have been validated and durably saved.
pub fn apply(appearance: &Appearance, points: f32, cx: &mut App) {
    let resolved = RuntimeTheme::resolve(appearance, &cx.text_system().all_font_names());
    let p = resolved.palette;
    let theme = Theme::global_mut(cx);
    theme.font_family = resolved.ui_font.clone();
    theme.font_size = px(points);
    theme.mono_font_family = resolved.terminal.font_family.clone().into();
    theme.mono_font_size = px(points);
    let colors = &mut theme.colors;
    macro_rules! color {($value:expr; $($field:ident),+ $(,)?)=>{$(colors.$field=rgb($value).into();)+};}
    color!(p.bg; background,list,table,tiles);
    color!(p.chrome; accordion,group_box,description_list_label,list_head,popover,sidebar,tab,tab_bar,tab_bar_segmented,table_head,title_bar);
    color!(p.surface; accordion_hover,list_even,list_hover,muted,secondary,secondary_active,secondary_hover,skeleton,switch,tab_active,table_even,table_hover);
    color!(p.border; border,input,sidebar_border,table_row_border,title_bar_border,window_border,scrollbar_thumb);
    color!(p.text; foreground,group_box_foreground,description_list_label_foreground,popover_foreground,secondary_foreground,sidebar_foreground,tab_active_foreground,table_head_foreground);
    color!(p.muted; muted_foreground,tab_foreground,scrollbar_thumb_hover);
    color!(p.accent; primary,primary_hover,primary_active,caret,ring,link,link_active,link_hover,drag_border,progress_bar,sidebar_primary,slider_bar,slider_thumb,switch_thumb);
    color!(p.bg; primary_foreground,sidebar_primary_foreground,danger_foreground,info_foreground,success_foreground,warning_foreground);
    color!(p.surface; accent,sidebar_accent);
    color!(p.text; accent_foreground,sidebar_accent_foreground);
    color!(p.cursor; list_active,table_active);
    color!(p.accent; list_active_border,table_active_border);
    color!(p.marked; selection,drop_target);
    color!(p.danger; danger,danger_active,danger_hover,bearish,red,red_light);
    color!(p.success; success,success_active,success_hover,bullish,green,green_light);
    color!(p.warning; warning,warning_active,warning_hover,yellow,yellow_light);
    color!(p.directory; info,info_active,info_hover,cyan,cyan_light);
    color!(p.accent; blue,blue_light);
    color!(resolved.terminal.ansi[5]; magenta,magenta_light);
    color!(p.chrome; scrollbar);
    color!(resolved.terminal.ansi[1]; chart_1);
    color!(resolved.terminal.ansi[2]; chart_2);
    color!(resolved.terminal.ansi[3]; chart_3);
    color!(resolved.terminal.ansi[4]; chart_4);
    color!(resolved.terminal.ansi[5]; chart_5);
    colors.overlay = rgba((p.bg << 8) | 0xd9).into();
    cx.set_global(resolved);
}
