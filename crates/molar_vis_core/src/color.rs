//! Color tables and per-scheme atom coloring.
//!
//! Colors are returned as RGBA8 and packed little-endian into a `u32` for use as
//! a per-instance vertex attribute (`r | g<<8 | b<<16 | a<<24`), matching the
//! `unpack_color` helper in the WGSL shaders.

use std::collections::HashMap;

use molar::prelude::*;

use crate::secstruct::{ss_color, SsMap};

/// CPK-style element colors as RGBA8, indexed by atomic number. Unknown elements
/// render magenta so they stand out.
pub fn element_color(atomic_number: u8) -> [u8; 4] {
    let rgb: [u8; 3] = match atomic_number {
        1 => [240, 240, 240],  // H  white
        6 => [144, 144, 144],  // C  grey
        7 => [48, 80, 248],    // N  blue
        8 => [255, 40, 40],    // O  red
        9 => [144, 224, 80],   // F  green
        11 => [171, 92, 242],  // Na violet
        12 => [138, 255, 0],   // Mg
        15 => [255, 128, 0],   // P  orange
        16 => [255, 220, 48],  // S  yellow
        17 => [31, 240, 31],   // Cl green
        19 => [143, 64, 212],  // K
        20 => [61, 255, 0],    // Ca
        26 => [224, 102, 51],  // Fe
        30 => [125, 128, 176], // Zn
        35 => [166, 41, 41],   // Br
        53 => [148, 0, 148],   // I
        _ => [255, 0, 255],    // unknown / unassigned
    };
    [rgb[0], rgb[1], rgb[2], 255]
}

/// Pack RGBA8 into a little-endian `u32` for upload as a vertex attribute.
pub fn pack_rgba8(c: [u8; 4]) -> u32 {
    (c[0] as u32) | ((c[1] as u32) << 8) | ((c[2] as u32) << 16) | ((c[3] as u32) << 24)
}

/// Distinct categorical palette (tab20-style) for chain/resid/resname coloring.
pub const PALETTE: [[u8; 3]; 12] = [
    [31, 119, 180],
    [255, 127, 14],
    [44, 160, 44],
    [214, 39, 40],
    [148, 103, 189],
    [140, 86, 75],
    [227, 119, 194],
    [188, 189, 34],
    [23, 190, 207],
    [255, 152, 150],
    [197, 176, 213],
    [152, 223, 138],
];

/// Pick a categorical color by key.
pub fn categorical(key: usize) -> [u8; 4] {
    let c = PALETTE[key % PALETTE.len()];
    [c[0], c[1], c[2], 255]
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

/// Red → white → blue ramp for a **signed** value `t` in [-1,1] (charge style): negative
/// red (electron-rich), zero white, positive blue. The chemistry convention, and the
/// reverse of [`beta_ramp`]'s low-to-high direction — a charge scale is diverging about a
/// meaningful zero rather than spanning an arbitrary range.
pub fn charge_ramp(t: f32) -> [u8; 4] {
    let t = t.clamp(-1.0, 1.0);
    let u = t.abs();
    let (r, g, b) = if t < 0.0 {
        (1.0, 1.0 - u, 1.0 - u) // → red
    } else {
        (1.0 - u, 1.0 - u, 1.0) // → blue
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, 255]
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
    /// Per-atom charge on a diverging red–white–blue scale, normalized by the largest
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

/// A color scheme together with the options that qualify it — currently just which
/// charge [`ColorMethod::Charge`] paints. Bundled so [`Colorizer`] and
/// [`crate::geometry::build`] take one value rather than growing a parameter per option.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ColorSpec {
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
        ColorSpec { method, charge_kind: ChargeKind::default() }
    }
}

