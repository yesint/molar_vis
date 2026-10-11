//! Color tables and per-scheme atom coloring.
//!
//! Colors are returned as RGBA8 and packed little-endian into a `u32` for use as
//! a per-instance vertex attribute (`r | g<<8 | b<<16 | a<<24`), matching the
//! `unpack_color` helper in the WGSL shaders.

use std::collections::HashMap;

use molar::prelude::*;

use crate::secstruct::{ss_color, SsMap};

/// Editable colors shared by every representation and persisted in settings/sessions.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ColorPreferences {
    /// Only overrides are stored; unmodified elements keep the built-in palette.
    pub elements: std::collections::BTreeMap<u8, [u8; 4]>,
    /// Alpha, 3₁₀, pi helices; sheet, bridge, turn, bend, polyproline, coil.
    pub secondary: [[u8; 4]; 9],
    #[serde(alias = "beta")]
    pub continuous: Gradient,
    pub categorical: Vec<[u8; 4]>,
}

pub const SS_TYPES: [SS; 9] = [
    SS::AlphaHelix,
    SS::Helix310,
    SS::PiHelix,
    SS::BetaSheet,
    SS::BetaBridge,
    SS::Turn,
    SS::Bend,
    SS::PolyProline,
    SS::Coil,
];

impl Default for ColorPreferences {
    fn default() -> Self {
        Self {
            elements: Default::default(),
            secondary: SS_TYPES.map(ss_color),
            continuous: Gradient::default(),
            categorical: PALETTE.to_vec(),
        }
    }
}

impl ColorPreferences {
    pub fn element(&self, number: u8) -> [u8; 4] {
        self.elements
            .get(&number)
            .copied()
            .unwrap_or_else(|| element_color(number))
    }

    pub fn discrete_palette(&self) -> &[[u8; 4]] {
        if self.categorical.is_empty() {
            &PALETTE
        } else {
            &self.categorical
        }
    }

    pub fn categorical(&self, key: usize) -> [u8; 4] {
        let palette = self.discrete_palette();
        palette[key % palette.len()]
    }

    pub fn secondary(&self, ss: SS) -> [u8; 4] {
        let index = SS_TYPES.iter().position(|&s| s == ss).unwrap_or(8);
        self.secondary[index]
    }
}

/// Three color stops at the low endpoint, midpoint, and high endpoint.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Gradient {
    #[serde(default = "default_midpoint")]
    pub midpoint: f32,
    pub stops: [[u8; 4]; 3],
}

fn default_midpoint() -> f32 {
    0.5
}

impl Default for Gradient {
    fn default() -> Self {
        Self {
            midpoint: 0.5,
            stops: [beta_ramp(0.0), beta_ramp(0.5), beta_ramp(1.0)],
        }
    }
}

impl Gradient {
    pub fn midpoint(&self) -> f32 {
        if self.midpoint.is_finite() {
            self.midpoint.clamp(0.01, 0.99)
        } else {
            0.5
        }
    }
    pub fn sample(&self, t: f32) -> [u8; 4] {
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.5
        };
        let mid = self.midpoint();
        let (a, b, u) = if t < mid {
            (self.stops[0], self.stops[1], t / mid)
        } else {
            (self.stops[1], self.stops[2], (t - mid) / (1.0 - mid))
        };
        std::array::from_fn(|i| ((1.0 - u) * a[i] as f32 + u * b[i] as f32) as u8)
    }
}

