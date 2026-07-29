//! The application's look: the embedded fonts, and the two **style sheets** that define its dark
//! and light themes.
//!
//! The styling itself is data — `themes/dark.toml` and `themes/light.toml`, each a parent egui
//! preset plus overrides, `include_str!`d into the binary and applied by the app-independent
//! [`egui_stylesheet`] crate. This file is only the glue: which sheet is which, the fonts
//! (which are not part of `Style`), the user's accent from [`AppearanceSettings`], and the handful
//! of colors egui has no field for (see [`Extras`]).
//!
//! egui keeps a separate `Style` per theme; both are configured on every [`apply`], and
//! [`set_theme`](egui::Context::set_theme) picks which is live — so `System` mode follows the host
//! preference and looks right either way.

use std::sync::LazyLock;

use eframe::egui;
use egui::Color32;

use crate::settings::{AppearanceSettings, ThemeMode};
use egui_stylesheet::{self as style_sheet, StyleSheet};

/// Apply the molar_vis look. Call at startup and whenever the appearance settings
/// change (it's idempotent and cheap).
/// Name of the bold font family, for [`egui::FontFamily::Name`]. Always bound (the face is
/// embedded), so unlike a system-font lookup this needs no availability check — handy, since an
/// unbound `FontFamily::Name` makes egui panic at layout time. Use it via [`bold`].
pub const BOLD_FAMILY: &str = "bold";

/// **Ubuntu Regular** (400) — the base UI face, replacing egui's bundled Ubuntu-**Light** (300),
/// which reads too thin at these sizes.
const REGULAR_TTF: &[u8] = include_bytes!("../assets/Ubuntu-Regular-subset.ttf");

/// **Ubuntu Bold** (700) — for emphasis.
///
/// egui ships no bold face at all, and `RichText::strong()` only swaps in a brighter colour, so
/// bold text needs a font of its own. Taking one from an unrelated family (DejaVu, or the
/// system UI font) makes bold text read as a *second typeface* rather than as emphasis — so both
/// faces here are the same family as each other, differing only in weight.
///
/// Both are subset to Latin-1 plus the typographic/scientific characters the UI uses: ~18 kB
/// each against the stock ~344 kB. Regenerate with `assets/subset-fonts.sh`. Ubuntu Font Licence
/// 1.0 (`assets/Ubuntu-UFL.txt`), the same licence as the Ubuntu-Light egui already bundles.
const BOLD_TTF: &[u8] = include_bytes!("../assets/Ubuntu-Bold-subset.ttf");

/// A [`egui::FontId`] at `size` in the bold family.
pub fn bold(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(BOLD_FAMILY.into()))
}

/// Put Ubuntu **Regular** at the head of the proportional family and register **Bold** as its
/// own family.
///
/// Both keep egui's bundled fonts *after* them as fallbacks, so an emoji or any glyph outside
/// the Latin-1 subset still resolves (to Ubuntu-Light) instead of rendering as a missing-glyph
/// box.
fn install_fonts(fonts: &mut egui::FontDefinitions) {
    fonts.font_data.insert(
        "Ubuntu-Regular".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(REGULAR_TTF)),
    );
    fonts.font_data.insert(
        "Ubuntu-Bold".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(BOLD_TTF)),
    );
    let proportional = fonts.families.entry(egui::FontFamily::Proportional).or_default();
    proportional.insert(0, "Ubuntu-Regular".to_owned());
    let fallbacks = proportional.clone();

    let mut bold = vec!["Ubuntu-Bold".to_owned()];
    bold.extend(fallbacks);
    fonts
        .families
        .insert(egui::FontFamily::Name(BOLD_FAMILY.into()), bold);
}

pub fn apply(ctx: &egui::Context, a: &AppearanceSettings) {
    // Pick which of the two configured styles is active. `System` follows the
    // host/browser color-scheme preference; the others pin it.
    ctx.set_theme(match a.theme {
        ThemeMode::Dark => egui::ThemePreference::Dark,
        ThemeMode::Light => egui::ThemePreference::Light,
        ThemeMode::System => egui::ThemePreference::System,
    });

    // Fonts are not part of `Style`, so they stay here rather than in a sheet: the Phosphor icon
    // font (eye / trash / gear / … glyphs) merged with the two embedded Ubuntu faces.
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    install_fonts(&mut fonts);
    ctx.set_fonts(fonts);

    // Accent (selection highlight): stored linear RGBA → Color32 (WYSIWYG with the
    // settings color picker, which round-trips through `egui::Rgba`).
    let accent: Color32 =
        egui::Rgba::from_rgba_unmultiplied(a.accent[0], a.accent[1], a.accent[2], a.accent[3])
            .into();

    for (theme, sheet) in [(egui::Theme::Dark, &*DARK), (egui::Theme::Light, &*LIGHT)] {
        ctx.style_mut_of(theme, |style| {
            if let Err(e) = sheet.apply(style, a.font_scale) {
                // The built-in sheets are validated by `built_in_sheets_apply`, so this can only
                // fire for a sheet loaded from elsewhere: keep the previous style and say so.
                log::error!("theme {:?}: {e}", sheet.name);
                return;
            }
            // The accent is a *setting*, so it overrides the sheet — on dark only, where the
            // palette is built around it; the light sheet uses Purogrey's own selection grey.
            if theme == egui::Theme::Dark {
                style.visuals.selection.bg_fill = accent;
                // egui takes a **selected** widget's text color from `selection.stroke`, and the
                // accent is user-configurable, so the pairing is measured rather than written down.
                style.visuals.selection.stroke =
                    egui::Stroke::new(1.0_f32, style_sheet::contrasting_text(accent));
            }
        });
        // Stash the sheet's `[extras]` for the accessors below, keyed by theme — the *live* theme
        // can change without `apply` running (`System` mode follows the host), so both are kept
        // and the reader picks by `dark_mode`.
        let extras = Extras::from_sheet(sheet);
        ctx.data_mut(|d| d.insert_temp(extras_id(theme == egui::Theme::Dark), extras));
    }
}