/// A per-method atom colorizer. Holds any context needed (e.g. the B-factor range
/// of the selection, the atom count for the Index gradient) computed once.
pub struct Colorizer {
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
        let ColorSpec { method, charge_kind } = spec;
        let (beta_min, beta_inv_range) = if matches!(method, ColorMethod::Beta) {
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for a in src.iter_atoms() {
                lo = lo.min(a.get_bfactor());
                hi = hi.max(a.get_bfactor());
            }
            if lo.is_finite() {
                (lo, 1.0 / (hi - lo).max(1e-6))
            } else {
                (0.0, 0.0)
            }
        } else {
            (0.0, 0.0)
        };
        let ss_rgba = match (method, ss) {
            (ColorMethod::SecStruct, Some(m)) => {
                Some(m.entries().map(|(ri, s)| (ri, ss_color(s))).collect())
            }
            _ => None,
        };
        // Normalize by the largest magnitude present rather than a fixed range: partial
        // charges run to ~±0.8 e while formal ones are ±1..2, and either way the point is
        // to read the sign and the relative extremes of *this* selection.
        let charge_inv_max = if method.is_charge() {
            let max = src
                .iter_atoms()
                .map(|a| atom_charge(&a, charge_kind).abs())
                .fold(0.0f32, f32::max);
            if max > 1e-6 { 1.0 / max } else { 0.0 }
        } else {
            0.0
        };
        Self {
            method,
            charge_kind,
            inv_n: 1.0 / (n_atoms.max(1) as f32),
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
            ColorMethod::Element => element_color(atom.get_atomic_number()),
            ColorMethod::Chain => categorical(atom.get_chain() as usize),
            ColorMethod::ResId => categorical(atom.get_resid().rem_euclid(1 << 24) as usize),
            ColorMethod::ResName => categorical(hash_str(atom.get_resname())),
            ColorMethod::Index => rainbow(id as f32 * self.inv_n),
            ColorMethod::Beta => {
                beta_ramp((atom.get_bfactor() - self.beta_min) * self.beta_inv_range)
            }
            ColorMethod::SecStruct => self
                .ss_rgba
                .as_ref()
                .and_then(|m| m.get(&atom.get_resindex()).copied())
                .unwrap_or([230, 230, 230, 255]),
            ColorMethod::Solid(rgba) => rgba,
            ColorMethod::Charge => {
                charge_ramp(atom_charge(&atom, self.charge_kind) * self.charge_inv_max)
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

    /// Negative red, positive blue, zero white — diverging about a meaningful zero, the
    /// chemistry convention.
    #[test]
    fn charge_ramp_signs() {
        let red = charge_ramp(-1.0);
        assert!(red[0] > 200 && red[1] < 40 && red[2] < 40, "negative is red: {red:?}");
        let blue = charge_ramp(1.0);
        assert!(blue[2] > 200 && blue[0] < 40 && blue[1] < 40, "positive is blue: {blue:?}");
        assert_eq!(charge_ramp(0.0), [255, 255, 255, 255], "zero is white");
        // Saturating, not wrapping: past the poles the color holds.
        assert_eq!(charge_ramp(-4.0), red);
        assert_eq!(charge_ramp(4.0), blue);
    }

    /// The scale is normalized by the largest magnitude *in the selection*, so the extreme
    /// atoms always reach the poles whatever the absolute scale.
    #[test]
    fn charge_coloring_normalizes_to_the_extremes() {
        let sys = charged_system([-0.4, 0.2, 0.0], None);
        let c = colors(&sys, ColorSpec { method: ColorMethod::Charge, ..Default::default() });
        assert_eq!(c[0], charge_ramp(-1.0), "the most negative atom saturates red");
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
            ColorSpec { method: ColorMethod::Charge, charge_kind: ChargeKind::Partial },
        );
        let formal = colors(
            &sys,
            ColorSpec { method: ColorMethod::Charge, charge_kind: ChargeKind::Formal },
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
            ColorSpec { method: ColorMethod::Charge, charge_kind: ChargeKind::Formal },
        );
        assert!(c.iter().all(|&x| x == [255, 255, 255, 255]), "all neutral → all white");
    }
}
