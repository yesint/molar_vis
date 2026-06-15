//! Secondary-structure cartoon mesh.
//!
//! For each protein chain we run a Catmull-Rom spline through the Cα atoms,
//! build a per-residue orientation frame from the carbonyl direction (with
//! flip-consistency so the ribbon doesn't twist 180° between residues), and
//! extrude an elliptical cross-section along the spline. The cross-section
//! morphs by DSSP class: a thin tube for coil/turn, a wide flat ribbon for
//! helices and sheets. Each β-strand C-terminus gets an arrowhead: a sharp barb
//! flaring to a wide base, then a linear taper to a point at the strand's last
//! Cα. Helix and sheet Cα paths are Laplacian-smoothed first so ribbons run down
//! the axis instead of corkscrewing along the raw Cα trace.

use molar::prelude::*;

use std::collections::BTreeMap;

use super::MeshData;
use crate::color::Colorizer;
use crate::render::MeshVertex;
use crate::secstruct::{SsClass, SsMap};

/// Spline subdivisions per residue (along the backbone). Matches VMD's
/// `BONDRES` default of 12 — enough that the tight helix loops read as smooth.
const STEPS: usize = 12;
/// Vertices around each cross-section ring.
const RING: usize = 12;
/// Catmull-Rom tension VMD uses for NewCartoon (`create_modified_CR_spline_basis`
/// slope): tangent = (P₂−P₀)/slope. 1.25 (vs standard CR's 2.0) gives fuller,
/// rounder loops through the helix turns, which is most of the "smoothness".
const CR_SLOPE: f32 = 1.25;
/// Arrowhead length, in residues, measured back from the strand's last Cα.
const ARROW_LEN: f32 = 1.6;
/// Arrowhead base half-width, as a multiple of the ribbon half-width.
const ARROW_BASE_SCALE: f32 = 1.7;

struct Residue {
    ca: Vector3f,
    /// Carbonyl O position, if present (for the ribbon orientation).
    o: Option<Vector3f>,
    color: u32,
    class: SsClass,
    chain: char,
    resindex: usize,
}

pub fn build(
    bound: &impl ParticleIterProvider,
    colorizer: &Colorizer,
    ss: &SsMap,
    coil_radius: f32,
    ribbon_width: f32,
    ribbon_thickness: f32,
) -> MeshData {
    // Group atoms by residue (BTreeMap keeps ascending resindex order).
    struct Acc {
        ca: Option<(Vector3f, u32)>,
        o: Option<Vector3f>,
        chain: char,
    }
    let mut by_res: BTreeMap<usize, Acc> = BTreeMap::new();
    for p in bound.iter_particle() {
        let acc = by_res.entry(p.atom.resindex).or_insert(Acc {
            ca: None,
            o: None,
            chain: p.atom.chain,
        });
        match p.atom.name.as_str() {
            "CA" => acc.ca = Some((v3(p.pos), colorizer.color(p.atom, p.id))),
            "O" | "OT1" | "OXT" => {
                if acc.o.is_none() {
                    acc.o = Some(v3(p.pos));
                }
            }
            _ => {}
        }
    }

    // Keep only residues with a Cα (the spline control points).
    let residues: Vec<Residue> = by_res
        .into_iter()
        .filter_map(|(ri, a)| {
            a.ca.map(|(ca, color)| Residue {
                ca,
                o: a.o,
                color,
                class: ss.class(ri),
                chain: a.chain,
                resindex: ri,
            })
        })
        .collect();

    let shape = Shape {
        coil_radius,
        ribbon_width,
        ribbon_thickness,
    };
    let mut mesh = MeshData::default();

    // Split into runs of consecutive, same-chain residues (break on chain change
    // or a gap in resindex — i.e. a chain break / missing residues).
    let mut start = 0;
    while start < residues.len() {
        let mut end = start + 1;
        while end < residues.len()
            && residues[end].chain == residues[start].chain
            && residues[end].resindex == residues[end - 1].resindex + 1
        {
            end += 1;
        }
        build_run(&residues[start..end], &shape, &mut mesh);
        start = end;
    }
    mesh
}

/// Cross-section dimensions per DSSP class.
struct Shape {
    coil_radius: f32,
    ribbon_width: f32,
    ribbon_thickness: f32,
}

impl Shape {
    /// (half-width, half-thickness) of the cross-section for a class. Helix and
    /// sheet are the same flat ribbon (VMD); coil is a circular tube.
    fn dims(&self, class: SsClass) -> (f32, f32) {
        match class {
            SsClass::Helix | SsClass::Sheet => (self.ribbon_width, self.ribbon_thickness),
            SsClass::Coil => (self.coil_radius, self.coil_radius),
        }
    }
}

