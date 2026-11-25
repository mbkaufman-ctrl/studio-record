//! Design tokens. Every colour in the UI comes from here.
//!
//! Lane: a quiet broadcast console. Warm near-black (or warm paper in
//! light mode), hairline borders, Geist type, and one accent — the
//! red-orange of a studio tally light — reserved for "you are live /
//! recording" and the single primary action.

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId, TextStyle};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy)]
pub struct Tokens {
    pub dark: bool,
    pub bg_base: Color32,
    pub bg_surface: Color32,
    pub bg_elevated: Color32,
    pub bg_inset: Color32,
    pub border_subtle: Color32,
    pub border_default: Color32,
    pub border_strong: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_tertiary: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_fg: Color32,
    pub accent_soft: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub hover_tint: Color32,
    pub monitor_bg: Color32,
}

/// HSL (h in degrees, s and l in percent) to a colour.
pub fn hsl(h: f32, s: f32, l: f32) -> Color32 {
    hsla(h, s, l, 1.0)
}

pub fn hsla(h: f32, s: f32, l: f32, a: f32) -> Color32 {
    let s = s / 100.0;
    let l = l / 100.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h % 360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(to(r), to(g), to(b), (a * 255.0) as u8)
}

impl Tokens {
    pub fn dark() -> Self {
        Tokens {
            dark: true,
            bg_base: hsl(30.0, 6.0, 6.5),
            bg_surface: hsl(30.0, 5.0, 9.5),
            bg_elevated: hsl(30.0, 5.0, 12.5),
            bg_inset: hsl(30.0, 6.0, 4.5),
            border_subtle: hsl(30.0, 5.0, 14.5),
            border_default: hsl(30.0, 5.0, 20.0),
            border_strong: hsl(30.0, 5.0, 30.0),
            text_primary: hsl(40.0, 12.0, 93.0),
            text_secondary: hsl(35.0, 6.0, 64.0),
            text_tertiary: hsl(35.0, 5.0, 44.0),
            accent: hsl(9.0, 86.0, 57.0),
            accent_hover: hsl(9.0, 90.0, 62.0),
            accent_fg: hsl(20.0, 30.0, 98.0),
            accent_soft: hsla(9.0, 86.0, 57.0, 0.14),
            success: hsl(145.0, 34.0, 52.0),
            warning: hsl(38.0, 72.0, 58.0),
            danger: hsl(2.0, 62.0, 60.0),
            hover_tint: hsla(40.0, 20.0, 90.0, 0.045),
            monitor_bg: hsl(240.0, 6.0, 6.5),
        }
    }

    pub fn light() -> Self {
        Tokens {
            dark: false,
            bg_base: hsl(40.0, 22.0, 96.5),
            bg_surface: hsl(40.0, 30.0, 99.0),
            bg_elevated: hsl(0.0, 0.0, 100.0),
            bg_inset: hsl(40.0, 14.0, 92.5),
            border_subtle: hsl(35.0, 14.0, 89.5),
            border_default: hsl(35.0, 10.0, 82.0),
            border_strong: hsl(35.0, 8.0, 66.0),
            text_primary: hsl(30.0, 10.0, 10.0),
            text_secondary: hsl(30.0, 5.0, 38.0),
            text_tertiary: hsl(30.0, 4.0, 56.0),
            accent: hsl(9.0, 80.0, 50.0),
            accent_hover: hsl(9.0, 84.0, 45.0),
            accent_fg: hsl(20.0, 30.0, 99.0),
            accent_soft: hsla(9.0, 80.0, 50.0, 0.10),
            success: hsl(145.0, 40.0, 36.0),
            warning: hsl(35.0, 80.0, 40.0),
            danger: hsl(2.0, 62.0, 46.0),
            hover_tint: hsla(30.0, 20.0, 20.0, 0.045),
            monitor_bg: hsl(240.0, 6.0, 7.0),
        }
    }
}

// Spacing (4px grid) and radii.
pub const S4: f32 = 4.0;
pub const S8: f32 = 8.0;
pub const S12: f32 = 12.0;
pub const S16: f32 = 16.0;
pub const S20: f32 = 20.0;
pub const S24: f32 = 24.0;
pub const S32: f32 = 32.0;
pub const R_SM: u8 = 6;
pub const R: u8 = 10;
pub const R_LG: u8 = 14;

