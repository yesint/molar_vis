//! Persistent **program settings** — the application-level preferences and
//! new-document defaults that used to be hardcoded at launch (SSAA, the dark
//! theme + fonts, the default representation / color / material, the default
//! camera/view, bond-guessing thresholds, trajectory playback, mouse
//! sensitivity, default pick/selection modes).
//!
//! Design (mirrors [`crate::session`]): this module is **pure data + serde**,
//! WASM-safe. Every field is `#[serde(default)]` and each section has a `Default`
//! impl reproducing the *exact* previous hardcoded values — so a fresh config
//! reproduces the old behavior, and older/newer files still load (missing fields
//! default, unknown fields are ignored). The native file IO (`load_or_create` /
//! `save`, using the platform config dir) is the only `#[cfg(not(wasm))]` part;
//! the browser build keeps settings in memory.
//!
//! How each setting takes effect is wired up in `app.rs`/`theme.rs`/`render.rs`:
//! app-global knobs (theme, anti-aliasing) apply live; new-document defaults
//! (view, representation, bond/trajectory) are read when the next scene/molecule
//! is created — they never silently mutate the open document (see the View tab's
//! "Apply to current view" button for an explicit push).

use serde::{Deserialize, Serialize};

use crate::camera::{Ao, Background, Camera, DepthCue, Projection, Shadow};
use crate::color::ColorMethod;
use crate::data::BondParams;
use crate::geometry::RepKind;
use crate::material::Material;
use crate::pick::{PickMode, SelectionMode};
use crate::trajectory::LoopMode;

/// Tag identifying a settings file, so a stray JSON isn't mistaken for one.
pub const SETTINGS_FORMAT: &str = "molar_vis_settings";
/// Bumped only on a breaking schema change; reads stay tolerant via `serde(default)`.
pub const SETTINGS_VERSION: u32 = 1;

fn default_format() -> String {
    SETTINGS_FORMAT.to_string()
}
fn default_version() -> u32 {
    SETTINGS_VERSION
}

/// UI theme selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum ThemeMode {
    /// The custom high-contrast dark palette (the previous, only behavior).
    #[default]
    Dark,
    /// egui's built-in light visuals (plus the accent + font scale).
    Light,
    /// Follow the host/browser color-scheme preference.
    System,
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 3] = [ThemeMode::Dark, ThemeMode::Light, ThemeMode::System];
    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::Dark => "Dark",
            ThemeMode::Light => "Light",
            ThemeMode::System => "System",
        }
    }
}

/// Application UI look (theme.rs).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    pub theme: ThemeMode,
    /// Multiplier on the base font sizes (1.0 = the previous sizes).
    pub font_scale: f32,
    /// Selection / accent color, linear RGBA (matches `Background` color encoding;
    /// default = the previous `selection.bg_fill` sRGB(54,96,168) in linear space).
    pub accent: [f32; 4],
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Dark,
            font_scale: 1.0,
            accent: [0.037, 0.117, 0.392, 1.0],
        }
    }
}

/// Offscreen render quality (render.rs).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderingSettings {
    /// Supersampling factor (1 = off, 2 = the previous default, up to 4).
    pub ssaa: u32,
    /// Cast-shadow depth-map resolution (square), e.g. 1024 / 2048 / 4096.
    pub shadow_res: u32,
}

impl Default for RenderingSettings {
    fn default() -> Self {
        Self { ssaa: 2, shadow_res: 2048 }
    }
}

impl RenderingSettings {
    /// Clamp to sane bounds (the dialog offers presets, but a hand-edited file
    /// shouldn't be able to request a 64× target or a 0-pixel shadow map).
    pub fn sanitized(&self) -> Self {
        Self {
            ssaa: self.ssaa.clamp(1, 4),
            shadow_res: self.shadow_res.clamp(256, 8192),
        }
    }
}

/// Defaults applied to the camera/scene of a **newly created** document
/// (initial load, Open into an empty scene, New session). Loading a saved session
/// overrides these with the session's own view state.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewDefaults {
    pub projection: Projection,
    /// Fraction of the viewport the framed object fills along its longest
    /// dimension (the previous `FILL` constant).
    pub fill: f32,
    pub depth_cue: DepthCue,
    pub ao: Ao,
    pub shadow: Shadow,
    pub background: Background,
}