fn build_run(run: &[Residue], shape: &Shape, mesh: &mut MeshData) {
    let n = run.len();
    if n < 2 {
        return;
    }

    // Per-residue SS class, with single-residue helix/sheet runs demoted to coil
    // (they render as spurious stubs/arrows otherwise).
    let mut classes: Vec<SsClass> = run.iter().map(|r| r.class).collect();
    demote_singletons(&mut classes);

    // Spline control points. β-strands get their pleat zig-zag cancelled with a
    // weighted neighbor average `(2·CAᵢ + CAᵢ₋₁ + CAᵢ₊₁)/4` (VMD); helix and coil
    // keep the raw Cα — the smooth helix ribbon comes from the orientation, not
    // from moving the centerline.
    let coords: Vec<Vector3f> = (0..n)
        .map(|i| {
            if classes[i] == SsClass::Sheet {
                let prev = run[i.saturating_sub(1)].ca;
                let next = run[(i + 1).min(n - 1)].ca;
                run[i].ca * 0.5 + (prev + next) * 0.25
            } else {
                run[i].ca
            }
        })
        .collect();

    // Ribbon orientation ("perp"), exactly VMD's scheme: at each residue take the
    // in-plane normal of the peptide plane built from the *previous* carbonyl,
    // `D = (A×B)×A` with `A = CAᵢ−CAᵢ₋₁`, `B = Oᵢ₋₁−CAᵢ₋₁`; flip it to agree with
    // the running direction `g`, then fold it into a renormalized cumulative
    // average. The running average is what keeps the frame steady through a helix
    // (where `D` is tiny and noisy because the carbonyl runs along the axis).
    // NOTE on scale: this running average mixes the unit-length `g` with the raw
    // `D` (∝ length³), so the smoothing strength depends on the absolute |D|. VMD
    // is calibrated for Ångström coords (|D|~17 ≫ |g|=1, so D leads where it is
    // reliable — sheets — and `g` coasts through helices where D is tiny). Our
    // coords are nm, where |D|~0.02 ≪ 1 would freeze `g` (→ rippled helices,
    // mis-oriented sheets), so we scale the direction vectors to Ångström.
    const NM_TO_ANGSTROM: f32 = 10.0;
    let mut perps = vec![Vector3f::zeros(); n];
    let mut g = Vector3f::zeros();
    let mut last_ca = run[0].ca;
    let mut last_o = run[0].o.unwrap_or(run[0].ca);
    for i in 0..n {
        let ca = run[i].ca;
        let a = (ca - last_ca) * NM_TO_ANGSTROM;
        let b = (last_o - last_ca) * NM_TO_ANGSTROM;
        let d = a.cross(&b).cross(&a);
        let dd = if d.dot(&g) < 0.0 { -d } else { d };
        g = (g + dd).normalize_or_zero();
        perps[i] = g;
        last_ca = ca;
        last_o = run[i].o.unwrap_or(last_o);
    }
    perps[0] = perps[1]; // first residue had no previous data to work with

    // β-strand C-termini get an arrowhead. One (base, tip) span per contiguous
    // sheet run, in continuous residue coordinates (see `width_at`).
    let strands = arrow_regions(&classes);
    let arrow_base = shape.ribbon_width * ARROW_BASE_SCALE;

    let ctx = RunCtx {
        run,
        coords: &coords,
        perps: &perps,
        classes: &classes,
        shape,
        arrow_base,
        strands: &strands,
    };

    // Sample the spline into cross-section rings.
    let mut rings: Vec<Ring> = Vec::with_capacity((n - 1) * STEPS + 1);
    for i in 0..n - 1 {
        for s in 0..STEPS {
            rings.push(sample(&ctx, i, s as f32 / STEPS as f32));
        }
    }
    rings.push(sample(&ctx, n - 2, 1.0));

    emit(&rings, mesh);
}

/// Demote single-residue helix/sheet runs to coil.
fn demote_singletons(classes: &mut [SsClass]) {
    let n = classes.len();
    let mut i = 0;
    while i < n {
        let c = classes[i];
        let mut j = i + 1;
        while j < n && classes[j] == c {
            j += 1;
        }
        if c != SsClass::Coil && (j - i) < 2 {
            classes[i..j].fill(SsClass::Coil);
        }
        i = j;
    }
}

