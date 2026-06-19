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

#[derive(Clone)]
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
    pbox: Option<&PeriodicBox>,
) -> MeshData {
    // Group atoms by residue (BTreeMap keeps ascending resindex order).
    struct Acc {
        ca: Option<(Vector3f, u32)>,
        o: Option<Vector3f>,
        chain: char,
    }
    let mut by_res: BTreeMap<usize, Acc> = BTreeMap::new();
    // Coarse-grained (Martini) input — backbone is `BB`, orientation from `SC1`.
    // The ribbon frame is built differently for CG (see `sample`): the side-chain
    // reference is the ribbon's *thin* axis, not its wide one.
    let mut cg = false;
    for p in bound.iter_particle() {
        let acc = by_res.entry(p.atom.resindex).or_insert(Acc {
            ca: None,
            o: None,
            chain: p.atom.chain,
        });
        match p.atom.name.as_str() {
            // Backbone trace point: atomistic Cα, or the Martini CG backbone bead.
            "CA" | "BB" => {
                cg |= p.atom.name.as_str() == "BB";
                acc.ca = Some((v3(p.pos), colorizer.color(p.atom, p.id)));
            }
            // Ribbon-orientation reference: the atomistic carbonyl O, or (for CG, which
            // has no carbonyl) the first side-chain bead SC1 — the BB→SC1 vector gives
            // the side-chain direction, which drives the ribbon's flat face the way the
            // carbonyl does for all-atom (`D = (A×B)×A` in `build_run`).
            "O" | "OT1" | "OXT" | "SC1" => {
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
    // or a gap in resindex — i.e. a chain break / missing residues) **and on a
    // periodic-box jump**: when consecutive Cα sit on opposite faces of the box
    // (a wrapped structure), the ribbon must not interpolate across the box. Such
    // a break is a *PBC break* — the chain actually continues across the boundary,
    // so the run end is faded out (vs. a hard cap at a real chain terminus).
    let mut start = 0;
    while start < residues.len() {
        let mut end = start + 1;
        while end < residues.len()
            && residues[end].chain == residues[start].chain
            && residues[end].resindex == residues[end - 1].resindex + 1
            && !is_pbc_jump(residues[end - 1].ca, residues[end].ca, pbox)
        {
            end += 1;
        }
        // The chain continues across this boundary (only a PBC jump split it) iff
        // the neighbour residue is the contiguous next/prev one but wrapped.
        let pbc_break = |i: usize, j: usize| {
            residues[j].chain == residues[i].chain
                && residues[j].resindex == residues[i].resindex + 1
                && is_pbc_jump(residues[i].ca, residues[j].ca, pbox)
        };
        // Build the run, **extending it one residue past each PBC-break end** with a
        // "ghost" control point at the across-boundary neighbour's nearest image.
        // The ribbon then runs out through the face toward where the partner really
        // is (like the dashed bonds) and is striped/faded only there — staying 100%
        // opaque up to the boundary. `fade_lo`/`fade_hi` = ghosts prepended/appended.
        let mut run_vec: Vec<Residue> = Vec::with_capacity(end - start + 2);
        let mut ext_lo = 0;
        if start > 0 && pbc_break(start - 1, start) {
            run_vec.push(ghost_of(&residues[start - 1], residues[start].ca, pbox));
            ext_lo = 1;
        }
        run_vec.extend_from_slice(&residues[start..end]);
        let mut ext_hi = 0;
        if end < residues.len() && pbc_break(end - 1, end) {
            run_vec.push(ghost_of(&residues[end], residues[end - 1].ca, pbox));
            ext_hi = 1;
        }
        build_run(&run_vec, &shape, ext_lo, ext_hi, pbox, cg, &mut mesh);
        start = end;
    }
    mesh
}

/// A spline control point placed at `neighbour`'s nearest periodic image to
/// `anchor`, copying its appearance. Used to extend a ribbon one residue past a
/// PBC break, out through the box face toward where the chain continues.
fn ghost_of(neighbour: &Residue, anchor: Vector3f, pbox: Option<&PeriodicBox>) -> Residue {
    let ca = match pbox {
        Some(b) => b
            .closest_image(
                &Pos::new(neighbour.ca.x, neighbour.ca.y, neighbour.ca.z),
                &Pos::new(anchor.x, anchor.y, anchor.z),
            )
            .coords,
        None => neighbour.ca,
    };
    Residue { ca, o: None, ..neighbour.clone() }
}

/// Whether consecutive Cα cross a periodic-box face — i.e. `next`'s nearest image
/// to `prev` is not `next` itself. `false` with no box.
fn is_pbc_jump(prev: Vector3f, next: Vector3f, pbox: Option<&PeriodicBox>) -> bool {
    let Some(b) = pbox else { return false };
    let prev_p = Pos::new(prev.x, prev.y, prev.z);
    let next_p = Pos::new(next.x, next.y, next.z);
    let img = b.closest_image(&next_p, &prev_p);
    let (dx, dy, dz) = (img.x - next.x, img.y - next.y, img.z - next.z);
    dx * dx + dy * dy + dz * dz > 1e-8
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

fn build_run(
    run: &[Residue],
    shape: &Shape,
    ext_lo: usize,
    ext_hi: usize,
    pbox: Option<&PeriodicBox>,
    cg: bool,
    mesh: &mut MeshData,
) {
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
    // keep the raw Cα — for all-atom the smooth helix ribbon comes from the carbonyl
    // orientation, not from moving the centerline.
    let mut coords: Vec<Vector3f> = (0..n)
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

    // CG-only per-residue data for the analytic helix ribbon (filled in the `if cg`
    // block below): the outward radial (= ribbon normal, also overrides `perps`), and
    // the smooth helix axis / transported frame e1 / spiral phase / ribbon radius that
    // let `sample` evaluate the helix **analytically** (a CR spline through only ~3.7
    // control points per turn overshoots and the turns collide).
    let mut cg_helix_normal = vec![Vector3f::zeros(); n];
    let mut cg_axis: Vec<Vector3f> = Vec::new();
    let mut cg_e1: Vec<Vector3f> = Vec::new();
    let mut cg_phi: Vec<f32> = Vec::new();
    let mut cg_radius: Vec<f32> = Vec::new();

    // CG helices: render a flat ribbon **wrapped onto the helix cylinder's surface**
    // (the recognisable α-helix cartoon). The recipe needs a clean, reliable helix
    // *axis* — which a CG backbone alone can't orient — so we build it in two steps:
    //
    //   1. Collapse the spiralling BB trace onto its **local axis** (windowed centroid
    //      over ~a turn + a Laplacian low-pass; helix-only, clamped to the run), giving
    //      a smooth (possibly wavy, for a bent helix) centerline.
    //   2. Put the ribbon centerline back on the **cylinder surface**: for each residue
    //      the outward **radial** (raw BB minus the axis, ⟂ the axis tangent) is the
    //      ribbon **normal** (flat face faces out); the centerline is the axis + a
    //      fixed radius (each helix's own mean BB-to-axis distance) along that radial.
    //      The radial rotates ~100°/residue, so the ribbon coils around the cylinder.
    //
    // Because the surface point sits ~one radius off the axis ≈ where the BB bead is,
    // the ribbon rejoins the raw-backbone coil smoothly at the helix ends (no axis-
    // offset kink). `cg_helix_normal` carries the radial into the `perps` override
    // below. All-atom keeps the raw Cα + carbonyl frame.
    if cg {
        let raw: Vec<Vector3f> = run.iter().map(|r| r.ca).collect();
        let mut axis = coords.clone(); // raw Cα at helix residues (sheet already smoothed)
        // 1a. windowed centroid → collapse the spiral onto the axis.
        const AXIS_W: usize = 3;
        for i in 0..n {
            if classes[i] != SsClass::Helix {
                continue;
            }
            let (rs, re) = helix_run_bounds(&classes, i);
            let lo = i.saturating_sub(AXIS_W).max(rs);
            let hi = (i + AXIS_W).min(re);
            let mut c = Vector3f::zeros();
            for s in &raw[lo..=hi] {
                c += *s;
            }
            axis[i] = c / (hi - lo + 1) as f32;
        }
        // 1b. low-pass the axis (the centroid leaves a faint residual wave); helix-only,
        // clamped to the run so the coil keeps its faithful raw backbone.
        const LP_PASSES: usize = 8;
        for _ in 0..LP_PASSES {
            let src = axis.clone();
            for i in 0..n {
                if classes[i] != SsClass::Helix {
                    continue;
                }
                let prev =
                    if i > 0 && classes[i - 1] == SsClass::Helix { src[i - 1] } else { src[i] };
                let next =
                    if i + 1 < n && classes[i + 1] == SsClass::Helix { src[i + 1] } else { src[i] };
                axis[i] = src[i] * 0.5 + (prev + next) * 0.25;
            }
        }
        // 2. per helix run: lay the ribbon centerline on a clean **uniform spiral**
        // around the smooth axis (see `cg_helix_ribbon`), recording the axis / frame /
        // phase / radius per residue for the analytic evaluation in `sample`.
        cg_e1 = vec![Vector3f::zeros(); n];
        cg_phi = vec![0.0; n];
        cg_radius = vec![0.0; n];
        cg_helix_ribbon(
            &classes,
            &axis,
            &raw,
            &mut coords,
            &mut cg_helix_normal,
            &mut cg_e1,
            &mut cg_phi,
            &mut cg_radius,
        );
        cg_axis = axis;
    }

    // Ribbon orientation ("perp"). The all-atom and CG paths differ fundamentally
    // (see `cg_perps`): all-atom uses VMD's renormalized cumulative-average of the
    // carbonyl-derived peptide-plane normal; CG uses the raw side-chain radial.
    let mut perps = if cg {
        cg_perps(run, &classes)
    } else {
        // VMD's scheme: at each residue take the in-plane normal of the peptide plane
        // built from the *previous* carbonyl, `D = (A×B)×A` with `A = CAᵢ−CAᵢ₋₁`,
        // `B = Oᵢ₋₁−CAᵢ₋₁`; flip it to agree with the running direction `g`, then fold
        // it into a renormalized cumulative average. NOTE on scale: this mixes the
        // unit `g` with raw `D` (∝ length³), so VMD is calibrated for Ångström coords
        // (|D|~17 ≫ |g|=1, so D leads in sheets and `g` coasts through helices where
        // the carbonyl is ~axial and D is tiny). Our coords are nm, so scale to Å.
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
        perps
    };
    // CG helix: use the axis-derived outward radial (computed above on the smooth axis)
    // as the ribbon normal — cleaner than `cg_perps`' per-residue radial, and it matches
    // the surface centerline so the flat face lies tangent to the cylinder.
    if cg {
        for i in 0..n {
            if classes[i] == SsClass::Helix && cg_helix_normal[i].norm() > 1e-6 {
                perps[i] = cg_helix_normal[i];
            }
        }
    }

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
        cg,
        axis: &cg_axis,
        e1: &cg_e1,
        phi: &cg_phi,
        radius: &cg_radius,
    };

    // Sample the spline into cross-section rings, tagging each with its nearest
    // residue's `resindex` (so the selection glow can extract just the sub-ribbon of
    // chosen residues from this exact mesh).
    let mut rings: Vec<Ring> = Vec::with_capacity((n - 1) * STEPS + 1);
    let mut ring_res: Vec<u32> = Vec::with_capacity((n - 1) * STEPS + 1);
    for i in 0..n - 1 {
        for s in 0..STEPS {
            let u = s as f32 / STEPS as f32;
            rings.push(sample(&ctx, i, u));
            let nearest = if u < 0.5 { i } else { i + 1 };
            ring_res.push(run[nearest].resindex as u32);
        }
    }
    rings.push(sample(&ctx, n - 2, 1.0));
    ring_res.push(run[n - 1].resindex as u32);

    // PBC-break ends: the ribbon is fully opaque up to the box face, then the
    // ghost extension *beyond* the face (rings whose center is outside the box) is
    // **dashed** — opaque stripe rings with transparent gap rings (matching the
    // dashed PBC bonds; no fade). `r = ring_index / STEPS` is the continuous
    // residue coordinate (0 … n-1); the ghost segments are `r < ext_lo` (start)
    // and `r > (n-1) - ext_hi` (end). Striping is per-ring (the finest the spline
    // sampling allows).
    if (ext_lo > 0 || ext_hi > 0) && pbox.is_some() {
        const STRIPE_RINGS: usize = 1;
        let b = pbox.unwrap();
        let max_r = (n - 1) as f32;
        for (idx, ring) in rings.iter_mut().enumerate() {
            let r = idx as f32 / STEPS as f32;
            let in_ghost = (ext_lo > 0 && r < ext_lo as f32)
                || (ext_hi > 0 && r > max_r - ext_hi as f32);
            if !in_ghost {
                continue; // a real residue → opaque
            }
            let center = Pos::new(ring.center.x, ring.center.y, ring.center.z);
            if b.is_inside(&center) {
                continue; // still inside the box → opaque (up to the face)
            }
            // Gap rings of the dash become transparent; stripe rings stay opaque.
            if (idx / STRIPE_RINGS) % 2 != 0 {
                ring.color &= 0x00ff_ffff;
            }
        }
    }

    emit(&rings, &ring_res, mesh);
}

/// The `[start, end]` residue index span of the contiguous helix run containing
/// `i` (which must itself be a helix residue).
fn helix_run_bounds(classes: &[SsClass], i: usize) -> (usize, usize) {
    let mut rs = i;
    while rs > 0 && classes[rs - 1] == SsClass::Helix {
        rs -= 1;
    }
    let mut re = i;
    while re + 1 < classes.len() && classes[re + 1] == SsClass::Helix {
        re += 1;
    }
    (rs, re)
}

/// An arbitrary unit vector perpendicular to `t` (for seeding a transport frame).
fn perp_any(t: Vector3f) -> Vector3f {
    let a = if t.x.abs() < 0.9 { Vector3f::x() } else { Vector3f::y() };
    t.cross(&a).normalize_or_zero()
}

/// Build the wrapping-ribbon centerline + outward normal for every **CG helix run**.
///
/// `axis` is the smooth helix centerline (collapsed + low-passed); `raw` the BB beads.
/// We lay the ribbon centerline on a **uniform spiral** around the axis rather than on
/// the raw beads: the raw per-residue radial jitters (thermal noise), which made the
/// ribbon wobble — turns came out non-uniform and overlapped where a wobble pulled two
/// turns together. Instead we build a **parallel-transported** frame `(e1, e2) ⟂ axis`
/// (no arbitrary twist), measure each bead's angular phase in it, **unwrap** it to a
/// monotonic progression and **low-pass** it (kills the jitter → uniform turns), then
/// place the centerline at `axis + radius·(cos φ·e1 + sin φ·e2)`. The radius is the
/// helix's own mean BB-to-axis distance scaled up (`RADIUS_SCALE`) toward the fatter
/// all-atom helix look. That outward radial is also the ribbon normal (flat face out).
#[allow(clippy::too_many_arguments)]
fn cg_helix_ribbon(
    classes: &[SsClass],
    axis: &[Vector3f],
    raw: &[Vector3f],
    coords: &mut [Vector3f],
    normal: &mut [Vector3f],
    e1_out: &mut [Vector3f],
    phi_out: &mut [f32],
    radius_out: &mut [f32],
) {
    use std::f32::consts::{PI, TAU};
    // CG BB sits ~0.18 nm off the axis; scale up to ~0.23 nm so the ribbon coils at the
    // same radius as an all-atom Cα helix (matching the AA cartoon — not fatter, which
    // blobs up when a turn is viewed face-on).
    const RADIUS_SCALE: f32 = 1.25;
    let n = classes.len();
    let mut i = 0;
    while i < n {
        if classes[i] != SsClass::Helix {
            i += 1;
            continue;
        }
        let (rs, re) = helix_run_bounds(classes, i);
        let m = re - rs + 1;

        // Local axis tangent per residue (windowed forward/back difference).
        let tang: Vec<Vector3f> = (rs..=re)
            .map(|k| {
                let prev = axis[k.saturating_sub(1).max(rs)];
                let next = axis[(k + 1).min(re)];
                (next - prev).normalize_or_zero()
            })
            .collect();

        // Parallel-transported frame e1 ⟂ tangent, seeded from the first bead's radial.
        let mut e1 = vec![Vector3f::zeros(); m];
        e1[0] = {
            let rr = raw[rs] - axis[rs];
            let v = rr - tang[0] * rr.dot(&tang[0]);
            if v.norm() < 1e-6 { perp_any(tang[0]) } else { v.normalize_or_zero() }
        };
        for k in 1..m {
            let t = tang[k];
            let v = e1[k - 1] - t * e1[k - 1].dot(&t);
            e1[k] = if v.norm() < 1e-6 { perp_any(t) } else { v.normalize_or_zero() };
        }

        // Measured phase of each bead in the transported frame, unwrapped to monotonic.
        let mut phi = vec![0f32; m];
        let mut sum_off = 0f32;
        for k in 0..m {
            let e2 = tang[k].cross(&e1[k]);
            let rr = raw[rs + k] - axis[rs + k];
            let rp = rr - tang[k] * rr.dot(&tang[k]);
            sum_off += rp.norm();
            let mut a = rp.dot(&e2).atan2(rp.dot(&e1[k]));
            if k > 0 {
                while a - phi[k - 1] > PI {
                    a -= TAU;
                }
                while a - phi[k - 1] < -PI {
                    a += TAU;
                }
            }
            phi[k] = a;
        }
        // Make the turns **uniform** (equal angular steps) by linearly interpolating the
        // phase — but **anchored to the actual measured phase at both ends**, not a
        // least-squares slope. A LS slope can place the first/last turn at an angle that
        // doesn't match the real backbone, landing the helix endpoint on the wrong side
        // of the cylinder; the coil then has to detour around it, which shows as a weird
        // ribbon "extension" jutting toward the neighbouring sheet/tube. Pinning the ends
        // to the measured phase keeps the endpoints on the real backbone while the
        // interior stays evenly spaced.
        if m >= 2 {
            let (p0, p1) = (phi[0], phi[m - 1]);
            for k in 0..m {
                phi[k] = p0 + (p1 - p0) * k as f32 / (m - 1) as f32;
            }
        }

        let radius = (sum_off / m as f32).max(0.05) * RADIUS_SCALE;
        for k in 0..m {
            let e2 = tang[k].cross(&e1[k]);
            let radial = (e1[k] * phi[k].cos() + e2 * phi[k].sin()).normalize_or_zero();
            normal[rs + k] = radial;
            coords[rs + k] = axis[rs + k] + radial * radius;
            // Record the frame/phase/radius so `cg_helix_sample` can evaluate the helix
            // analytically (avoiding CR overshoot through the sparse control points).
            e1_out[rs + k] = e1[k];
            phi_out[rs + k] = phi[k];
            radius_out[rs + k] = radius;
        }
        i = re + 1;
    }
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

/// CG ribbon half-width at residue `k`, **tapered toward the coil tube near a helix
/// run's ends** so the wide flat ribbon eases down into the thin loop tube instead of
/// staying full-size and ending abruptly. Non-helix residues just use their class width.
fn cg_res_width(c: &RunCtx, k: usize) -> f32 {
    let k = k.min(c.classes.len() - 1);
    if c.classes[k] != SsClass::Helix {
        return c.shape.dims(c.classes[k]).0;
    }
    // Smoothstep from the coil width at the run end up to the full ribbon width over
    // `TAPER` residues inward.
    const TAPER: f32 = 2.0;
    let (rs, re) = helix_run_bounds(c.classes, k);
    let d = (k - rs).min(re - k) as f32;
    let t = (d / TAPER).clamp(0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t);
    let coil = c.shape.coil_radius;
    coil + (c.shape.ribbon_width - coil) * s
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
    /// CG input: `perps` is the SC1-derived **thin** axis (radial/normal), so the
    /// wide axis is `tangent × perp` — a 90° swap from all-atom, where the carbonyl
    /// `perp` is the wide axis.
    cg: bool,
    /// CG analytic-helix data, per residue (empty when `!cg`): the smooth helix axis,
    /// the transported frame `e1` ⟂ axis, the unwrapped spiral phase, and the ribbon
    /// radius. Used only for helix-interior segments (see `cg_helix_sample`).
    axis: &'a [Vector3f],
    e1: &'a [Vector3f],
    phi: &'a [f32],
    radius: &'a [f32],
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

/// Cubic Hermite interpolation: position + tangent at `u ∈ [0,1]` of the curve from `p0`
/// (tangent `m0`) to `p1` (tangent `m1`).
fn hermite(p0: Vector3f, m0: Vector3f, p1: Vector3f, m1: Vector3f, u: f32) -> (Vector3f, Vector3f) {
    let (u2, u3) = (u * u, u * u * u);
    let h00 = 2.0 * u3 - 3.0 * u2 + 1.0;
    let h10 = u3 - 2.0 * u2 + u;
    let h01 = -2.0 * u3 + 3.0 * u2;
    let h11 = u3 - u2;
    let pos = p0 * h00 + m0 * h10 + p1 * h01 + m1 * h11;
    let (d00, d10, d01, d11) =
        (6.0 * u2 - 6.0 * u, 3.0 * u2 - 4.0 * u + 1.0, -6.0 * u2 + 6.0 * u, 3.0 * u2 - 2.0 * u);
    let tan = p0 * d00 + m0 * d10 + p1 * d01 + m1 * d11;
    (pos, tan)
}

/// Centerline + unit tangent for a **CG helix↔coil boundary** segment `(i, i+1)`. A plain
/// CR spline here takes its helix-side tangent from a control point ~one turn back on the
/// cylinder, so it swings backward and lays a second ribbon stub over the last turn. We
/// instead Hermite-interpolate with the **true spiral tangent** on the helix side (so the
/// ribbon flows out of the last turn) and a CR-style tangent on the coil side. Tangent
/// magnitudes mimic CR's `(p₂−p₀)/slope ≈ 1.6·chord` so the curve fullness matches.
fn cg_boundary_centerline(c: &RunCtx, i: usize, u: f32, n: usize) -> (Vector3f, Vector3f) {
    let p0 = c.coords[i];
    let p1 = c.coords[i + 1];
    let scale = (p1 - p0).norm().max(1e-4) * 2.0 / CR_SLOPE;
    let (m0, m1) = if c.classes[i] == SsClass::Helix {
        // helix → coil: spiral tangent leaving residue i (= end of segment i-1,i).
        let st = cg_helix_sample(c, i - 1, 1.0, n).tangent;
        let cnext = c.coords[(i + 2).min(n - 1)];
        (st * scale, (cnext - p0) * (1.0 / CR_SLOPE))
    } else {
        // coil → helix: spiral tangent entering residue i+1 (= start of segment i+1,i+2).
        let st = cg_helix_sample(c, i + 1, 0.0, n).tangent;
        let cprev = c.coords[i.saturating_sub(1)];
        ((p1 - cprev) * (1.0 / CR_SLOPE), st * scale)
    };
    let (pos, tan) = hermite(p0, m0, p1, m1, u);
    (pos, tan.normalize_or_zero())
}

/// One cross-section ring for a **CG helix-interior** segment, evaluated as an analytic
/// helix: a CR spline along the **smooth axis** (well-spaced points → no overshoot) plus
/// an analytic rotation around it (`radius·(cos φ·e1 + sin φ·e2)`, the frame/phase from
/// `cg_helix_ribbon`). The ribbon's broad face faces radially out (`normal = radial`),
/// the wide axis is the helix binormal (`tangent × radial`); the spiral tangent is the
/// axis tangent plus the circumferential term `radius·dφ`.
fn cg_helix_sample(c: &RunCtx, i: usize, u: f32, n: usize) -> Ring {
    // Clamp the axis spline to this helix's own residues — letting p0/p3 reach into the
    // flanking coil (whose `axis` is the raw backbone, off the helix axis) pulls the last
    // turn toward the coil and distorts it.
    let a1 = c.axis[i];
    let a2 = c.axis[i + 1];
    let a0 = if i > 0 && c.classes[i - 1] == SsClass::Helix { c.axis[i - 1] } else { a1 };
    let a3 = if i + 2 < n && c.classes[i + 2] == SsClass::Helix { c.axis[i + 2] } else { a2 };
    let (ac, at) = cr_eval(a0, a1, a2, a3, u);
    let tan_axis = at.normalize_or_zero();

    // Interpolate the transported frame across the segment, re-orthogonalize to the axis.
    let e1i = c.e1[i] * (1.0 - u) + c.e1[i + 1] * u;
    let e1u = {
        let e = e1i - tan_axis * e1i.dot(&tan_axis);
        if e.norm() < 1e-6 { c.e1[i] } else { e.normalize_or_zero() }
    };
    let e2u = tan_axis.cross(&e1u);

    let phi = c.phi[i] * (1.0 - u) + c.phi[i + 1] * u;
    let (sphi, cphi) = phi.sin_cos();
    let radial = (e1u * cphi + e2u * sphi).normalize_or_zero();
    let radius = c.radius[i] * (1.0 - u) + c.radius[i + 1] * u;
    let center = ac + radial * radius;

    // Spiral tangent = axis tangent + the circumferential term (d/du of the offset).
    let dphi = c.phi[i + 1] - c.phi[i];
    let circ = -e1u * sphi + e2u * cphi;
    let tangent = (at + circ * (radius * dphi)).normalize_or_zero();
    let width = tangent.cross(&radial).normalize_or_zero();

    Ring {
        center,
        tangent,
        width,
        normal: radial,
        // Taper toward the coil near the helix ends so the ribbon blends into the tube.
        hw: cg_res_width(c, i) * (1.0 - u) + cg_res_width(c, i + 1) * u,
        ht: c.shape.ribbon_thickness,
        color: lerp_color(c.run[i].color, c.run[i + 1].color, u),
    }
}

fn sample(c: &RunCtx, i: usize, u: f32) -> Ring {
    let n = c.coords.len();
    let helix_i = c.cg && c.classes[i] == SsClass::Helix;
    let helix_j = c.cg && c.classes[i + 1] == SsClass::Helix;
    // CG helix interior: evaluate the centerline as an **analytic helix** on the smooth
    // axis instead of CR-splining the surface control points (only ~3.7 per turn, so CR
    // overshoots and the turns collide).
    if helix_i && helix_j {
        return cg_helix_sample(c, i, u, n);
    }
    // The remaining work needs a centerline + tangent. A CG helix↔coil **boundary** uses
    // a Hermite whose helix-side tangent is the true spiral tangent (so the ribbon flows
    // smoothly into the coil tube rather than the CR spline swinging back over the last
    // turn — its tangent there came from a control point ~one turn back on the cylinder).
    let (center, tangent) = if helix_i || helix_j {
        cg_boundary_centerline(c, i, u, n)
    } else {
        let p0 = c.coords[i.saturating_sub(1)];
        let p1 = c.coords[i];
        let p2 = c.coords[i + 1];
        let p3 = c.coords[(i + 2).min(n - 1)];
        let (center, tan) = cr_eval(p0, p1, p2, p3, u);
        (center, tan.normalize_or_zero())
    };

    // Interpolated orientation reference + its tangent-perpendicular partner. All
    // mutually ⟂ (the reference need not be exactly ⟂ tangent; `cross` is ⟂ both).
    // All-atom: the carbonyl reference is the **wide** axis (width=reference). CG: the
    // SC1 reference is the **thin** axis (radial in a helix), so the axes swap — width
    // = tangent × reference — else the helix ribbon shows its edge and screws.
    let reference = (c.perps[i] * (1.0 - u) + c.perps[i + 1] * u).normalize_or_zero();
    let cross = tangent.cross(&reference).normalize_or_zero();
    // CG: `reference` is the radial (thin) axis, so the broad face faces outward like
    // the all-atom carbonyl ribbon → width = tangent × reference. All-atom: the
    // carbonyl `reference` is already the wide axis.
    let (perp, normal) = if c.cg { (cross, reference) } else { (reference, cross) };

    let (_, ht_a) = c.shape.dims(c.classes[i]);
    let (_, ht_b) = c.shape.dims(c.classes[i + 1]);
    let ht = ht_a * (1.0 - u) + ht_b * u;
    // Arrowhead-aware ribbon half-width (β-strand barbs + taper to a point); see
    // `width_at`. A CG helix↔coil boundary instead uses the helix-end-tapered width so
    // the ribbon blends smoothly into the coil tube (no arrows there).
    let hw = if helix_i || helix_j {
        cg_res_width(c, i) * (1.0 - u) + cg_res_width(c, i + 1) * u
    } else {
        width_at(c, i as f32 + u)
    };

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
/// `ring_res[i]` is the source residue of ring `i`, stamped onto every vertex of
/// that ring (and the adjacent end cap) into `mesh.vert_res`.
fn emit(rings: &[Ring], ring_res: &[u32], mesh: &mut MeshData) {
    let base = mesh.vertices.len() as u32;

    for (ri, r) in rings.iter().enumerate() {
        // A flat ribbon (helix/sheet: half-thickness ≪ half-width) is shaded as a true
        // **flat tape**: the two broad faces get a constant ±normal so they read flat,
        // and only the thin edge tips get the side normal (a crisp edge). An elliptical
        // normal here instead fans ~180° across the broad face → it shades like a domed
        // lens, which is what made foreshortened helix turns look like solid blobs.
        // Round cross-sections (coil tube: thickness ≈ width) keep the smooth ellipse.
        let flat = r.ht < 0.6 * r.hw;
        for k in 0..RING {
            let theta = std::f32::consts::TAU * k as f32 / RING as f32;
            let (s, c) = theta.sin_cos();
            let offset = r.width * (r.hw * c) + r.normal * (r.ht * s);
            let nrm = if flat {
                if s > 1e-4 {
                    r.normal
                } else if s < -1e-4 {
                    -r.normal
                } else {
                    r.width * c.signum() // edge tip
                }
            } else {
                (r.width * (c / r.hw.max(1e-4)) + r.normal * (s / r.ht.max(1e-4)))
                    .normalize_or_zero()
            };
            mesh.vertices.push(MeshVertex {
                pos: (r.center + offset).to_array(),
                normal: nrm.to_array(),
                color: r.color,
                mat: 0,
            });
            mesh.vert_res.push(ring_res[ri]);
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
    add_cap(mesh, rings.first().unwrap(), ring_res[0], base, true);
    let last_base = base + ((rings.len() - 1) * RING) as u32;
    add_cap(mesh, rings.last().unwrap(), ring_res[rings.len() - 1], last_base, false);
}

fn add_cap(mesh: &mut MeshData, ring: &Ring, res: u32, ring_base: u32, front: bool) {
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
    mesh.vert_res.push(res);
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

/// A unit vector ⟂ to a CG run's initial tangent, to seed the coasting ribbon
/// frame (so a chain that starts mid-helix isn't degenerate). Uses the first
/// residue's SC1 direction if it has a usable perpendicular component, else an
/// arbitrary perpendicular to the tangent.
fn seed_perp(run: &[Residue]) -> Vector3f {
    let t = if run.len() > 1 { run[1].ca - run[0].ca } else { Vector3f::x() };
    let t = t.normalize_or_zero();
    let reference = run[0].o.map(|o| o - run[0].ca).unwrap_or_else(Vector3f::x);
    let mut p = reference - t * reference.dot(&t);
    if p.norm() < 1e-6 {
        p = t.cross(&Vector3f::y());
    }
    if p.norm() < 1e-6 {
        p = t.cross(&Vector3f::x());
    }
    p.normalize_or_zero()
}

/// Per-residue ribbon orientation (the **thin**/normal axis — see `sample`'s `cg`
/// swap) for a **CG** run. The all-atom carbonyl machinery doesn't transfer, so:
///
/// - **Helix**: orient the normal **radially outward from the local helix axis** so
///   the flat face always faces out (and the ribbon coils cleanly instead of
///   screwing). The outward radial is the negative curvature `2·BBᵢ − BBᵢ₋₁ − BBᵢ₊₁`
///   (the chord midpoint sits toward the axis, so `BBᵢ` minus it points out),
///   projected ⟂ the **local axis direction** (a windowed tangent `BBᵢ₊₂ − BBᵢ₋₂`)
///   to strip the axial component. It rotates with the helix — that's intended.
/// - **Sheet**: the SC1 side-chain ≈ the sheet normal; use it, flipped to stay
///   consistent across the strand's alternating side chains.
/// - **Coil**: a tube (orientation invisible) — just parallel-transport for smoothness.
fn cg_perps(run: &[Residue], classes: &[SsClass]) -> Vec<Vector3f> {
    let n = run.len();
    let mut perps = vec![Vector3f::zeros(); n];
    let transport = |v: Vector3f, axis: Vector3f| (v - axis * v.dot(&axis)).normalize_or_zero();
    let mut prev = seed_perp(run);
    for i in 0..n {
        let ca = run[i].ca;
        let prev_ca = run[i.saturating_sub(1)].ca;
        let next_ca = run[(i + 1).min(n - 1)].ca;
        let mut cur = match classes[i] {
            SsClass::Helix => {
                // Fit the **local helix axis** over a window of ~one turn: the axis
                // direction is the windowed tangent and a point on it is the window
                // **centroid** (which sits on the axis once the window spans a turn).
                // The ribbon normal is the outward radial `BBᵢ − axis point` ⟂ the
                // axis — the flat face faces out and the ribbon coils cleanly. Built
                // from the smooth centroid (not per-residue curvature), so the radial
                // varies smoothly and points consistently outward → no sign flips, no
                // pinches, no continuity hacks needed.
                //
                // Clamp the window to **this helix's own residues** — letting it bleed
                // into the flanking coil skews the centroid off the axis and the radial
                // tilts, so the ribbon splays/“unwidens” at the helix ends.
                const W: usize = 7;
                let mut rs = i;
                while rs > 0 && classes[rs - 1] == SsClass::Helix {
                    rs -= 1;
                }
                let mut re = i;
                while re + 1 < n && classes[re + 1] == SsClass::Helix {
                    re += 1;
                }
                let lo = i.saturating_sub(W).max(rs);
                let hi = (i + W).min(re);
                let dir = (run[hi].ca - run[lo].ca).normalize_or_zero();
                let mut centroid = Vector3f::zeros();
                for r in &run[lo..=hi] {
                    centroid += r.ca;
                }
                centroid /= (hi - lo + 1) as f32;
                let radial = ca - centroid;
                let r = (radial - dir * radial.dot(&dir)).normalize_or_zero();
                let coast = transport(prev, dir);
                if r.norm() < 1e-4 {
                    coast
                } else {
                    // A *pure* radial normal tracks the helix at ~100°/residue, which
                    // reads as a candy-twist (and is spiky where the centroid is noisy).
                    // So blend mostly the **parallel-transported** (minimal-twist, smooth
                    // like the all-atom ribbon) frame with a light radial **anchor** that
                    // keeps the flat face biased outward instead of drifting to an
                    // arbitrary roll. `A` = the radial-anchor weight.
                    const A: f32 = 1.0;
                    (r * A + coast * (1.0 - A)).normalize_or_zero()
                }
            }
            SsClass::Sheet => {
                // SC1 ≈ the sheet normal; flip to stay consistent across the strand's
                // alternating side chains.
                let t = (next_ca - prev_ca).normalize_or_zero();
                let s = run[i].o.map(|o| o - ca).unwrap_or(prev);
                let mut nrm = (s - t * s.dot(&t)).normalize_or_zero();
                if nrm.norm() < 1e-6 {
                    nrm = transport(prev, t);
                }
                if nrm.dot(&prev) < 0.0 {
                    nrm = -nrm;
                }
                nrm
            }
            SsClass::Coil => {
                // A tube — orientation is invisible; just keep the frame smooth.
                let t = (next_ca - prev_ca).normalize_or_zero();
                transport(prev, t)
            }
        };
        if cur.norm() < 1e-6 {
            cur = if prev.norm() > 1e-6 { prev } else { seed_perp(run) };
        }
        perps[i] = cur;
        prev = cur;
    }
    perps
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