/// CPK-style element colors as RGBA8, indexed by atomic number. Unknown elements
/// render magenta so they stand out.
pub fn element_color(atomic_number: u8) -> [u8; 4] {
    let rgb: [u8; 3] = match atomic_number {
        1 => [240, 240, 240],  // H  white
        2 => [217, 255, 255],  // He
        3 => [204, 128, 255],  // Li
        4 => [194, 255, 0],    // Be
        5 => [255, 181, 181],  // B
        6 => [144, 144, 144],  // C  grey
        7 => [48, 80, 248],    // N  blue
        8 => [255, 40, 40],    // O  red
        9 => [144, 224, 80],   // F  green
        10 => [179, 227, 245], // Ne
        11 => [171, 92, 242],  // Na violet
        12 => [138, 255, 0],   // Mg
        13 => [191, 166, 166], // Al
        14 => [240, 200, 160], // Si
        15 => [255, 128, 0],   // P  orange
        16 => [255, 220, 48],  // S  yellow
        17 => [31, 240, 31],   // Cl green
        18 => [128, 209, 227], // Ar
        19 => [143, 64, 212],  // K
        20 => [61, 255, 0],    // Ca
        21 => [230, 230, 230], // Sc
        22 => [191, 194, 199], // Ti
        23 => [166, 166, 171], // V
        24 => [138, 153, 199], // Cr
        25 => [156, 122, 199], // Mn
        26 => [224, 102, 51],  // Fe
        27 => [240, 144, 160], // Co
        28 => [80, 208, 80],   // Ni
        29 => [200, 128, 51],  // Cu
        30 => [125, 128, 176], // Zn
        31 => [194, 143, 143], // Ga
        32 => [102, 143, 143], // Ge
        33 => [189, 128, 227], // As
        34 => [255, 161, 0],   // Se
        35 => [166, 41, 41],   // Br
        36 => [92, 184, 209],  // Kr
        37 => [112, 46, 176],  // Rb
        38 => [0, 255, 0],    // Sr
        39 => [148, 255, 255], // Y
        40 => [148, 224, 224], // Zr
        41 => [115, 194, 201], // Nb
        42 => [84, 181, 181],  // Mo
        43 => [59, 158, 158],  // Tc
        44 => [36, 143, 143],  // Ru
        45 => [10, 125, 140],  // Rh
        46 => [0, 105, 133],   // Pd
        47 => [192, 192, 192], // Ag
        48 => [255, 217, 143], // Cd
        49 => [166, 117, 115], // In
        50 => [102, 128, 128], // Sn
        51 => [158, 99, 181],  // Sb
        52 => [212, 122, 0],   // Te
        53 => [148, 0, 148],   // I
        54 => [66, 158, 176],  // Xe
        _ => [255, 0, 255],    // unknown / unassigned
    };
    [rgb[0], rgb[1], rgb[2], 255]
}

/// Pack RGBA8 into a little-endian `u32` for upload as a vertex attribute.
pub fn pack_rgba8(c: [u8; 4]) -> u32 {
    (c[0] as u32) | ((c[1] as u32) << 8) | ((c[2] as u32) << 16) | ((c[3] as u32) << 24)
}

/// Default discrete colors, also used by solid-color picker presets.
pub const PALETTE: [[u8; 4]; 18] = [
    [230, 50, 50, 255],   // red
    [255, 140, 0, 255],   // orange
    [240, 220, 40, 255],  // yellow
    [60, 200, 80, 255],   // green
    [60, 200, 210, 255],  // cyan
    [60, 110, 240, 255],  // blue
    [150, 80, 220, 255],  // purple
    [220, 60, 200, 255],  // magenta
    [240, 130, 180, 255], // pink
    [30, 150, 150, 255],  // teal
    [170, 220, 40, 255],  // lime
    [40, 60, 150, 255],   // navy
    [245, 245, 245, 255], // white
    [200, 200, 200, 255], // light grey
    [140, 140, 140, 255], // grey
    [80, 80, 80, 255],    // dark grey
    [25, 25, 25, 255],    // near-black
    [150, 90, 50, 255],   // brown
];

/// Pick a categorical color by key.
pub fn categorical(key: usize) -> [u8; 4] {
    PALETTE[key % PALETTE.len()]
}

fn hash_str(s: &str) -> usize {
    s.bytes()
        .fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize))
}

/// HSV→RGB rainbow for `t` in [0,1] (red → green → blue across ~300°).
pub fn rainbow(t: f32) -> [u8; 4] {
    let h = t.clamp(0.0, 1.0) * 300.0 / 60.0; // hue sector 0..5
    let (s, v) = (0.65_f32, 0.95_f32);
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
        255,
    ]
}

/// Blue → white → red ramp for a normalized value `t` in [0,1] (B-factor style).
pub fn beta_ramp(t: f32) -> [u8; 4] {
    let t = t.clamp(0.0, 1.0);
    let (r, g, b) = if t < 0.5 {
        let u = t * 2.0;
        (u, u, 1.0)
    } else {
        let u = (t - 0.5) * 2.0;
        (1.0, 1.0 - u, 1.0 - u)
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, 255]
}

/// Blue → white → red for signed charges: negative blue, positive red.
pub fn charge_ramp(t: f32) -> [u8; 4] {
    beta_ramp((t.clamp(-1.0, 1.0) + 1.0) * 0.5)
}