/// Contiguous β-strand runs → `(base, tip)` in continuous residue coordinates.
/// `tip` is the strand's last Cα (the arrow point); `base` is `ARROW_LEN`
/// residues before it (clamped to the strand start) where the barb flares out.
fn arrow_regions(classes: &[SsClass]) -> Vec<(f32, f32)> {
    let n = classes.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if classes[i] == SsClass::Sheet {
            let s = i;
            let mut e = i + 1;
            while e < n && classes[e] == SsClass::Sheet {
                e += 1;
            }
            let tip = (e - 1) as f32;
            let base = (tip - ARROW_LEN).max(s as f32);
            out.push((base, tip));
            i = e;
        } else {
            i += 1;
        }
    }
    out
}

/// Ribbon half-width at a class boundary (no arrow logic).
fn residue_width(c: &RunCtx, idx: usize) -> f32 {
    let idx = idx.min(c.classes.len() - 1);
    c.shape.dims(c.classes[idx]).0
}

/// Ribbon half-width at continuous residue coordinate `r`, arrowhead-aware.
/// Outside any arrow: linear blend of the class half-widths. Inside an arrow
/// span `[base, tip]`: a sharp barb (a width discontinuity at `base`, since
/// samples just below `base` keep the ribbon width) flaring to `arrow_base`,
/// then a linear taper to a point at `tip`; for the residue past `tip` the width
/// ramps from the point back up into the following coil tube.
fn width_at(c: &RunCtx, r: f32) -> f32 {
    let n = c.classes.len();
    for &(base, tip) in c.strands {
        if r >= base && r <= tip {
            let denom = (tip - base).max(1e-3);
            return (c.arrow_base * (tip - r) / denom).max(1e-3);
        }
        if r > tip && r < tip + 1.0 {
            // Arrow point (0) → following coil tube, over one residue.
            let next = residue_width(c, tip as usize + 1);
            return (next * (r - tip)).max(1e-3);
        }
    }
    let i = r.floor() as usize;
    let u = r - i as f32;
    let wa = residue_width(c, i);
    let wb = residue_width(c, (i + 1).min(n - 1));
    wa * (1.0 - u) + wb * u
}

/// Everything the per-sample cross-section builder needs for one run.
struct RunCtx<'a> {
    run: &'a [Residue],
    coords: &'a [Vector3f],
    perps: &'a [Vector3f],
    classes: &'a [SsClass],
    shape: &'a Shape,
    arrow_base: f32,
    /// β-strand arrow spans `(base, tip)`, in continuous residue coordinates.
    strands: &'a [(f32, f32)],
}

/// One cross-section: a center frame + ellipse dimensions + color.
struct Ring {
    center: Vector3f,
    tangent: Vector3f,
    width: Vector3f,
    normal: Vector3f,
    hw: f32,
    ht: f32,
    color: u32,
}

fn sample(c: &RunCtx, i: usize, u: f32) -> Ring {
    let n = c.coords.len();
    let p0 = c.coords[i.saturating_sub(1)];
    let p1 = c.coords[i];
    let p2 = c.coords[i + 1];
    let p3 = c.coords[(i + 2).min(n - 1)];

    let (center, tan) = cr_eval(p0, p1, p2, p3, u);
    let tangent = tan.normalize_or_zero();

    // Width axis = interpolated perp; thickness axis = tangent × perp (VMD's
    // updir). The perp need not be exactly ⟂ to the tangent — `normal` is ⟂ to
    // both, and the two cross-section axes (perp, normal) are mutually ⟂.
    let perp = (c.perps[i] * (1.0 - u) + c.perps[i + 1] * u).normalize_or_zero();
    let normal = tangent.cross(&perp).normalize_or_zero();

    let (_, ht_a) = c.shape.dims(c.classes[i]);
    let (_, ht_b) = c.shape.dims(c.classes[i + 1]);
    let ht = ht_a * (1.0 - u) + ht_b * u;
    // Arrowhead-aware ribbon half-width (β-strand barbs + taper to a point); see
    // `width_at`. Everything else is the original elliptical cross-section.
    let hw = width_at(c, i as f32 + u);

    Ring {
        center,
        tangent,
        width: perp,
        normal,
        hw,
        ht,
        color: lerp_color(c.run[i].color, c.run[i + 1].color, u),
    }
}