pub fn medium() -> FontFamily {
    FontFamily::Name("medium".into())
}
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}
pub fn mono_medium() -> FontFamily {
    FontFamily::Name("mono-medium".into())
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    };
    add(&mut fonts, "geist", include_bytes!("../assets/fonts/geist-sans-400.ttf"));
    add(&mut fonts, "geist-medium", include_bytes!("../assets/fonts/geist-sans-500.ttf"));
    add(&mut fonts, "geist-semibold", include_bytes!("../assets/fonts/geist-sans-600.ttf"));
    add(&mut fonts, "geist-mono", include_bytes!("../assets/fonts/geist-mono-400.ttf"));
    add(&mut fonts, "geist-mono-medium", include_bytes!("../assets/fonts/geist-mono-500.ttf"));
    // Hebrew: Heebo, drawn for Hebrew UI text (Geist has no Hebrew letters).
    add(&mut fonts, "heebo", include_bytes!("../assets/fonts/heebo-400.ttf"));
    add(&mut fonts, "heebo-medium", include_bytes!("../assets/fonts/heebo-500.ttf"));
    add(&mut fonts, "heebo-semibold", include_bytes!("../assets/fonts/heebo-600.ttf"));

    // Key symbols (⌘ ⌥ ⌃ ⇧) aren't in Geist; borrow them from the system.
    let mut symbol_fallback: Vec<String> = Vec::new();
    for path in ["/System/Library/Fonts/Apple Symbols.ttf", "C:\\Windows\\Fonts\\seguisym.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert("symbols".into(), Arc::new(FontData::from_owned(bytes)));
            symbol_fallback.push("symbols".into());
            break;
        }
    }
    for fam in [FontFamily::Proportional, FontFamily::Monospace] {
        if let Some(v) = fonts.families.get_mut(&fam) {
            v.extend(symbol_fallback.iter().cloned());
        }
    }
    let fallback_prop = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let fallback_mono = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let with = |first: &str, hebrew: &str, rest: &Vec<String>| {
        let mut v = vec![first.to_string(), hebrew.to_string()];
        v.extend(rest.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, with("geist", "heebo", &fallback_prop));
    fonts.families.insert(FontFamily::Monospace, with("geist-mono", "heebo", &fallback_mono));
    fonts.families.insert(medium(), with("geist-medium", "heebo-medium", &fallback_prop));
    fonts.families.insert(semibold(), with("geist-semibold", "heebo-semibold", &fallback_prop));
    fonts.families.insert(mono_medium(), with("geist-mono-medium", "heebo-medium", &fallback_mono));
    ctx.set_fonts(fonts);
}

/// Installs both the dark and the light style; egui picks by preference.
pub fn apply(ctx: &egui::Context) {
    let base = (*ctx.global_style()).clone();
    ctx.set_style_of(egui::Theme::Dark, build(&base, &Tokens::dark()));
    ctx.set_style_of(egui::Theme::Light, build(&base, &Tokens::light()));
}

pub fn set_choice(ctx: &egui::Context, c: ThemeChoice) {
    ctx.set_theme(match c {
        ThemeChoice::System => egui::ThemePreference::System,
        ThemeChoice::Dark => egui::ThemePreference::Dark,
        ThemeChoice::Light => egui::ThemePreference::Light,
    });
}

pub fn current(ctx: &egui::Context) -> Tokens {
    if ctx.theme() == egui::Theme::Light {
        Tokens::light()
    } else {
        Tokens::dark()
    }
}

fn build(base: &egui::Style, t: &Tokens) -> egui::Style {
    let mut style = base.clone();
    style.text_styles = [
        (TextStyle::Heading, FontId::new(20.0, semibold())),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.0, medium())),
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(S8, S8);
    style.spacing.button_padding = egui::vec2(S12, 6.0);
    style.spacing.interact_size.y = 32.0;
    style.spacing.combo_height = 320.0;
    style.animation_time = 0.18;

    let mut v = if t.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = t.bg_base;
    v.window_fill = t.bg_elevated;
    v.extreme_bg_color = t.bg_inset;
    v.faint_bg_color = t.bg_surface;
    v.code_bg_color = t.bg_inset;
    v.override_text_color = None;
    v.hyperlink_color = t.accent;
    v.selection.bg_fill = t.accent_soft;
    v.selection.stroke = egui::Stroke::new(1.0, t.accent);
    v.window_stroke = egui::Stroke::new(1.0, t.border_default);
    v.window_corner_radius = egui::CornerRadius::same(R_LG);
    v.menu_corner_radius = egui::CornerRadius::same(R);
    v.popup_shadow = egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(if t.dark { 110 } else { 30 }) };
    v.window_shadow = v.popup_shadow;
    v.text_cursor.stroke = egui::Stroke::new(2.0, t.accent);

    let rounding = egui::CornerRadius::same(R_SM + 2);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = t.bg_surface;
    w.noninteractive.weak_bg_fill = t.bg_surface;
    w.noninteractive.bg_stroke = egui::Stroke::new(1.0, t.border_subtle);
    w.noninteractive.fg_stroke = egui::Stroke::new(1.0, t.text_secondary);
    w.noninteractive.corner_radius = rounding;

    w.inactive.bg_fill = t.bg_surface;
    w.inactive.weak_bg_fill = Color32::TRANSPARENT;
    w.inactive.bg_stroke = egui::Stroke::new(1.0, t.border_default);
    w.inactive.fg_stroke = egui::Stroke::new(1.5, t.text_primary);
    w.inactive.corner_radius = rounding;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = t.bg_elevated;
    w.hovered.weak_bg_fill = t.hover_tint;
    w.hovered.bg_stroke = egui::Stroke::new(1.0, t.border_strong);
    w.hovered.fg_stroke = egui::Stroke::new(1.5, t.text_primary);
    w.hovered.corner_radius = rounding;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = t.bg_elevated;
    w.active.weak_bg_fill = t.hover_tint;
    w.active.bg_stroke = egui::Stroke::new(1.0, t.accent);
    w.active.fg_stroke = egui::Stroke::new(1.5, t.text_primary);
    w.active.corner_radius = rounding;
    w.active.expansion = 0.0;

    w.open.bg_fill = t.bg_elevated;
    w.open.weak_bg_fill = t.bg_elevated;
    w.open.bg_stroke = egui::Stroke::new(1.0, t.border_strong);
    w.open.fg_stroke = egui::Stroke::new(1.5, t.text_primary);
    w.open.corner_radius = rounding;

    style.visuals = v;
    style
}