/// An atom's charge of the requested kind. A missing `formal_charge` column reads as 0,
/// so a structure without formal charges paints uniformly neutral rather than failing.
pub fn atom_charge(atom: &impl AtomLike, kind: ChargeKind) -> f32 {
    match kind {
        ChargeKind::Partial => atom.get_charge(),
        ChargeKind::Formal => atom.get_formal_charge().unwrap_or(0) as f32,
    }
}

/// Which of an atom's two charges the `Charge` scheme paints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ChargeKind {
    /// The working/partial charge (molar's always-present `charge` column) — read from a
    /// topology that carries one, or computed on demand (espaloma; see `crate::charges`).
    #[default]
    Partial,
    /// The integer formal charge (molar's optional `formal_charge` column, e.g. from an
    /// SDF `M  CHG` record). Absent on most structures, hence "if they exist".
    Formal,
}

impl ChargeKind {
    pub const ALL: [ChargeKind; 2] = [ChargeKind::Partial, ChargeKind::Formal];

    pub fn label(self) -> &'static str {
        match self {
            ChargeKind::Partial => "Partial",
            ChargeKind::Formal => "Formal",
        }
    }
}

/// How atoms are colored. Secondary-structure coloring lands with M6 (DSSP).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ColorMethod {
    Element,
    Chain,
    ResId,
    ResName,
    Index,
    Beta,
    SecStruct,
    /// A single user-chosen RGBA color for the whole selection.
    Solid([u8; 4]),
    /// Per-atom charge on the shared continuous scale (blue–white–red by default), normalized by the largest
    /// magnitude in the selection so the sign is always readable. *Which* charge —
    /// partial or formal — is an **option** of this method, not a second method: it lives
    /// on the representation ([`crate::scene::Representation::charge_kind`]) and is edited
    /// in the rep settings' `Color` tab, alongside computing partial charges on demand.
    Charge,
}

/// Default color for the `Solid` scheme when first selected (VMD-ish orange).
pub const DEFAULT_SOLID: [u8; 4] = [255, 165, 0, 255];

impl ColorMethod {
    /// The picker entries, in order. `Solid` carries [`DEFAULT_SOLID`] here; the
    /// actual per-rep color is edited via the color-picker submenu.
    pub const ALL: [ColorMethod; 9] = [
        ColorMethod::Element,
        ColorMethod::Chain,
        ColorMethod::ResId,
        ColorMethod::ResName,
        ColorMethod::Index,
        ColorMethod::Beta,
        ColorMethod::SecStruct,
        ColorMethod::Charge,
        ColorMethod::Solid(DEFAULT_SOLID),
    ];

    pub fn label(self) -> &'static str {
        match self {
            ColorMethod::Element => "Element",
            ColorMethod::Chain => "Chain",
            ColorMethod::ResId => "ResID",
            ColorMethod::ResName => "ResName",
            ColorMethod::Index => "Index",
            ColorMethod::Beta => "B-factor",
            ColorMethod::SecStruct => "Structure",
            ColorMethod::Solid(_) => "Solid",
            ColorMethod::Charge => "Charge",
        }
    }

    /// Whether this scheme needs a DSSP pass (per-residue SS assignment).
    pub fn needs_ss(self) -> bool {
        matches!(self, ColorMethod::SecStruct)
    }

    /// Whether this scheme paints charges (and so has the `Color` tab's options).
    pub fn is_charge(self) -> bool {
        matches!(self, ColorMethod::Charge)
    }
}

/// A color scheme with charge options and the shared editable palette.
/// Bundled so [`Colorizer`] and
/// [`crate::geometry::build`] take one value rather than growing a parameter per option.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ColorSpec {
    pub preferences: std::sync::Arc<ColorPreferences>,
    pub method: ColorMethod,
    pub charge_kind: ChargeKind,
}

impl Default for ColorMethod {
    fn default() -> Self {
        ColorMethod::Element
    }
}

impl From<ColorMethod> for ColorSpec {
    /// A scheme with default options — for the internal builders (glow, hover lens) that
    /// hard-code a method and never paint charges.
    fn from(method: ColorMethod) -> Self {
        ColorSpec { method, ..Default::default() }
    }
}

