//! Build an [`egui::Style`] from a small, hand-writable TOML **style sheet**: a parent preset
//! plus overrides.
//!
//! This module knows nothing about this application — it is a general egui utility, kept
//! self-contained so it can be lifted into another project (or its own crate) unchanged. Its only
//! dependencies are `egui` (with the `serde` feature), `serde_json` and `toml`.
//!
//! # Why not a full style dump
//!
//! egui's `Style`/`Visuals` are serde-serializable, so a theme *can* be a complete dump of them —
//! which is what the existing crates in this space do ([`egui-theme`], [`egui-thematic`],
//! [`egui-stylist`]). Two problems with that: the file is a hundred-odd fields of egui internals
//! that nobody wants to hand-edit, and it silently pins itself to one egui version's struct shape.
//! A sheet here says only what it *changes*, on top of `Visuals::dark()` or `Visuals::light()`, so
//! it stays short, reviewable, and mostly version-proof: fields egui adds keep their new defaults
//! instead of being frozen at the values that happened to be current when the file was written.
//!
//! [`egui-theme`]: https://crates.io/crates/egui-theme
//! [`egui-thematic`]: https://crates.io/crates/egui-thematic
//! [`egui-stylist`]: https://github.com/jacobsky/egui-stylist
//!
//! # Format
//!
//! ```toml
//! name = "Purogrey"          # for humans / logs
//! parent = "light"           # "dark" | "light" — the egui preset to start from
//!
//! [palette]                  # optional named colors, referenced below as "$name"
//! window = "#c6c6c6"
//! text   = "#000000"
//!
//! [visuals]                  # keys are `egui::Visuals` field paths, verbatim
//! panel_fill = "$window"
//! error_fg_color = "#aa1e1e"
//! widgets.inactive.bg_fill = "$window"
//! widgets.hovered.bg_stroke = { width = 1.0, color = "#777777" }
//! window_shadow = { offset = [8, 14], blur = 28, spread = 2, color = "#0000005a" }
//!
//! [text]                     # sugar: egui has *five* per-state text colors + a weak one
//! normal = "$text"
//! dim    = "#303030"
//!
//! [metrics]                  # the parts of `Style` outside `Visuals`
//! heading = 24.0
//! body = 17.0
//! item_spacing = [8.0, 7.0]
//!
//! [extras]                   # colors egui has no field for; the host app names them
//! ok = "#146e2d"
//! ```
//!
//! Values: `"#rgb"`, `"#rrggbb"`, `"#rrggbbaa"` (alpha is **premultiplied** on the way in, as
//! [`egui::Color32`] expects), `"$name"` for a palette entry, or a raw `[r, g, b, a]` array.
//! Anything egui's own serde accepts also works, so nested tables (strokes, shadows) can be given
//! whole or field-by-field.
//!
//! A misspelled path is an **error**, not a silent no-op: every override is checked against the
//! parent's own shape before it is applied. Parsing is done once (see the callers' `LazyLock`) and
//! costs tens of microseconds, so a sheet can be `include_str!`d into the binary and still cost
//! nothing measurable at startup.

use std::collections::BTreeMap;

use egui::Color32;

/// A parsed style sheet: a parent preset plus the overrides to apply on top of it.
#[derive(Debug, Clone)]
pub struct StyleSheet {
    /// Human-readable name (`name` in the file), for logs and pickers.
    pub name: String,
    parent: Parent,
    /// The `[visuals]` overrides, as a JSON object with `$palette` refs and colors resolved.
    visuals: serde_json::Value,
    text: Option<TextColors>,
    metrics: Metrics,
    extras: BTreeMap<String, Color32>,
}

/// Which built-in egui preset a sheet starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parent {
    Dark,
    Light,
}

impl Parent {
    fn visuals(self) -> egui::Visuals {
        match self {
            Self::Dark => egui::Visuals::dark(),
            Self::Light => egui::Visuals::light(),
        }
    }

    /// Whether a sheet built on this parent is a dark theme (`Visuals::dark_mode`).
    pub fn is_dark(self) -> bool {
        self == Self::Dark
    }
}

#[derive(Debug, Clone, Copy)]
struct TextColors {
    normal: Color32,
    dim: Color32,
}

/// The `Style` knobs outside `Visuals` a sheet may set. All optional: an absent value leaves
/// egui's default alone.
#[derive(Debug, Clone, Copy, Default)]
struct Metrics {
    heading: Option<f32>,
    body: Option<f32>,
    button: Option<f32>,
    monospace: Option<f32>,
    small: Option<f32>,
    item_spacing: Option<[f32; 2]>,
    button_padding: Option<[f32; 2]>,
}

