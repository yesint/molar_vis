//! Global egui styling: larger fonts and a higher-contrast dark theme.

use eframe::egui;
use egui::{Color32, FontFamily, FontId, TextStyle};

/// Apply the molar_vis look. Call once at startup with the egui context.
pub fn apply(ctx: &egui::Context) {
    // Force the dark theme regardless of the host's color-scheme preference.
    // Without this, eframe on the web follows the browser's `prefers-color-scheme`
    // (often light) and resolves the active theme to Light, so the dark `Visuals`
    // set via `set_global_style` below land on the inactive theme and the UI shows
    // up white. Pinning the preference makes Dark the active theme everywhere.
    ctx.set_theme(egui::ThemePreference::Dark);

    // Merge in the Phosphor icon font (eye / trash / copy / plus / projection
    // glyphs used by the panel) alongside the default fonts.
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);

    let mut style = (*ctx.global_style()).clone();

    // Larger, more legible type scale.
    style.text_styles = [
        (TextStyle::Heading, FontId::new(24.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(17.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(17.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(15.0, FontFamily::Monospace)),
        (TextStyle::Small, FontId::new(13.5, FontFamily::Proportional)),
    ]
    .into();

    // High-contrast dark palette.
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(Color32::from_rgb(238, 238, 242));
    v.panel_fill = Color32::from_rgb(20, 21, 25);
    v.window_fill = Color32::from_rgb(28, 29, 34);
    v.extreme_bg_color = Color32::from_rgb(12, 12, 15);
    // Brighter "weak"/non-interactive text so secondary labels stay readable.
    v.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(196, 198, 205);
    v.widgets.inactive.fg_stroke.color = Color32::from_rgb(220, 222, 228);
    v.selection.bg_fill = Color32::from_rgb(54, 96, 168);
    style.visuals = v;

    // A little more breathing room around widgets.
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);

    ctx.set_global_style(style);
}
