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

    // Merge in the Phosphor icon font (eye / trash / gear / … glyphs used by the
    // panel) alongside the default fonts.
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    install_fonts(&mut fonts);
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
        v.panel_fill = Color32::from_rgb(20, 21, 25);
        v.window_fill = Color32::from_rgb(28, 29, 34);
        // Text fields. egui's dark preset sinks these *below* the panel (a "well"), which against
        // this palette's near-black panel left the rep selection field indistinguishable from the
        // background. They're the brightest surface instead — the same relationship the light
        // palette has (fields 235 over a 207 window), so a field reads as a field in both.
        v.extreme_bg_color = Color32::from_rgb(44, 45, 52);
        set_text_colors(&mut v, Color32::from_rgb(238, 238, 242), Color32::from_rgb(184, 186, 194));
        v.selection.bg_fill = accent;
        // A *selected* widget's text comes from `selection.stroke`, so it has to contrast with
        // the accent — which is user-configurable, hence measured rather than paired by hand.
        v.selection.stroke = egui::Stroke::new(1.0_f32, contrasting_text(accent));
        v.window_shadow = SHADOW_DARK;
        v.popup_shadow = POPUP_SHADOW_DARK;
        style.visuals = v;
    });

    // Light mode: egui's built-in light visuals are far too low-contrast for this UI —
    // mid-grey text on near-white, which makes the rep rows and secondary labels hard to
    // read. Replaced with a palette taken from the **Purogrey** KDE colour scheme, whose
    // whole point is contrast: a mid-grey window (198) carrying *black* text, brighter
    // panels for input areas, and inactive text only mildly dimmed (48 rather than a pale
    // grey). Mirrors the structure of the dark palette above.
    ctx.style_mut_of(egui::Theme::Light, |style| {
        let mut v = egui::Visuals::light();
        v.panel_fill = LIGHT_WINDOW;
        v.window_fill = LIGHT_WINDOW_ALT;
        v.extreme_bg_color = LIGHT_VIEW_ALT;
        v.faint_bg_color = LIGHT_WINDOW_ALT;
        set_text_colors(&mut v, LIGHT_TEXT, LIGHT_TEXT_DIM);
        v.widgets.noninteractive.bg_fill = LIGHT_WINDOW;
        v.widgets.noninteractive.weak_bg_fill = LIGHT_WINDOW;
        v.widgets.noninteractive.bg_stroke.color = LIGHT_LINE;
        // Resting widgets are distinguished by **fill**, not by an outline, and that is a
        // constraint rather than a preference: a resting `bg_stroke` of nonzero width makes
        // buttons that are frameless at rest (`Button::selectable` — every icon toggle in the rep
        // rows, and every menu row) **jump 1 px on hover**. `Style::button_style` pre-subtracts
        // the stroke width from `inner_margin` so a border doesn't change a *framed* button's
        // size, but the frameless branch drops the stroke and keeps the shrunken margin. See
        // `hover_does_not_resize_widgets`.
        //
        // Two fills, and the distinction matters on a grey window: `weak_bg_fill` is a
        // button/dropdown **face**, `bg_fill` is an *indicator* interior — a checkbox box, a
        // slider rail. Both take Purogrey's bright View shade, which is what keeps a rail from
        // vanishing into the 207 dialog it sits on and gives a button a silhouette without a
        // border.
        v.widgets.inactive.bg_fill = LIGHT_VIEW;
        v.widgets.inactive.weak_bg_fill = LIGHT_VIEW;
        // Hover/press lift the fill further and firm up the outline (drawn only in these framed
        // states, so it costs no layout), since on a grey panel a fill change alone is easy to miss.
        v.widgets.hovered.bg_fill = LIGHT_HOVER;
        v.widgets.hovered.weak_bg_fill = LIGHT_HOVER;
        v.widgets.hovered.bg_stroke.color = LIGHT_DECORATION;
        v.widgets.active.bg_fill = LIGHT_PRESS;
        v.widgets.active.weak_bg_fill = LIGHT_PRESS;
        v.widgets.active.bg_stroke.color = LIGHT_FOCUS;
        v.widgets.open.bg_fill = LIGHT_VIEW;
        // `widgets.open.weak_bg_fill` is also what `Window` paints its **title bar** with, so
        // it has to sit *below* `window_fill` or the bar looks lighter than the dialog it
        // captions. Purogrey's own titlebar is dark (WM 87) with light text, but this colour
        // doubles as the fill of open dropdowns — whose text is black — so it goes one shade
        // down from the window instead of all the way dark.
        v.widgets.open.weak_bg_fill = LIGHT_TITLEBAR;
        // Purogrey's selection: a dark grey plate with near-white text, not the tinted-blue-
        // with-black-text egui defaults to.
        v.selection.bg_fill = LIGHT_SELECTION_BG;
        v.selection.stroke = egui::Stroke::new(1.0_f32, LIGHT_SELECTION_FG);
        v.window_shadow = SHADOW_LIGHT;
        v.popup_shadow = POPUP_SHADOW_LIGHT;
        style.visuals = v;
    });
}

