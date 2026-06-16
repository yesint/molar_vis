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
}

/// Default color for the `Solid` scheme when first selected (VMD-ish orange).
pub const DEFAULT_SOLID: [u8; 4] = [255, 165, 0, 255];

impl ColorMethod {
    /// The picker entries, in order. `Solid` carries [`DEFAULT_SOLID`] here; the
    /// actual per-rep color is edited via the color-picker submenu.
    pub const ALL: [ColorMethod; 8] = [
        ColorMethod::Element,
        ColorMethod::Chain,
        ColorMethod::ResId,
        ColorMethod::ResName,
        ColorMethod::Index,
        ColorMethod::Beta,
        ColorMethod::SecStruct,
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
        }
    }

    /// Whether this scheme needs a DSSP pass (per-residue SS assignment).
    pub fn needs_ss(self) -> bool {
        matches!(self, ColorMethod::SecStruct)
    }
}

/// A per-method atom colorizer. Holds any context needed (e.g. the B-factor range
/// of the selection, the atom count for the Index gradient) computed once.
pub struct Colorizer {
    method: ColorMethod,
    inv_n: f32,
    beta_min: f32,
    beta_inv_range: f32,
    /// resindex → SS color, for `SecStruct` (precomputed from a DSSP pass).
    ss_rgba: Option<HashMap<usize, [u8; 4]>>,
}

impl Colorizer {
    /// `src` is the bound atoms being colored (used to derive the B-factor range);
    /// `n_atoms` is the molecule's total atom count (for the Index gradient).
    /// `ss` is a precomputed DSSP map, required only for `SecStruct`.
    pub fn new(
        method: ColorMethod,
        src: &impl AtomProvider,
        n_atoms: usize,
        ss: Option<&SsMap>,
    ) -> Self {
        let (beta_min, beta_inv_range) = if matches!(method, ColorMethod::Beta) {
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for a in src.iter_atoms() {
                lo = lo.min(a.bfactor);
                hi = hi.max(a.bfactor);
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
        Self {
            method,
            inv_n: 1.0 / (n_atoms.max(1) as f32),
            beta_min,
            beta_inv_range,
            ss_rgba,
        }
    }

    /// Packed RGBA8 color for an atom (`id` is its global atom index).
    pub fn color(&self, atom: &Atom, id: usize) -> u32 {
        let rgba = match self.method {
            ColorMethod::Element => element_color(atom.atomic_number),
            ColorMethod::Chain => categorical(atom.chain as usize),
            ColorMethod::ResId => categorical(atom.resid.rem_euclid(1 << 24) as usize),
            ColorMethod::ResName => categorical(hash_str(atom.resname.as_str())),
            ColorMethod::Index => rainbow(id as f32 * self.inv_n),
            ColorMethod::Beta => beta_ramp((atom.bfactor - self.beta_min) * self.beta_inv_range),
            ColorMethod::SecStruct => self
                .ss_rgba
                .as_ref()
                .and_then(|m| m.get(&atom.resindex).copied())
                .unwrap_or([230, 230, 230, 255]),
            ColorMethod::Solid(rgba) => rgba,
        };
        pack_rgba8(rgba)
    }
}