/// What can go wrong reading or applying a sheet. All of it is authoring error, so the messages
/// name the offending key.
#[derive(Debug)]
pub enum Error {
    Toml(String),
    /// `parent` is missing or is not `"dark"`/`"light"`.
    Parent(String),
    /// A color value that isn't a hex string, a palette reference, or an `[r,g,b,a]` array.
    Color { key: String, value: String },
    /// An override path that does not exist in `egui::Visuals` (almost always a typo).
    UnknownField(String),
    /// The merged tree didn't deserialize back into `Visuals` — a well-named field given a value
    /// of the wrong shape.
    Shape(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Toml(e) => write!(f, "not valid TOML: {e}"),
            Self::Parent(p) => write!(f, "`parent` must be \"dark\" or \"light\", got {p:?}"),
            Self::Color { key, value } => {
                write!(f, "{key}: {value:?} is not a color (use \"#rrggbb[aa]\", \"$palette_name\" or [r,g,b,a])")
            }
            Self::UnknownField(path) => write!(f, "no such egui field: {path}"),
            Self::Shape(e) => write!(f, "wrong value shape: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl StyleSheet {
    /// Parse a sheet. Does no egui work, so it is cheap and callable before a `Context` exists.
    pub fn parse(src: &str) -> Result<Self, Error> {
        // `toml::Table`, not `toml::Value`: the latter's `FromStr` parses a single *value*, so a
        // whole document fails with a baffling "expected nothing".
        let table: toml::Table = toml::from_str(src).map_err(|e| Error::Toml(format!("{e}")))?;
        let table = &table;

        let parent = match table.get("parent").and_then(|v| v.as_str()) {
            Some("dark") => Parent::Dark,
            Some("light") => Parent::Light,
            other => return Err(Error::Parent(other.unwrap_or("<missing>").to_owned())),
        };
        let name = table
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unnamed")
            .to_owned();

        // Named colors, resolved first so everything below can reference them.
        let mut palette = BTreeMap::new();
        if let Some(t) = table.get("palette").and_then(|v| v.as_table()) {
            for (k, v) in t {
                let s = v.as_str().unwrap_or_default();
                palette.insert(
                    k.clone(),
                    parse_hex(s).ok_or_else(|| Error::Color {
                        key: format!("palette.{k}"),
                        value: s.to_owned(),
                    })?,
                );
            }
        }

        // `[visuals]` → JSON, with color strings turned into the `[r,g,b,a]` arrays egui's serde
        // expects. Going through JSON (not TOML) as the merge medium is deliberate: `Visuals` has
        // `Option` fields, and TOML cannot represent `None` at all.
        let visuals = match table.get("visuals") {
            Some(v) => {
                let json = serde_json::to_value(v).map_err(|e| Error::Toml(format!("{e}")))?;
                resolve_colors(json, &palette, "visuals")?
            }
            None => serde_json::Value::Object(Default::default()),
        };

        let text = match table.get("text").and_then(|v| v.as_table()) {
            Some(t) => {
                let pick = |k: &str| -> Result<Color32, Error> {
                    let s = t.get(k).and_then(|v| v.as_str()).unwrap_or_default();
                    resolve_color_str(s, &palette).ok_or_else(|| Error::Color {
                        key: format!("text.{k}"),
                        value: s.to_owned(),
                    })
                };
                Some(TextColors { normal: pick("normal")?, dim: pick("dim")? })
            }
            None => None,
        };

        let mut metrics = Metrics::default();
        if let Some(t) = table.get("metrics").and_then(|v| v.as_table()) {
            let num = |k: &str| t.get(k).and_then(|v| v.as_float().map(|f| f as f32).or_else(|| v.as_integer().map(|i| i as f32)));
            let pair = |k: &str| -> Option<[f32; 2]> {
                let a = t.get(k)?.as_array()?;
                let f = |i: usize| -> Option<f32> {
                    let v = a.get(i)?;
                    v.as_float().map(|f| f as f32).or_else(|| v.as_integer().map(|i| i as f32))
                };
                Some([f(0)?, f(1)?])
            };
            metrics = Metrics {
                heading: num("heading"),
                body: num("body"),
                button: num("button"),
                monospace: num("monospace"),
                small: num("small"),
                item_spacing: pair("item_spacing"),
                button_padding: pair("button_padding"),
            };
        }

        let mut extras = BTreeMap::new();
        if let Some(t) = table.get("extras").and_then(|v| v.as_table()) {
            for (k, v) in t {
                let s = v.as_str().unwrap_or_default();
                extras.insert(
                    k.clone(),
                    resolve_color_str(s, &palette).ok_or_else(|| Error::Color {
                        key: format!("extras.{k}"),
                        value: s.to_owned(),
                    })?,
                );
            }
        }

        Ok(Self { name, parent, visuals, text, metrics, extras })
    }

    /// The preset this sheet builds on.
    pub fn parent(&self) -> Parent {
        self.parent
    }

    /// A color from `[extras]` — for the things egui has no field for (it models only
    /// `warn_fg_color` and `error_fg_color`, so e.g. an affirmative green lives here).
    pub fn extra(&self, key: &str) -> Option<Color32> {
        self.extras.get(key).copied()
    }

    /// Overwrite `style`'s visuals with the parent preset plus this sheet's overrides, and apply
    /// its metrics. `font_scale` multiplies the type sizes (1.0 = as written).
    ///
    /// Assigns rather than merges into the existing visuals, so applying a sheet twice is
    /// idempotent and a previously applied sheet leaves nothing behind.
    pub fn apply(&self, style: &mut egui::Style, font_scale: f32) -> Result<(), Error> {
        let base = serde_json::to_value(self.parent.visuals())
            .map_err(|e| Error::Shape(format!("{e}")))?;
        check_paths(&base, &self.visuals, "")?;
        let mut merged = base;
        merge(&mut merged, &self.visuals);
        style.visuals =
            serde_json::from_value(merged).map_err(|e| Error::Shape(format!("{e}")))?;

        if let Some(t) = self.text {
            set_text_colors(&mut style.visuals, t.normal, t.dim);
        }

        let m = &self.metrics;
        let s = font_scale.clamp(0.1, 10.0);
        let mut sizes: Vec<(egui::TextStyle, egui::FontId)> = Vec::new();
        let mut push = |st: egui::TextStyle, size: Option<f32>, family: egui::FontFamily| {
            if let Some(size) = size {
                sizes.push((st, egui::FontId::new(size * s, family)));
            }
        };
        use egui::{FontFamily::Monospace, FontFamily::Proportional, TextStyle};
        push(TextStyle::Heading, m.heading, Proportional);
        push(TextStyle::Body, m.body, Proportional);
        push(TextStyle::Button, m.button, Proportional);
        push(TextStyle::Monospace, m.monospace, Monospace);
        push(TextStyle::Small, m.small, Proportional);
        for (st, id) in sizes {
            style.text_styles.insert(st, id);
        }
        if let Some(v) = m.item_spacing {
            style.spacing.item_spacing = v.into();
        }
        if let Some(v) = m.button_padding {
            style.spacing.button_padding = v.into();
        }
        Ok(())
    }
}

/// Set every text color of a palette from just two: `text` for anything at rest, `dim` for
/// secondary labels.
///
/// The obvious lever, `visuals.override_text_color`, is a **trap**: it forces one color on *all*
/// widget text, including a **selected** widget's — whose color egui otherwise takes from
/// `selection.stroke`, so a selected toggle ends up with the panel's ink on the selection plate
/// (unreadable when the two are close in luminance). Setting the per-state strokes instead leaves
/// that mechanism working. `Visuals::text_color()` reads `noninteractive`, so painters that ask the
/// style for "the text color" follow along.
pub fn set_text_colors(v: &mut egui::Visuals, text: Color32, dim: Color32) {
    for st in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        st.fg_stroke.color = text;
    }
    // Left to egui this is `text × weak_text_alpha`, which fades further than a dense UI can
    // usually afford, so it is set outright.
    v.weak_text_color = Some(dim);
}

/// Black or white, whichever is legible on `bg` (Rec. 601 luma).
///
/// For text drawn on a plate whose color is chosen at runtime — a user-configurable accent, say —
/// where the pairing can't be written down in advance.
pub fn contrasting_text(bg: Color32) -> Color32 {
    if luma(bg) > 140.0 {
        Color32::from_rgb(16, 16, 16)
    } else {
        Color32::from_rgb(244, 244, 246)
    }
}

/// Rec. 601 luma of a color, 0..255.
pub fn luma(c: Color32) -> f32 {
    0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32
}

// --- internals ---------------------------------------------------------------------

/// `#rgb` / `#rrggbb` / `#rrggbbaa` → a color. Alpha is premultiplied, since [`Color32`] stores
/// premultiplied values and a caller writing `#00000080` means "half-transparent black".
fn parse_hex(s: &str) -> Option<Color32> {
    let h = s.strip_prefix('#')?;
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    match h.len() {
        3 => {
            let d = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok().map(|v| v * 17);
            Some(Color32::from_rgb(d(0)?, d(1)?, d(2)?))
        }
        6 => Some(Color32::from_rgb(b(0)?, b(2)?, b(4)?)),
        8 => Some(Color32::from_rgba_unmultiplied(b(0)?, b(2)?, b(4)?, b(6)?)),
        _ => None,
    }
}

/// A color written as a hex string or as `$palette_name`.
fn resolve_color_str(s: &str, palette: &BTreeMap<String, Color32>) -> Option<Color32> {
    match s.strip_prefix('$') {
        Some(name) => palette.get(name).copied(),
        None => parse_hex(s),
    }
}

/// Walk a JSON tree and turn every string into the `[r,g,b,a]` array egui's serde expects for a
/// color. Type-agnostic on purpose: the module doesn't need to know which fields are colors, only
/// that a string in a style sheet is always one (nothing else in `Visuals` is a string).
fn resolve_colors(
    v: serde_json::Value,
    palette: &BTreeMap<String, Color32>,
    path: &str,
) -> Result<serde_json::Value, Error> {
    use serde_json::Value;
    Ok(match v {
        Value::String(s) => {
            let c = resolve_color_str(&s, palette).ok_or_else(|| Error::Color {
                key: path.to_owned(),
                value: s.clone(),
            })?;
            serde_json::json!([c.r(), c.g(), c.b(), c.a()])
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let child = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                out.insert(k, resolve_colors(v, palette, &child)?);
            }
            Value::Object(out)
        }
        other => other,
    })
}