/// The colors egui's `Visuals` has no field for. It models exactly two semantic colors —
/// `warn_fg_color` and `error_fg_color` — so anything else has to be named in a sheet's
/// `[extras]` table and looked up here.
///
/// Everything that *does* have an egui field goes through the style instead: an error message
/// reads `ui.visuals().error_fg_color`, not a helper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extras {
    /// An affirmative control — the pending selection's accept ✓.
    pub ok: Color32,
    /// Selection highlight over a **dark** 3-D backdrop (the GPU glow and the cues egui draws
    /// over the viewport).
    pub glow_on_dark_bg: Color32,
    /// …and over a **light** one, where a bright cyan would vanish.
    pub glow_on_light_bg: Color32,
}

impl Default for Extras {
    fn default() -> Self {
        Self {
            ok: Color32::GREEN,
            glow_on_dark_bg: Color32::from_rgb(130, 215, 255),
            glow_on_light_bg: Color32::from_rgb(230, 170, 0),
        }
    }
}

impl Extras {
    fn from_sheet(sheet: &StyleSheet) -> Self {
        let d = Self::default();
        Self {
            ok: sheet.extra("ok").unwrap_or(d.ok),
            glow_on_dark_bg: sheet.extra("glow_on_dark_bg").unwrap_or(d.glow_on_dark_bg),
            glow_on_light_bg: sheet.extra("glow_on_light_bg").unwrap_or(d.glow_on_light_bg),
        }
    }
}

/// The active theme's extras. Falls back to [`Extras::default`] before the first [`apply`].
pub fn extras(ctx: &egui::Context) -> Extras {
    let dark = ctx.global_style().visuals.dark_mode;
    ctx.data(|d| d.get_temp(extras_id(dark))).unwrap_or_default()
}

/// Color for an affirmative control, from the active theme's `[extras] ok`.
pub fn ok_color(ui: &egui::Ui) -> Color32 {
    extras(ui.ctx()).ok
}

/// Selection-highlight color for the given 3-D backdrop — the hover ring, the lasso polygon, the
/// draw-mode rubber band, and (via the camera uniform) the GPU selection glow itself.
///
/// Keyed off the **viewport background**, not the UI theme: a cue drawn over the render has to
/// contrast with the render. One source for both the egui-drawn cues and the shaders, so they
/// cannot drift apart.
pub fn glow_color(ctx: &egui::Context, background: &crate::camera::Background) -> Color32 {
    let e = extras(ctx);
    if background.is_light() {
        e.glow_on_light_bg
    } else {
        e.glow_on_dark_bg
    }
}