// --- Purogrey-derived light palette ------------------------------------------------
// Read from the scheme's own `[Colors:*]` groups rather than eyeballed, so the contrast
// ratios are the ones that scheme was designed around.

/// Window background — the panels (`Colors:Window/BackgroundNormal`).
const LIGHT_WINDOW: Color32 = Color32::from_rgb(198, 198, 198);
/// Slightly lifted window shade, for floating windows and faint row striping.
const LIGHT_WINDOW_ALT: Color32 = Color32::from_rgb(207, 207, 207);
/// View background — lists and input areas (`Colors:View/BackgroundNormal`).
const LIGHT_VIEW: Color32 = Color32::from_rgb(226, 226, 226);
/// Brightest shade, for text fields (`Colors:View/BackgroundAlternate`).
const LIGHT_VIEW_ALT: Color32 = Color32::from_rgb(235, 235, 235);
/// Hovered widget fill — a step above the resting View shade.
const LIGHT_HOVER: Color32 = Color32::from_rgb(238, 238, 238);
/// Pressed/active widget fill — a step above that again.
const LIGHT_PRESS: Color32 = Color32::from_rgb(248, 248, 248);
/// Primary text — **black**, as the scheme specifies (`Colors:Window/ForegroundNormal`).
const LIGHT_TEXT: Color32 = Color32::from_rgb(0, 0, 0);
/// Dimmed text (`Colors:Window/ForegroundInactive`) — still near-black, which is the
/// difference between this and egui's washed-out light theme.
const LIGHT_TEXT_DIM: Color32 = Color32::from_rgb(48, 48, 48);
/// Separators / resting outlines (`Colors:Window/DecorationHover`).
const LIGHT_LINE: Color32 = Color32::from_rgb(106, 106, 106);
/// Hover outline (`Colors:Button/DecorationHover`).
const LIGHT_DECORATION: Color32 = Color32::from_rgb(119, 119, 119);
/// Focus outline (`Colors:Window/DecorationFocus`).
const LIGHT_FOCUS: Color32 = Color32::from_rgb(71, 71, 71);
/// Selected-row plate (`Colors:Selection/BackgroundNormal`) — deliberately dark, so the
/// near-white [`LIGHT_SELECTION_FG`] on top of it is unmistakable.
const LIGHT_SELECTION_BG: Color32 = Color32::from_rgb(94, 94, 94);
/// Selected-row text (`Colors:Selection/ForegroundNormal`).
const LIGHT_SELECTION_FG: Color32 = Color32::from_rgb(241, 241, 241);
/// Dialog title bars: one shade below the window body so the caption reads as a caption.
const LIGHT_TITLEBAR: Color32 = Color32::from_rgb(184, 184, 184);