/// Numeric endpoints used by rendering and by the property scale legends.
pub fn continuous_range(
    method: ColorMethod,
    kind: ChargeKind,
    src: &impl AtomProvider,
    n_atoms: usize,
) -> Option<[f32; 2]> {
    match method {
        ColorMethod::Beta => {
            let mut low = f32::INFINITY;
            let mut high = f32::NEG_INFINITY;
            for atom in src.iter_atoms() {
                let value = atom.get_bfactor();
                if value.is_finite() {
                    low = low.min(value);
                    high = high.max(value);
                }
            }
            Some(if low.is_finite() {
                [low, high]
            } else {
                [0.0; 2]
            })
        }
        ColorMethod::Charge => {
            let max = src
                .iter_atoms()
                .map(|atom| atom_charge(&atom, kind).abs())
                .filter(|value| value.is_finite())
                .fold(0.0_f32, f32::max);
            Some(if max > 0.0 { [-max, max] } else { [0.0; 2] })
        }
        ColorMethod::Index => Some([0.0, n_atoms.saturating_sub(1) as f32]),
        _ => None,
    }
}

/// A per-method atom colorizer. Holds any context needed (e.g. the B-factor range
/// of the selection, the atom count for the Index gradient) computed once.
pub struct Colorizer {
    preferences: std::sync::Arc<ColorPreferences>,
    method: ColorMethod,
    /// Which charge `Charge` paints (ignored by every other method).
    charge_kind: ChargeKind,
    inv_n: f32,
    beta_min: f32,
    beta_inv_range: f32,
    /// resindex → SS color, for `SecStruct` (precomputed from a DSSP pass).
    ss_rgba: Option<HashMap<usize, [u8; 4]>>,
    /// `1 / max|q|` over the selection, for `Charge`: the scale is diverging about zero,
    /// so one symmetric factor is enough (0 when every charge is zero → all white).
    charge_inv_max: f32,
}

impl Colorizer {
    /// `src` is the bound atoms being colored (used to derive the B-factor range);
    /// `n_atoms` is the molecule's total atom count (for the Index gradient).
    /// `ss` is a precomputed DSSP map, required only for `SecStruct`.
    pub fn new(
        spec: ColorSpec,
        src: &impl AtomProvider,
        n_atoms: usize,
        ss: Option<&SsMap>,
    ) -> Self {
        let ColorSpec { method, charge_kind, preferences } = spec;
        let range = continuous_range(method, charge_kind, src, n_atoms).unwrap_or([0.0; 2]);
        let beta_min = range[0];
        let beta_inv_range = 1.0 / (range[1] - range[0]).max(1e-6);
        let ss_rgba = match (method, ss) {
            (ColorMethod::SecStruct, Some(m)) => {
                Some(m.entries().map(|(ri, s)| (ri, preferences.secondary(s))).collect())
            }
            _ => None,
        };
        // Normalize by the largest magnitude present rather than a fixed range: partial
        // charges run to ~±0.8 e while formal ones are ±1..2, and either way the point is
        // to read the sign and the relative extremes of *this* selection.
        let charge_inv_max = if method.is_charge() && range[1] > 1e-6 { 1.0 / range[1] } else { 0.0 };
        Self {
            preferences,
            method,
            charge_kind,
            inv_n: 1.0 / (n_atoms.saturating_sub(1).max(1) as f32),
            beta_min,
            beta_inv_range,
            ss_rgba,
            charge_inv_max,
        }
    }