/// Turn the list of rings into a triangle mesh (ribbon body + end caps).
fn emit(rings: &[Ring], mesh: &mut MeshData) {
    let base = mesh.vertices.len() as u32;

    for r in rings {
        for k in 0..RING {
            let theta = std::f32::consts::TAU * k as f32 / RING as f32;
            let (s, c) = theta.sin_cos();
            let offset = r.width * (r.hw * c) + r.normal * (r.ht * s);
            // Analytic outward normal of the ellipse cross-section.
            let nrm = (r.width * (c / r.hw.max(1e-4)) + r.normal * (s / r.ht.max(1e-4)))
                .normalize_or_zero();
            mesh.vertices.push(MeshVertex {
                pos: (r.center + offset).to_array(),
                normal: nrm.to_array(),
                color: r.color,
                mat: 0,
            });
        }
    }

    // Quad strip between consecutive rings.
    for r in 0..rings.len() - 1 {
        let a0 = base + (r * RING) as u32;
        let b0 = base + ((r + 1) * RING) as u32;
        for k in 0..RING {
            let k2 = (k + 1) % RING;
            let a = a0 + k as u32;
            let b = a0 + k2 as u32;
            let c = b0 + k2 as u32;
            let d = b0 + k as u32;
            mesh.indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }

    // Flat end caps (fan around a center vertex).
    add_cap(mesh, rings.first().unwrap(), base, true);
    let last_base = base + ((rings.len() - 1) * RING) as u32;
    add_cap(mesh, rings.last().unwrap(), last_base, false);
}

fn add_cap(mesh: &mut MeshData, ring: &Ring, ring_base: u32, front: bool) {
    // Skip near-degenerate rings (e.g. an arrow point that lands on a run end):
    // their fan would be slivers with unstable normals and ~zero area anyway.
    if ring.hw < 1e-3 || ring.ht < 1e-3 {
        return;
    }
    let normal = if front { -ring.tangent } else { ring.tangent };
    let center_idx = mesh.vertices.len() as u32;
    mesh.vertices.push(MeshVertex {
        pos: ring.center.to_array(),
        normal: normal.to_array(),
        color: ring.color,
        mat: 0,
    });
    for k in 0..RING {
        let k2 = (k + 1) % RING;
        let a = ring_base + k as u32;
        let b = ring_base + k2 as u32;
        if front {
            mesh.indices.extend_from_slice(&[center_idx, b, a]);
        } else {
            mesh.indices.extend_from_slice(&[center_idx, a, b]);
        }
    }
}

//──────────────────────────────────────────────────────────────────────────────
// Math helpers
//──────────────────────────────────────────────────────────────────────────────

fn v3(p: &Pos) -> Vector3f {
    p.coords
}

/// Two small conveniences nalgebra's `Vector3` doesn't provide, used by the
/// spline/cross-section math: a zero-safe normalize and a plain `[f32; 3]`.
trait Vec3Ext {
    fn normalize_or_zero(self) -> Vector3f;
    fn to_array(self) -> [f32; 3];
}

impl Vec3Ext for Vector3f {
    fn normalize_or_zero(self) -> Vector3f {
        self.try_normalize(1e-9).unwrap_or_else(Vector3f::zeros)
    }
    fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }
}

/// Evaluate VMD's modified Catmull-Rom (slope `CR_SLOPE`) on the segment from
/// `p1` to `p2`, returning (position, tangent) at `w ∈ [0,1]`. Interpolates the
/// control points; the slope controls loop fullness. Matches VMD's
/// `create_modified_CR_spline_basis` + `make_spline_interpolation`.
fn cr_eval(p0: Vector3f, p1: Vector3f, p2: Vector3f, p3: Vector3f, w: f32) -> (Vector3f, Vector3f) {
    let is = 1.0 / CR_SLOPE;
    let q0 = p0 * (-is) + p1 * (2.0 - is) + p2 * (is - 2.0) + p3 * is; // w³
    let q1 = p0 * (2.0 * is) + p1 * (is - 3.0) + p2 * (3.0 - 2.0 * is) + p3 * (-is); // w²
    let q2 = p0 * (-is) + p2 * is; // w¹
    let q3 = p1; // w⁰
    let pos = ((q0 * w + q1) * w + q2) * w + q3;
    let tan = (q0 * (3.0 * w) + q1 * 2.0) * w + q2;
    (pos, tan)
}

fn lerp_color(a: u32, b: u32, t: f32) -> u32 {
    let mix = |sa: u32, sb: u32| -> u32 {
        let fa = sa as f32;
        let fb = sb as f32;
        (fa + (fb - fa) * t).round().clamp(0.0, 255.0) as u32
    };
    let chan = |c: u32, sh: u32| (c >> sh) & 0xff;
    mix(chan(a, 0), chan(b, 0))
        | (mix(chan(a, 8), chan(b, 8)) << 8)
        | (mix(chan(a, 16), chan(b, 16)) << 16)
        | (0xff << 24)
}