impl Default for ViewDefaults {
    fn default() -> Self {
        Self {
            projection: Projection::Orthographic,
            fill: 0.9,
            depth_cue: DepthCue::default(),
            ao: Ao::default(),
            shadow: Shadow::default(),
            background: Background::default(),
        }
    }
}

impl ViewDefaults {
    /// Stamp these view defaults onto `cam` (used when seeding a fresh scene's
    /// camera and by the dialog's "Apply to current view"). Leaves the framing
    /// (target/distance/orientation) alone — only the view-style knobs change.
    pub fn seed_camera(&self, cam: &mut Camera) {
        cam.projection = self.projection;
        cam.depth_cue = self.depth_cue;
        cam.ao = self.ao;
        cam.shadow = self.shadow;
        cam.background = self.background;
        cam.fill = self.fill;
    }
}

/// Defaults for a **newly created** representation (the initial rep of a loaded
/// molecule and the "add representation" button).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RepDefaults {
    pub kind: RepKind,
    pub color: ColorMethod,
    pub material: Material,
    pub selection: String,
    /// Default Surface grid-quality level (applied when `kind == Surface`).
    pub surface_quality: u32,
}

impl Default for RepDefaults {
    fn default() -> Self {
        Self {
            kind: RepKind::Lines,
            color: ColorMethod::Element,
            material: Material::Opaque,
            selection: "all".to_string(),
            surface_quality: 2,
        }
    }
}

/// Interaction + loading behavior (mouse, picking defaults, trajectory playback,
/// bond guessing).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorSettings {
    /// Multipliers on the built-in orbit/roll rates (1.0 = the previous feel).
    pub orbit_sensitivity: f32,
    pub roll_sensitivity: f32,
    /// Pick mode a fresh session starts in.
    pub pick_mode: PickMode,
    /// Selection-expansion mode a fresh session starts in.
    pub selection_mode: SelectionMode,
    /// Default trajectory playback rate (frames per second).
    pub traj_fps: f32,
    /// Default trajectory loop behavior.
    pub loop_mode: LoopMode,
    /// Bond-guessing thresholds (affect the next structure loaded).
    pub bond_factor: f32,
    pub bond_search_cutoff: f32,
    pub bond_min_dist: f32,
    /// **Periodic bond search**: find covalent bonds crossing a box face in a
    /// wrapped structure (minimum-image search). Off by default — the periodic
    /// search is much slower on large structures. Affects the next structure loaded.
    pub bond_search_periodic: bool,
    /// Draw bonds that wrap across a box face as **dashed minimum-image half-bonds**
    /// (and split cartoon ribbons at the boundary). Off → such bonds draw as plain
    /// solid half-bonds (a long line across the box). A render setting (applies live).
    pub dashed_pbc_bonds: bool,
}

impl Default for BehaviorSettings {
    fn default() -> Self {
        let b = BondParams::default();
        Self {
            orbit_sensitivity: 1.0,
            roll_sensitivity: 1.0,
            pick_mode: PickMode::Off,
            selection_mode: SelectionMode::Atoms,
            traj_fps: 15.0,
            loop_mode: LoopMode::Loop,
            bond_factor: b.factor,
            bond_search_cutoff: b.search_cutoff,
            bond_min_dist: b.min_dist,
            bond_search_periodic: b.periodic,
            dashed_pbc_bonds: true,
        }
    }
}

impl BehaviorSettings {
    /// Assemble the bond-guessing thresholds for `data::load_with`.
    pub fn bond_params(&self) -> BondParams {
        BondParams {
            factor: self.bond_factor,
            search_cutoff: self.bond_search_cutoff,
            min_dist: self.bond_min_dist,
            periodic: self.bond_search_periodic,
        }
    }
}