    /// Packed RGBA8 color for an atom (`id` is its global atom index). Takes the atom
    /// by value: molar hands out `AtomRef` column proxies (a `Copy` two-word handle),
    /// not `&Atom`.
    pub fn color(&self, atom: impl AtomLike, id: usize) -> u32 {
        let rgba = match self.method {
            ColorMethod::Element => self.preferences.element(atom.get_atomic_number()),
            ColorMethod::Chain => self.preferences.categorical(atom.get_chain() as usize),
            ColorMethod::ResId => self.preferences.categorical(atom.get_resid().rem_euclid(1 << 24) as usize),
            ColorMethod::ResName => self.preferences.categorical(hash_str(atom.get_resname())),
            ColorMethod::Index => self.preferences.continuous.sample(id as f32 * self.inv_n),
            ColorMethod::Beta => {
                self.preferences.continuous.sample((atom.get_bfactor() - self.beta_min) * self.beta_inv_range)
            }
            ColorMethod::SecStruct => self
                .ss_rgba
                .as_ref()
                .and_then(|m| m.get(&atom.get_resindex()).copied())
                .unwrap_or(self.preferences.secondary[8]),
            ColorMethod::Solid(rgba) => rgba,
            ColorMethod::Charge => {
                self.preferences.continuous.sample((atom_charge(&atom, self.charge_kind) * self.charge_inv_max + 1.0) * 0.5)
            }
        };
        pack_rgba8(rgba)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A three-atom system whose partial and formal charges differ, so a test can tell
    /// which one a scheme actually read.
    fn charged_system(partial: [f32; 3], formal: Option<[i32; 3]>) -> System {
        let mut top = Topology::default();
        for (i, name) in ["O", "H1", "H2"].iter().enumerate() {
            let mut a = Atom::new().with_name(name).with_resname("MOL").with_resid(1).guess();
            a.charge = partial[i];
            a.formal_charge = formal.map(|f| f[i]);
            top.atoms.push(&a);
        }
        top.assign_resindex();
        let st = State {
            coords: (0..3).map(|i| Pos::new(i as f32 * 0.1, 0.0, 0.0)).collect(),
            ..Default::default()
        };
        System::new(top, st).unwrap()
    }

    fn colors(sys: &System, spec: ColorSpec) -> Vec<[u8; 4]> {
        let bound = sys.select_all_bound();
        let c = Colorizer::new(spec, &bound, 3, None);
        bound
            .iter_particle()
            .map(|p| {
                let packed = c.color(p.atom, p.id);
                [
                    (packed & 0xff) as u8,
                    ((packed >> 8) & 0xff) as u8,
                    ((packed >> 16) & 0xff) as u8,
                    ((packed >> 24) & 0xff) as u8,
                ]
            })
            .collect()
    }

    /// Negative blue, positive red, and zero white by default.
    #[test]
    fn charge_ramp_signs() {
        let red = charge_ramp(1.0);
        assert!(red[0] > 200 && red[1] < 40 && red[2] < 40, "positive is red: {red:?}");
        let blue = charge_ramp(-1.0);
        assert!(blue[2] > 200 && blue[0] < 40 && blue[1] < 40, "negative is blue: {blue:?}");
        assert_eq!(charge_ramp(0.0), [255, 255, 255, 255], "zero is white");
        // Saturating, not wrapping: past the poles the color holds.
        assert_eq!(charge_ramp(4.0), red);
        assert_eq!(charge_ramp(-4.0), blue);
    }

    /// The scale is normalized by the largest magnitude *in the selection*, so the extreme
    /// atoms always reach the poles whatever the absolute scale.
    #[test]
    fn charge_coloring_normalizes_to_the_extremes() {
        let sys = charged_system([-0.4, 0.2, 0.0], None);
        let c = colors(&sys, ColorSpec { method: ColorMethod::Charge, ..Default::default() });
        assert_eq!(c[0], charge_ramp(-1.0), "the most negative atom saturates blue");
        assert_eq!(c[1], charge_ramp(0.5), "half the magnitude → half the ramp");
        assert_eq!(c[2], charge_ramp(0.0), "an uncharged atom is white");

        // Ten times smaller charges give exactly the same picture.
        let small = charged_system([-0.04, 0.02, 0.0], None);
        assert_eq!(colors(&small, ColorSpec { method: ColorMethod::Charge, ..Default::default() }), c);
    }

    /// Partial and formal are two *options* of the one Charge scheme, reading different
    /// columns of the same atoms.
    #[test]
    fn charge_kind_selects_which_charge_is_read() {
        // Partial charges say atom 0 is the negative one; formal charges say atom 2 is.
        let sys = charged_system([-0.4, 0.0, 0.0], Some([0, 0, -1]));
        let partial = colors(
            &sys,
            ColorSpec { method: ColorMethod::Charge, charge_kind: ChargeKind::Partial, ..Default::default() },
        );
        let formal = colors(
            &sys,
            ColorSpec { method: ColorMethod::Charge, charge_kind: ChargeKind::Formal, ..Default::default() },
        );
        assert_eq!(partial[0], charge_ramp(-1.0));
        assert_eq!(partial[2], charge_ramp(0.0));
        assert_eq!(formal[0], charge_ramp(0.0));
        assert_eq!(formal[2], charge_ramp(-1.0));
    }

    /// "Formal charges, if they exist": a structure without the optional column paints
    /// uniformly neutral instead of failing.
    #[test]
    fn absent_formal_charges_paint_neutral() {
        let sys = charged_system([-0.4, 0.2, 0.0], None);
        let c = colors(
            &sys,
            ColorSpec { method: ColorMethod::Charge, charge_kind: ChargeKind::Formal, ..Default::default() },
        );
        assert!(c.iter().all(|&x| x == [255, 255, 255, 255]), "all neutral → all white");
    }
}

#[cfg(test)]
mod preference_tests {
    use super::*;

