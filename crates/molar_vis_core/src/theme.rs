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
/// Name of the bold font family, for [`egui::FontFamily::Name`]. Only bound when the system
/// actually provided a bold face — see [`install_system_fonts`] and [`bold`].
pub const BOLD_FAMILY: &str = "bold";

/// Whether [`BOLD_FAMILY`] is bound to any font.
///
/// An unbound `FontFamily::Name` makes egui **panic** at layout time, so every use of the bold
/// family has to be guarded. Set once by `install_system_fonts`; false on wasm (no filesystem
/// to read system fonts from) and on any system that yielded no distinct bold face.
static BOLD_AVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether bold text can be rendered (see [`BOLD_AVAILABLE`]).
pub fn has_bold() -> bool {
    BOLD_AVAILABLE.load(std::sync::atomic::Ordering::Relaxed)
}

/// A [`egui::FontId`] at `size` in the bold family, falling back to the normal proportional
/// family when there is no bold face — so callers never bind an unbound family.
pub fn bold(size: f32) -> egui::FontId {
    if has_bold() {
        egui::FontId::new(size, egui::FontFamily::Name(BOLD_FAMILY.into()))
    } else {
        egui::FontId::proportional(size)
    }
}

/// The **desktop's** UI font family, as configured in the user's environment.
///
/// Deliberately not fontconfig's generic `sans-serif`: that is the fallback for "some sans
/// font", whereas this is the family the desktop is actually set to. They routinely differ —
/// a KDE session set to "SF Pro Display" still answers "Noto Sans" for the generic alias.
///
/// Tried in order: the `MOLAR_VIS_UI_FONT` override, KDE's `kdeglobals`, then GTK's
/// `settings.ini`. `None` means "no preference found" and the caller falls back to the generic
/// alias. (macOS/Windows expose this through their own APIs; there fontdb's own defaults for
/// the generic families already resolve to the platform UI font.)
#[cfg(not(target_arch = "wasm32"))]
fn desktop_ui_font() -> Option<String> {
    if let Ok(name) = std::env::var("MOLAR_VIS_UI_FONT") {
        let name = name.trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|d| d.config_dir().to_path_buf()))?;

    // KDE: `[General] font=SF Pro Display,12,-1,5,400,…` — a Qt font spec, family first.
    if let Ok(text) = std::fs::read_to_string(config.join("kdeglobals")) {
        if let Some(family) = ini_value(&text, "General", "font")
            .and_then(|v| v.split(',').next().map(str::trim).map(str::to_owned))
            .filter(|f| !f.is_empty())
        {
            return Some(family);
        }
    }
    // GTK: `gtk-font-name=Cantarell 11` — family plus a trailing point size.
    for dir in ["gtk-4.0", "gtk-3.0"] {
        if let Ok(text) = std::fs::read_to_string(config.join(dir).join("settings.ini")) {
            if let Some(family) = ini_value(&text, "Settings", "gtk-font-name")
                .map(|v| {
                    v.trim_end_matches(|c: char| c.is_ascii_digit() || c == ' ' || c == '.')
                        .to_owned()
                })
                .filter(|f| !f.is_empty())
            {
                return Some(family);
            }
        }
    }
    None
}

/// `key`'s value inside `[section]` of a minimal INI file. Enough for the two desktop config
/// files read above — no escaping, no continuations, first match wins.
#[cfg(not(target_arch = "wasm32"))]
fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_section = name == section;
        } else if in_section {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim().to_owned());
                }
            }
        }
    }
    None
}

/// Replace the UI's proportional face with the **system** sans-serif font, and register its
/// **bold** sibling as [`BOLD_FAMILY`].
///
/// egui bundles only `Ubuntu-Light`, with no bold face anywhere, so `RichText::strong()` can
/// merely brighten the colour. Borrowing a bold face from an unrelated family (DejaVu) reads as
/// a foreign typeface next to Ubuntu-Light, so instead both weights are taken from **one**
/// family — whatever fontconfig says `sans-serif` is — which also makes the app look native.
///
/// The bundled fonts stay in the family lists *after* the system ones, so emoji and any glyph
/// the system face lacks still resolve. If the system yields no sans-serif, or no bold distinct
/// from the regular, nothing is changed and `has_bold()` stays false.
#[cfg(not(target_arch = "wasm32"))]
fn install_system_fonts(fonts: &mut egui::FontDefinitions) {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();

    // **The desktop's UI font, not fontconfig's generic `sans-serif`.** Those are different
    // things: this KDE box is set to "SF Pro Display" while `fc-match sans-serif` answers
    // "Noto Sans". Querying the generic alias would ignore the user's actual choice, which is
    // the whole point of picking up the system font. Falls back to the generic alias when no
    // desktop preference can be read.
    let ui_family = desktop_ui_font();
    if let Some(f) = &ui_family {
        log::debug!("desktop UI font preference: {f}");
    }
    let pick = |weight: fontdb::Weight| {
        let families = match &ui_family {
            Some(name) => vec![fontdb::Family::Name(name), fontdb::Family::SansSerif],
            None => vec![fontdb::Family::SansSerif],
        };
        db.query(&fontdb::Query {
            families: &families,
            weight,
            stretch: fontdb::Stretch::Normal,
            style: fontdb::Style::Normal,
        })
    };
    let load = |id: fontdb::ID| -> Option<(String, egui::FontData)> {
        let name = db.face(id)?.post_script_name.clone();
        let (bytes, index) = db.with_face_data(id, |data, index| (data.to_vec(), index))?;
        Some((
            name,
            egui::FontData {
                font: std::borrow::Cow::Owned(bytes),
                index,
                tweak: Default::default(),
            },
        ))
    };

    let Some(regular_id) = pick(fontdb::Weight::NORMAL) else {
        log::info!("no system sans-serif font found; keeping the bundled fonts (no bold)");
        return;
    };
    let Some((regular_name, regular)) = load(regular_id) else {
        return;
    };
    log::info!("UI font: {regular_name} (system sans-serif)");
    fonts.font_data.insert(regular_name.clone(), std::sync::Arc::new(regular));
    // Ahead of the bundled fonts, which stay as fallbacks (emoji, missing glyphs).
    let proportional = fonts.families.entry(egui::FontFamily::Proportional).or_default();
    proportional.insert(0, regular_name);
    let proportional = proportional.clone();

    // A family with only one weight resolves both queries to the same face — that is not a
    // bold face, so don't pretend it is.
    let bold_id = pick(fontdb::Weight::BOLD).filter(|&id| id != regular_id);
    let Some((bold_name, bold)) = bold_id.and_then(load) else {
        log::info!("system font has no distinct bold face; emphasis falls back to brighter text");
        return;
    };
    log::info!("bold UI font: {bold_name}");
    fonts.font_data.insert(bold_name.clone(), std::sync::Arc::new(bold));
    let mut family = vec![bold_name];
    family.extend(proportional);
    fonts
        .families
        .insert(egui::FontFamily::Name(BOLD_FAMILY.into()), family);
    BOLD_AVAILABLE.store(true, std::sync::atomic::Ordering::Relaxed);
}

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
    #[cfg(not(target_arch = "wasm32"))]
    install_system_fonts(&mut fonts);
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