/// The whole persisted settings document.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub appearance: AppearanceSettings,
    #[serde(default)]
    pub rendering: RenderingSettings,
    #[serde(default)]
    pub view: ViewDefaults,
    #[serde(default)]
    pub reps: RepDefaults,
    #[serde(default)]
    pub behavior: BehaviorSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            format: default_format(),
            version: default_version(),
            appearance: AppearanceSettings::default(),
            rendering: RenderingSettings::default(),
            view: ViewDefaults::default(),
            reps: RepDefaults::default(),
            behavior: BehaviorSettings::default(),
        }
    }
}

// --- Native file IO (the platform config dir). WASM keeps settings in memory. ---

#[cfg(not(target_arch = "wasm32"))]
impl Settings {
    /// Path to the settings file in the platform config dir, e.g.
    /// `~/.config/molar_vis/settings.json` on Linux. `None` if the OS doesn't
    /// expose a config dir (then settings stay in-memory at defaults).
    pub fn config_path() -> Option<std::path::PathBuf> {
        directories::ProjectDirs::from("", "", "molar_vis")
            .map(|d| d.config_dir().join("settings.json"))
    }

    /// Read the settings file, creating it with defaults on first launch.
    /// A parse error is non-fatal: the bad file is renamed to `*.bak`, a fresh
    /// default file is written, and defaults are returned.
    pub fn load_or_create() -> Self {
        let Some(path) = Self::config_path() else {
            log::warn!("no platform config dir; using default settings (not persisted)");
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Settings>(&text) {
                Ok(s) => {
                    log::info!("loaded settings from {}", path.display());
                    s
                }
                Err(e) => {
                    let bak = path.with_extension("json.bak");
                    log::warn!(
                        "settings file {} is invalid ({e}); backing up to {} and resetting",
                        path.display(),
                        bak.display()
                    );
                    let _ = std::fs::rename(&path, &bak);
                    let s = Self::default();
                    let _ = s.save();
                    s
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let s = Self::default();
                match s.save() {
                    Ok(()) => log::info!("created default settings at {}", path.display()),
                    Err(e) => log::warn!("couldn't write default settings: {e}"),
                }
                s
            }
            Err(e) => {
                log::warn!("couldn't read settings ({e}); using defaults");
                Self::default()
            }
        }
    }

    /// Write the settings as pretty JSON, creating the config dir if needed.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_path().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no platform config dir")
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trips() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn defaults_match_previous_constants() {
        let s = Settings::default();
        // Rendering
        assert_eq!(s.rendering.ssaa, 2);
        assert_eq!(s.rendering.shadow_res, 2048);
        // Representation defaults
        assert_eq!(s.reps.kind, RepKind::Lines);
        assert_eq!(s.reps.color, ColorMethod::Element);
        assert_eq!(s.reps.material, Material::Opaque);
        assert_eq!(s.reps.selection, "all");
        // View defaults
        assert_eq!(s.view.projection, Projection::Orthographic);
        assert_eq!(s.view.fill, 0.9);
        assert_eq!(s.view.depth_cue, DepthCue::default());
        assert_eq!(s.view.ao, Ao::default());
        assert_eq!(s.view.shadow, Shadow::default());
        // Behavior + bond guessing
        assert_eq!(s.behavior.traj_fps, 15.0);
        assert_eq!(s.behavior.loop_mode, LoopMode::Loop);
        assert_eq!(s.behavior.pick_mode, PickMode::Off);
        assert_eq!(s.behavior.selection_mode, SelectionMode::Atoms);
        assert_eq!(s.behavior.bond_params(), BondParams::default());
    }

    #[test]
    fn missing_sections_default() {
        // An almost-empty file (forward/back compat): every absent field defaults.
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn unknown_fields_ignored_and_partial_applied() {
        // Unknown keys ignored; a partial section keeps its other fields at default.
        let json = r#"{
            "format": "molar_vis_settings",
            "rendering": { "ssaa": 4 },
            "behavior": { "traj_fps": 30.0 },
            "totally_unknown": 123
        }"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(s.rendering.ssaa, 4);
        assert_eq!(s.rendering.shadow_res, 2048); // untouched → default
        assert_eq!(s.behavior.traj_fps, 30.0);
        assert_eq!(s.behavior.bond_factor, BondParams::default().factor); // default
    }
}