    #[test]
    fn custom_colors_reach_atoms_and_diverging_palettes() {
        let mut top = Topology::default();
        for (i, charge) in [-0.4, 0.0, 0.4].into_iter().enumerate() {
            let mut atom = Atom::new()
                .with_name("C")
                .with_resname("MOL")
                .with_resid(1)
                .guess();
            atom.charge = charge;
            atom.bfactor = i as f32 * 10.0;
            top.atoms.push(&atom);
        }
        top.assign_resindex();
        let state = State {
            coords: (0..3).map(|i| Pos::new(i as f32, 0.0, 0.0)).collect(),
            ..Default::default()
        };
        let system = System::new(top, state).unwrap();
        let bound = system.select_all_bound();
        assert_eq!(
            continuous_range(ColorMethod::Beta, ChargeKind::Partial, &bound, 3),
            Some([0.0, 20.0])
        );
        assert_eq!(
            continuous_range(ColorMethod::Charge, ChargeKind::Partial, &bound, 3),
            Some([-0.4, 0.4])
        );
        assert_eq!(
            continuous_range(ColorMethod::Index, ChargeKind::Partial, &bound, 3),
            Some([0.0, 2.0])
        );
        let mut prefs = ColorPreferences::default();
        let custom = [12, 180, 42, 255];
        prefs.elements.insert(6, custom);
        prefs.secondary[8] = custom;
        prefs.continuous.stops = [[180, 30, 70, 255], [90, 100, 110, 255], [40, 180, 90, 255]];
        let prefs = std::sync::Arc::new(prefs);
        for (method, expected) in [
            (ColorMethod::Element, [custom; 3]),
            (ColorMethod::SecStruct, [custom; 3]),
            (ColorMethod::Beta, prefs.continuous.stops),
            (ColorMethod::Index, prefs.continuous.stops),
            (ColorMethod::Charge, prefs.continuous.stops),
        ] {
            let colorizer = Colorizer::new(
                ColorSpec {
                    method,
                    preferences: prefs.clone(),
                    ..Default::default()
                },
                &bound,
                3,
                None,
            );
            let actual: Vec<_> = bound
                .iter_particle()
                .map(|p| colorizer.color(p.atom, p.id))
                .collect();
            assert_eq!(actual, expected.map(pack_rgba8), "{method:?}");
        }
    }

    #[test]
    fn editable_categorical_palette_colors_residues_names_and_chains() {
        let mut top = Topology::default();
        let mut atom = Atom::new()
            .with_name("C")
            .with_resname("MOL")
            .with_resid(1)
            .guess();
        atom.chain = 'A';
        top.atoms.push(&atom);
        top.assign_resindex();
        let system = System::new(
            top,
            State {
                coords: vec![Pos::new(0.0, 0.0, 0.0)],
                ..Default::default()
            },
        )
        .unwrap();
        let bound = system.select_all_bound();
        let red = [180, 30, 60, 255];
        let green = [20, 180, 40, 255];
        let mut prefs = ColorPreferences::default();
        assert_eq!(prefs.categorical.len(), 18);
        assert_eq!(
            prefs
                .categorical
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            18
        );
        prefs.categorical = vec![red, green];
        for (method, expected) in [
            (ColorMethod::Chain, green),
            (ColorMethod::ResId, green),
            (ColorMethod::ResName, red),
        ] {
            let colorizer = Colorizer::new(
                ColorSpec {
                    method,
                    preferences: std::sync::Arc::new(prefs.clone()),
                    ..Default::default()
                },
                &bound,
                1,
                None,
            );
            let particle = bound.iter_particle().next().unwrap();
            assert_eq!(
                colorizer.color(particle.atom, particle.id),
                pack_rgba8(expected)
            );
        }
        prefs.categorical.remove(0);
        assert_eq!(prefs.categorical(20), green);
        prefs.categorical.push(red);
        prefs.categorical[0] = [10, 20, 30, 255];
        assert_eq!(prefs.categorical(0), [10, 20, 30, 255]);
        assert_eq!(prefs.categorical(1), red);
        prefs.categorical.clear();
        assert_eq!(prefs.categorical(0), categorical(0));
    }