// --- Shadows -----------------------------------------------------------------------
// egui's defaults are ~10 % black, which all but vanishes against either palette — a
// floating dialog then has no visible separation from the panel behind it. These are
// several times deeper, and offset further, so windows read as *above* the UI.

const SHADOW_LIGHT: egui::Shadow = egui::Shadow {
    offset: [8, 14],
    blur: 28,
    spread: 2,
    color: Color32::from_black_alpha(90),
};
const POPUP_SHADOW_LIGHT: egui::Shadow = egui::Shadow {
    offset: [4, 8],
    blur: 18,
    spread: 1,
    color: Color32::from_black_alpha(75),
};
/// Deeper still on dark, where a black shadow has less to work with.
const SHADOW_DARK: egui::Shadow = egui::Shadow {
    offset: [8, 14],
    blur: 28,
    spread: 2,
    color: Color32::from_black_alpha(160),
};
const POPUP_SHADOW_DARK: egui::Shadow = egui::Shadow {
    offset: [4, 8],
    blur: 18,
    spread: 1,
    color: Color32::from_black_alpha(140),
};

/// Set every text colour of a palette from just two: `text` for anything at rest, `dim` for
/// secondary labels.
///
/// The obvious lever, `visuals.override_text_color`, is a **trap**: it forces one colour on
/// *all* widget text, including a **selected** widget's — so a selected toggle painted with the
/// selection colour kept the panel's text colour and could come out black-glyph-on-dark-plate
/// (or white-on-light, if the accent were pale). Setting the per-state strokes instead lets
/// egui pick `selection.stroke` when a widget is selected, which is the whole point of that
/// field. `Visuals::text_color()` reads `noninteractive`, so our own painters follow along.
fn set_text_colors(v: &mut egui::Visuals, text: Color32, dim: Color32) {
    for st in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        st.fg_stroke.color = text;
    }
    // Secondary labels (counts, the fps footer, suggestion hints). Left to egui this is
    // `text × weak_text_alpha`, which on either palette fades further than these UIs can
    // afford — both themes deliberately keep their dim text close to the primary one.
    v.weak_text_color = Some(dim);
}

/// Black or white, whichever is legible on `bg`.
///
/// Used for the text of *selected* widgets, whose colour egui takes from
/// `visuals.selection.stroke` — the accent is user-configurable, so the pairing can't be
/// hardcoded. Rec. 601 luma, which tracks perceived brightness closely enough for a
/// two-way choice.
fn contrasting_text(bg: Color32) -> Color32 {
    let luma = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    if luma > 140.0 {
        Color32::from_rgb(16, 16, 16)
    } else {
        Color32::from_rgb(244, 244, 246)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The accent is user-configurable, so the *selected* ink has to be derived from it — a
    /// dark accent needs a light glyph and a pale one a dark glyph. Getting this backwards
    /// is invisible in code review and glaring on screen (a black glyph on a dark plate).
    #[test]
    fn selected_ink_contrasts_with_the_accent() {
        for dark in [
            Color32::from_rgb(54, 96, 167),  // the default blue accent
            Color32::from_rgb(94, 94, 94),   // Purogrey's selection grey
            Color32::BLACK,
        ] {
            assert!(contrasting_text(dark).r() > 200, "{dark:?} needs light ink");
        }
        for pale in [
            Color32::from_rgb(255, 214, 10), // amber
            Color32::from_rgb(160, 220, 160),
            Color32::WHITE,
        ] {
            assert!(contrasting_text(pale).r() < 60, "{pale:?} needs dark ink");
        }
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
                ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let r = ui.button("Button").rect;
                        seen.borrow_mut().push(("button", r));
                        // Frameless at rest — the case that broke.
                        let r = ui.selectable_label(false, "Toggle").rect;
                        seen.borrow_mut().push(("selectable_label", r));
                        let r = ui.menu_button("Menu", |_| {}).response.rect;
                        seen.borrow_mut().push(("menu_button", r));
                    });
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
