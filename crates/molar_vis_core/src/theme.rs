//! Global egui styling: larger fonts and a higher-contrast dark theme.
//!
//! Driven by [`AppearanceSettings`] (theme mode, font scale, accent color) so the
//! look is user-configurable + persisted. egui keeps a separate style for dark and
//! light mode; we configure **both** (custom high-contrast dark palette, egui's
//! built-in light) plus the shared font scale / spacing / accent, then
//! [`set_theme`](egui::Context::set_theme) picks which is active — so `System`
//! mode follows the host preference and looks right either way.

use eframe::egui;
use egui::{Color32, FontFamily, FontId, TextStyle};

use crate::settings::{AppearanceSettings, ThemeMode};

/// Apply the molar_vis look. Call at startup and whenever the appearance settings
/// change (it's idempotent and cheap).
pub fn apply(ctx: &egui::Context, a: &AppearanceSettings) {
    // Pick which of the two configured styles is active. `System` follows the
    // host/browser color-scheme preference; the others pin it.
    ctx.set_theme(match a.theme {
        ThemeMode::Dark => egui::ThemePreference::Dark,
        ThemeMode::Light => egui::ThemePreference::Light,
        ThemeMode::System => egui::ThemePreference::System,
    });

    // Merge in the Phosphor icon font (eye / trash / gear / … glyphs used by the
    // panel) alongside the default fonts.
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);

    // Accent (selection highlight): stored linear RGBA → Color32 (WYSIWYG with the
    // settings color picker, which round-trips through `egui::Rgba`).
    let accent: Color32 =
        egui::Rgba::from_rgba_unmultiplied(a.accent[0], a.accent[1], a.accent[2], a.accent[3])
            .into();

    // Larger, more legible type scale + breathing room — applied to *both* the dark
    // and light styles (theme-independent).
    let s = a.font_scale.clamp(0.5, 3.0);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(24.0 * s, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(17.0 * s, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(17.0 * s, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(15.0 * s, FontFamily::Monospace)),
            (TextStyle::Small, FontId::new(13.5 * s, FontFamily::Proportional)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 7.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
    });

    // High-contrast dark palette (the original look) + accent.
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        let mut v = egui::Visuals::dark();
        v.override_text_color = Some(Color32::from_rgb(238, 238, 242));
        v.panel_fill = Color32::from_rgb(20, 21, 25);
        v.window_fill = Color32::from_rgb(28, 29, 34);
        v.extreme_bg_color = Color32::from_rgb(12, 12, 15);
        // Brighter "weak"/non-interactive text so secondary labels stay readable.
        v.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(196, 198, 205);
        v.widgets.inactive.fg_stroke.color = Color32::from_rgb(220, 222, 228);
        v.selection.bg_fill = accent;
        style.visuals = v;
    });

    // Light mode: egui's built-in light visuals + the accent.
    ctx.style_mut_of(egui::Theme::Light, |style| {
        let mut v = egui::Visuals::light();
        v.selection.bg_fill = accent;
        style.visuals = v;
    });
}