    #[test]
    fn previous_bfactor_palette_migrates_to_the_shared_gradient() {
        let previous = r#"{"beta":{"midpoint":0.25,"stops":[[10,20,30,255],[40,50,60,255],[70,80,90,255]]},"charge":{"stops":[[255,0,0,255],[255,255,255,255],[0,0,255,255]]}}"#;
        let prefs: ColorPreferences = serde_json::from_str(previous).unwrap();
        assert_eq!(prefs.continuous.midpoint, 0.25);
        assert_eq!(prefs.continuous.stops[0], [10, 20, 30, 255]);
        assert_eq!(prefs.categorical.len(), 18);
        let saved = serde_json::to_value(&prefs).unwrap();
        assert!(saved.get("continuous").is_some());
        assert!(saved.get("beta").is_none() && saved.get("charge").is_none());
    }

    #[test]
    fn session_and_saved_settings_roundtrip_colors_and_accept_older_files() {
        let mut settings = crate::settings::Settings::default();
        settings.colors.elements.insert(8, [10, 20, 30, 255]);
        settings.colors.secondary[0] = [40, 50, 60, 255];
        settings.colors.continuous.stops[1] = [70, 80, 90, 255];
        settings.colors.continuous.midpoint = 0.7;
        settings.colors.continuous.stops[2] = [100, 110, 120, 255];
        settings.colors.categorical[0] = [12, 34, 56, 255];
        settings.colors.categorical.push([100, 150, 200, 255]);
        let encoded = serde_json::to_string(&settings).unwrap();
        let restored: crate::settings::Settings = serde_json::from_str(&encoded).unwrap();
        assert_eq!(restored.colors, settings.colors);
        let view = crate::session::ViewState {
            colors: Some(settings.colors.clone()),
            ..Default::default()
        };
        let restored: crate::session::ViewState =
            serde_json::from_str(&serde_json::to_string(&view).unwrap()).unwrap();
        assert_eq!(restored.colors, Some(settings.colors));
        let old: crate::settings::Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.colors, ColorPreferences::default());
        let old: crate::session::ViewState = serde_json::from_str("{}").unwrap();
        assert!(old.colors.is_none());
    }

    #[test]
    fn secondary_structure_overrides_cover_every_assignment() {
        let mut prefs = ColorPreferences::default();
        for (i, ss) in SS_TYPES.into_iter().enumerate() {
            prefs.secondary[i] = [i as u8, 42, 90, 255];
            assert_eq!(prefs.secondary(ss), prefs.secondary[i]);
        }
        assert_eq!(prefs.secondary(SS::Break), prefs.secondary[8]);
    }

    #[test]
    fn movable_midpoint_affects_interpolation_and_old_palettes_default_to_center() {
        let mut ramp = ColorPreferences::default().continuous;
        ramp.midpoint = 0.25;
        assert_eq!(ramp.sample(0.25), ramp.stops[1]);
        assert_eq!(ramp.sample(0.125), [127, 127, 255, 255]);
        assert_eq!(ramp.sample(0.625), [255, 127, 127, 255]);
        let old: Gradient =
            serde_json::from_str(r#"{"stops":[[0,0,255,255],[255,255,255,255],[255,0,0,255]]}"#)
                .unwrap();
        assert_eq!(old.midpoint, 0.5);
        ramp.midpoint = 0.0;
        assert_eq!(ramp.midpoint(), 0.01);
        ramp.midpoint = f32::NAN;
        assert_eq!(ramp.midpoint(), 0.5);
    }

    #[test]
    fn gradient_interpolates_clamps_and_preserves_all_three_stops() {
        let ramp = Gradient {
            midpoint: 0.5,
            stops: [[0, 20, 40, 255], [100, 120, 140, 255], [200, 220, 240, 255]],
        };
        assert_eq!(ramp.sample(-2.0), ramp.stops[0]);
        assert_eq!(ramp.sample(0.25), [50, 70, 90, 255]);
        assert_eq!(ramp.sample(0.5), ramp.stops[1]);
        assert_eq!(ramp.sample(0.75), [150, 170, 190, 255]);
        assert_eq!(ramp.sample(2.0), ramp.stops[2]);
        assert_eq!(ramp.sample(f32::NAN), ramp.stops[1]);
    }
}