/// Recursively overlay `over` onto `base`: objects merge key-by-key, everything else replaces.
/// So `widgets.hovered.bg_stroke.color = …` changes only that color, while
/// `widgets.hovered.bg_stroke = { … }` replaces the whole stroke.
fn merge(base: &mut serde_json::Value, over: &serde_json::Value) {
    match (base, over) {
        (serde_json::Value::Object(b), serde_json::Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(slot) => merge(slot, v),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

/// Check that every override path exists in the parent's serialized shape. serde ignores unknown
/// fields, so without this a typo (`panel_fil`) would be accepted and do nothing at all — the
/// worst failure mode for a hand-edited file.
fn check_paths(
    base: &serde_json::Value,
    over: &serde_json::Value,
    path: &str,
) -> Result<(), Error> {
    let (Some(b), Some(o)) = (base.as_object(), over.as_object()) else {
        return Ok(());
    };
    for (k, v) in o {
        let child = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
        match b.get(k) {
            // A `null` in the base is an `Option::None` field: it has no shape to descend into,
            // so the value is taken on trust and checked by the deserializer instead.
            Some(next) if !next.is_null() => check_paths(next, v, &child)?,
            Some(_) => {}
            None => return Err(Error::UnknownField(child)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHEET: &str = r##"
        name = "Test"
        parent = "dark"

        [palette]
        plum = "#804090"

        [visuals]
        panel_fill = "$plum"
        error_fg_color = "#ff0000"
        widgets.inactive.bg_fill = "#010203"
        widgets.hovered.bg_stroke = { width = 2.0, color = "#0f0" }
        window_shadow = { offset = [1, 2], blur = 3, spread = 4, color = "#00000080" }

        [text]
        normal = "#eeeeee"
        dim = "#999999"

        [metrics]
        body = 17.0
        item_spacing = [8.0, 7.0]

        [extras]
        ok = "$plum"
    "##;

    fn applied(src: &str, scale: f32) -> egui::Style {
        let sheet = StyleSheet::parse(src).expect("parse");
        let mut style = egui::Style::default();
        sheet.apply(&mut style, scale).expect("apply");
        style
    }

    #[test]
    fn overrides_land_and_the_rest_is_the_parent() {
        let style = applied(SHEET, 1.0);
        let v = &style.visuals;
        assert_eq!(v.panel_fill, Color32::from_rgb(0x80, 0x40, 0x90));
        assert_eq!(v.error_fg_color, Color32::RED);
        assert_eq!(v.widgets.inactive.bg_fill, Color32::from_rgb(1, 2, 3));
        // A whole-table override replaces both fields of the stroke…
        assert_eq!(v.widgets.hovered.bg_stroke.width, 2.0);
        assert_eq!(v.widgets.hovered.bg_stroke.color, Color32::from_rgb(0, 255, 0));
        // …and `#rrggbbaa` premultiplies, as Color32 requires.
        assert_eq!(v.window_shadow.offset, [1, 2]);
        assert_eq!(v.window_shadow.color, Color32::from_rgba_unmultiplied(0, 0, 0, 0x80));
        // Untouched fields keep the *parent's* values, not `Visuals::default()`'s.
        let parent = egui::Visuals::dark();
        assert_eq!(v.window_fill, parent.window_fill);
        assert_eq!(v.hyperlink_color, parent.hyperlink_color);
        assert!(v.dark_mode);
        // Text sugar reaches all five states plus the weak color.
        assert_eq!(v.widgets.open.fg_stroke.color, Color32::from_rgb(238, 238, 238));
        assert_eq!(v.weak_text_color, Some(Color32::from_rgb(153, 153, 153)));
        // Metrics, scaled.
        assert_eq!(style.spacing.item_spacing, egui::vec2(8.0, 7.0));
        assert_eq!(style.text_styles[&egui::TextStyle::Body].size, 17.0);
        assert_eq!(
            applied(SHEET, 2.0).text_styles[&egui::TextStyle::Body].size,
            34.0
        );
    }

    #[test]
    fn extras_are_available_by_name() {
        let sheet = StyleSheet::parse(SHEET).unwrap();
        assert_eq!(sheet.extra("ok"), Some(Color32::from_rgb(0x80, 0x40, 0x90)));
        assert_eq!(sheet.extra("nope"), None);
        assert_eq!(sheet.parent(), Parent::Dark);
        assert_eq!(sheet.name, "Test");
    }

    /// Applying twice must give the same result as applying once — the whole point of assigning
    /// the parent preset rather than mutating whatever visuals were there.
    #[test]
    fn apply_is_idempotent() {
        let sheet = StyleSheet::parse(SHEET).unwrap();
        let mut a = egui::Style::default();
        sheet.apply(&mut a, 1.0).unwrap();
        let mut b = egui::Style::default();
        sheet.apply(&mut b, 1.0).unwrap();
        sheet.apply(&mut b, 1.0).unwrap();
        assert_eq!(a.visuals, b.visuals);
    }

    /// A typo must fail loudly: serde ignores unknown fields, so this is the one failure mode a
    /// hand-edited sheet cannot be allowed to have.
    #[test]
    fn a_misspelled_field_is_an_error() {
        let src = "parent = \"dark\"\n[visuals]\npanel_fil = \"#000000\"\n";
        let sheet = StyleSheet::parse(src).unwrap();
        let err = sheet.apply(&mut egui::Style::default(), 1.0).unwrap_err();
        assert!(matches!(err, Error::UnknownField(ref p) if p == "panel_fil"), "{err}");
        let nested = "parent = \"dark\"\n[visuals]\nwidgets.inactive.bg_filll = \"#000000\"\n";
        let err = StyleSheet::parse(nested)
            .unwrap()
            .apply(&mut egui::Style::default(), 1.0)
            .unwrap_err();
        assert!(err.to_string().contains("widgets.inactive.bg_filll"), "{err}");
    }

    #[test]
    fn bad_input_is_reported_not_ignored() {
        assert!(matches!(StyleSheet::parse("parent = \"blue\""), Err(Error::Parent(_))));
        assert!(matches!(StyleSheet::parse("["), Err(Error::Toml(_))));
        let bad_color = "parent = \"dark\"\n[visuals]\npanel_fill = \"c6c6c6\"\n";
        assert!(matches!(StyleSheet::parse(bad_color), Err(Error::Color { .. })));
        let missing_ref = "parent = \"dark\"\n[visuals]\npanel_fill = \"$nope\"\n";
        assert!(matches!(StyleSheet::parse(missing_ref), Err(Error::Color { .. })));
        // Right name, wrong shape → caught by the deserializer.
        let bad_shape = "parent = \"dark\"\n[visuals]\nwindow_shadow = 3\n";
        let err = StyleSheet::parse(bad_shape)
            .unwrap()
            .apply(&mut egui::Style::default(), 1.0)
            .unwrap_err();
        assert!(matches!(err, Error::Shape(_)), "{err}");
    }

    #[test]
    fn contrasting_ink_flips_with_the_plate() {
        for dark in [Color32::from_rgb(54, 96, 167), Color32::from_rgb(94, 94, 94), Color32::BLACK] {
            assert!(contrasting_text(dark).r() > 200, "{dark:?} needs light ink");
        }
        for pale in [Color32::from_rgb(255, 214, 10), Color32::from_rgb(160, 220, 160), Color32::WHITE] {
            assert!(contrasting_text(pale).r() < 60, "{pale:?} needs dark ink");
        }
    }
}