/// [`glow_color`] at `alpha` — for the layered hover ring.
pub fn glow_color_alpha(
    ctx: &egui::Context,
    background: &crate::camera::Background,
    alpha: u8,
) -> Color32 {
    let c = glow_color(ctx, background);
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

fn extras_id(dark: bool) -> egui::Id {
    egui::Id::new(("molar_vis_theme_extras", dark))
}

/// The built-in sheets, parsed once per process. Parsing is ~20 µs, but there is no reason to
/// redo it on every settings change.
static DARK: LazyLock<StyleSheet> = LazyLock::new(|| load("dark", include_str!("../themes/dark.toml")));
static LIGHT: LazyLock<StyleSheet> =
    LazyLock::new(|| load("light", include_str!("../themes/light.toml")));

/// Parse a built-in sheet. A failure here is a bug in a file that ships *inside the binary*, and
/// `built_in_sheets_apply` catches it in CI — but a release build should still start, so it falls
/// back to the bare egui preset rather than panicking.
fn load(which: &str, src: &str) -> StyleSheet {
    match StyleSheet::parse(src) {
        Ok(s) => s,
        Err(e) => {
            log::error!("built-in {which} theme is invalid ({e}); falling back to egui's preset");
            let parent = if which == "light" { "light" } else { "dark" };
            StyleSheet::parse(&format!("name = \"fallback\"\nparent = \"{parent}\"\n"))
                .expect("the minimal fallback sheet parses")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sheets ship *inside the binary*, so "does the theme file parse and apply" is a
    /// compile-and-test question, not something a user should discover at startup. Checks a few
    /// values from each file so a typo'd hex or a renamed egui field can't pass silently.
    #[test]
    fn built_in_sheets_apply() {
        let mut style = egui::Style::default();
        DARK.apply(&mut style, 1.0).expect("dark sheet");
        assert!(style.visuals.dark_mode);
        assert_eq!(style.visuals.panel_fill, Color32::from_rgb(20, 21, 25));
        // Text fields sit *above* the panel in both themes (egui's dark preset sinks them below).
        assert!(
            style_sheet::luma(style.visuals.extreme_bg_color)
                > style_sheet::luma(style.visuals.panel_fill)
        );
        // A dark backdrop needs a *light* shadow to show at all: white, premultiplied, so all
        // four channels carry the alpha (a black shadow would have rgb 0 and alpha > 0).
        let sh = style.visuals.window_shadow.color;
        assert!(sh.a() > 0, "the dark theme must cast a shadow at all");
        assert_eq!((sh.r(), sh.g(), sh.b()), (sh.a(), sh.a(), sh.a()), "expected a white bloom");

        let mut style = egui::Style::default();
        LIGHT.apply(&mut style, 1.0).expect("light sheet");
        assert!(!style.visuals.dark_mode);
        assert_eq!(style.visuals.panel_fill, Color32::from_rgb(198, 198, 198));
        assert_eq!(style.visuals.widgets.inactive.fg_stroke.color, Color32::BLACK);
        // A resting outline is what made frameless buttons resize on hover: it must stay off.
        assert_eq!(style.visuals.widgets.inactive.bg_stroke.width, 0.0);
        // Semantic colors: the error red is an egui field, the accept green an extra.
        assert_eq!(style.visuals.error_fg_color, Color32::from_rgb(170, 30, 30));
        assert_eq!(LIGHT.extra("ok"), Some(Color32::from_rgb(20, 110, 45)));
        assert!(LIGHT.extra("glow_on_light_bg").is_some());
        assert_eq!(DARK.extra("glow_on_dark_bg"), LIGHT.extra("glow_on_dark_bg"));
    }

    /// Hovering a widget must not **resize** it — in either theme.
    ///
    /// egui's `Style::button_style` subtracts the resting `bg_stroke.width` from `inner_margin`
    /// so that adding a border doesn't change a *framed* button's size. But a button that is
    /// frameless at rest (`Button::selectable` — every icon toggle in the rep rows, and every
    /// menu row) drops the stroke and keeps the shrunken margin, so a resting border of width 1
    /// made those widgets **1 px smaller at rest than on hover**: the row twitched under the
    /// cursor. Light-theme-only at the time, because only that palette had such a border.
    #[test]
    fn hover_does_not_resize_widgets() {
        use std::cell::RefCell;
        for theme in [ThemeMode::Dark, ThemeMode::Light] {
            let ctx = egui::Context::default();
            apply(&ctx, &AppearanceSettings { theme, ..Default::default() });
            let seen: RefCell<Vec<(&str, egui::Rect)>> = RefCell::new(Vec::new());
            // One frame with the pointer at `pointer`; yields each probe widget's rect.
            let frame = |pointer: egui::Pos2| {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 300.0),
                    )),
                    events: vec![egui::Event::PointerMoved(pointer)],
                    ..Default::default()
                };
                seen.borrow_mut().clear();
                let _ = ctx.run_ui(input, |ui| {
                    let r = ui.button("Button").rect;
                    seen.borrow_mut().push(("button", r));
                    // Frameless at rest — the case that broke.
                    let r = ui.selectable_label(false, "Toggle").rect;
                    seen.borrow_mut().push(("selectable_label", r));
                    let r = ui.menu_button("Menu", |_| {}).response.rect;
                    seen.borrow_mut().push(("menu_button", r));
                });
                seen.borrow().clone()
            };
            // egui reads a widget's state from the *previous* frame's response, so every
            // measurement is the second of two identical frames.
            let corner = egui::pos2(399.0, 299.0);
            frame(corner);
            let rest = frame(corner);
            for (i, (name, rect)) in rest.iter().enumerate() {
                frame(rect.center());
                let hovered = frame(rect.center())[i].1;
                assert_eq!(
                    rect.size(),
                    hovered.size(),
                    "{theme:?}: {name} resizes on hover ({:?} → {:?})",
                    rect.size(),
                    hovered.size()
                );
            }
        }
    }
}
