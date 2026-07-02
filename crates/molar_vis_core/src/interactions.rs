//! Detection of non-covalent interactions between two atom sets, à la PLIP /
//! Discovery Studio. Pure logic, WASM-safe — no molar or GPU dependency: the caller
//! (`app::build::build_interactions`) gathers two [`InteractionSet`]s (one per partner
//! representation, read from each molecule's displayed frame + topology) and this
//! returns the interaction lines.
//!
//! Types detected (PLIP thresholds, `plip/basic/config.py`):
//! - **Hydrogen bonds** — donor/acceptor N/O/S; with an explicit H, D–A `< 4.1 Å` **and**
//!   D–H···A angle `> 100°`, else a heavy-atom D–A `≤ 3.5 Å` fallback (structures without H).
//! - **Hydrophobic** — carbons with only C/H neighbours, `< 4.0 Å`, one per residue pair.
//! - **Salt bridges** — opposite-charge group centroids `< 5.5 Å`.
//! - **π-stacking** — two aromatic ring centroids `< 5.5 Å` with parallel or T-shaped planes.
//! - **π-cation** — aromatic ring centroid ↔ cationic centroid `< 6.0 Å`.
//! - **Halogen bonds** — C–X (Cl/Br/I) ··· acceptor (N/O/S) `< 4.0 Å`, C–X···A angle `> 140°`.
//!
//! Atom-level detection (H-bond / hydrophobic / halogen) is **grid-based**
//! (`spatial::AtomGrid`) — never an O(N·M) double loop. Group-level detection (salt
//! bridge / π) is over the small ring / charge-group lists (O(n²), n tiny).

use crate::spatial::AtomGrid;
use glam::Vec3;
use std::collections::BTreeMap;

/// Smallest separation treated as a real contact (nm) — excludes coincident / bonded
/// atoms (PLIP `MIN_DIST`).
const MIN_DIST: f32 = 0.05;

/// One heavy atom, annotated with everything the atom-level detectors need. `res_key`
/// must be **globally unique per (molecule, residue)** across both sets so residue-level
/// dedup doesn't merge same-index residues from different molecules (the caller packs
/// the molecule index into it). Hydrogens are not entries — their positions ride in
/// their heavy neighbour's `attached_h`.
pub struct AtomInfo {
    pub pos: Vec3,
    /// Atomic number: 6=C, 7=N, 8=O, 16=S, 17/35/53 = Cl/Br/I.
    pub atomicnum: u8,
    pub res_key: u64,
    /// True for a carbon whose bonded neighbours are all C/H → a hydrophobic atom.
    pub only_ch_neighbors: bool,
    /// Positions of the hydrogens bonded to this atom (drives the H-bond angle test).
    pub attached_h: Vec<Vec3>,
    /// A bonded heavy neighbour's position (the C for a halogen — the C–X vector for the
    /// halogen-bond angle). `None` if the atom has no heavy neighbour.
    pub antecedent: Option<Vec3>,
}

/// An aromatic ring, reduced to its centroid + plane normal (unit) for π interactions.
pub struct RingInfo {
    pub center: Vec3,
    pub normal: Vec3,
    pub res_key: u64,
}

/// A formal-charge group (protein sidechain / ligand functional group), reduced to the
/// centroid of its charged atoms.
pub struct ChargeGroup {
    pub center: Vec3,
    pub res_key: u64,
}

/// Everything one representation contributes to interaction detection.
#[derive(Default)]
pub struct InteractionSet {
    pub atoms: Vec<AtomInfo>,
    pub rings: Vec<RingInfo>,
    pub cations: Vec<ChargeGroup>,
    pub anions: Vec<ChargeGroup>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InteractionKind {
    HBond,
    Hydrophobic,
    SaltBridge,
    PiStacking,
    PiCation,
    Halogen,
}

/// One detected interaction, as a line segment between two atoms / group centroids (nm).
pub struct Interaction {
    pub kind: InteractionKind,
    pub a: Vec3,
    pub b: Vec3,
}

/// Per-type detection cutoffs (nm / degrees). Derived from the user-facing
/// [`InteractionSettings`]; the angle thresholds are internal constants.
#[derive(Clone, Copy, Debug)]
pub struct DetectParams {
    pub hbonds: bool,
    pub hbond_dist_h: f32,
    pub hbond_dist_heavy: f32,
    pub hbond_angle_min: f32,
    pub hydrophobic: bool,
    pub hydrophobic_dist: f32,
    pub salt_bridges: bool,
    pub salt_bridge_dist: f32,
    pub pi_stacking: bool,
    pub pi_stacking_dist: f32,
    pub pi_stacking_angle: f32,
    pub pi_stacking_offset: f32,
    pub pi_cation: bool,
    pub pi_cation_dist: f32,
    pub pi_cation_offset: f32,
    pub halogen: bool,
    pub halogen_dist: f32,
    pub halogen_angle_min: f32,
}

/// The user-editable interaction settings carried by an `Interactions` rep
/// (`RepParams::Interactions`). Each type has an enable flag + a distance cutoff (nm);
/// `line_width` is the dashed-line width (px). `Copy` + serde so it rides `RepParams`
/// through undo/redo and sessions. Convert to [`DetectParams`] with [`Self::detect`].
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InteractionSettings {
    // --- Hydrogen bonds ---
    #[serde(default = "yes")]
    pub hbonds: bool,
    /// Heavy-atom donor–acceptor distance cutoff (nm) — used when no explicit H.
    #[serde(default = "d_hbond")]
    pub hbond_dist: f32,
    /// Donor–acceptor distance cutoff (nm) applied when the donor bears an explicit H.
    #[serde(default = "d_hbond_h")]
    pub hbond_dist_h: f32,
    /// Minimum donor–H···acceptor angle (degrees), when an H is present.
    #[serde(default = "d_hbond_angle")]
    pub hbond_angle: f32,
    // --- Hydrophobic ---
    #[serde(default = "yes")]
    pub hydrophobic: bool,
    #[serde(default = "d_hydrophobic")]
    pub hydrophobic_dist: f32,
    // --- Salt bridges ---
    #[serde(default = "yes")]
    pub salt_bridges: bool,
    #[serde(default = "d_salt")]
    pub salt_bridge_dist: f32,
    // --- π-stacking ---
    #[serde(default = "yes")]
    pub pi_stacking: bool,
    #[serde(default = "d_pi_stack")]
    pub pi_stacking_dist: f32,
    /// Angle tolerance (degrees): ring planes within this of parallel → parallel stack;
    /// within this of perpendicular → T-shaped stack.
    #[serde(default = "d_pi_angle")]
    pub pi_stacking_angle: f32,
    /// Max lateral offset (nm) of the two ring centroids for a parallel stack.
    #[serde(default = "d_pi_offset")]
    pub pi_stacking_offset: f32,
    // --- π-cation ---
    #[serde(default = "yes")]
    pub pi_cation: bool,
    #[serde(default = "d_pi_cation")]
    pub pi_cation_dist: f32,
    /// Max lateral offset (nm) of the cation from the ring axis.
    #[serde(default = "d_pi_offset")]
    pub pi_cation_offset: f32,
    // --- Halogen bonds ---
    #[serde(default = "yes")]
    pub halogen: bool,
    #[serde(default = "d_halogen")]
    pub halogen_dist: f32,
    /// Minimum C–X···acceptor angle (degrees).
    #[serde(default = "d_halogen_angle")]
    pub halogen_angle: f32,
    // --- Rendering ---
    #[serde(default = "d_width")]
    pub line_width: f32,
}

// Free fns so each field can carry a `#[serde(default)]` (older sessions load).
fn yes() -> bool {
    true
}
fn d_hbond() -> f32 {
    0.35
}
fn d_hbond_h() -> f32 {
    0.41
}
fn d_hbond_angle() -> f32 {
    100.0
}
fn d_hydrophobic() -> f32 {
    0.40
}
fn d_salt() -> f32 {
    0.55
}
fn d_pi_stack() -> f32 {
    0.55
}
fn d_pi_angle() -> f32 {
    30.0
}
fn d_pi_offset() -> f32 {
    0.20
}
fn d_pi_cation() -> f32 {
    0.60
}
fn d_halogen() -> f32 {
    0.40
}
fn d_halogen_angle() -> f32 {
    140.0
}
fn d_width() -> f32 {
    3.0
}

impl Default for InteractionSettings {
    fn default() -> Self {
        Self {
            hbonds: true,
            hbond_dist: d_hbond(),
            hbond_dist_h: d_hbond_h(),
            hbond_angle: d_hbond_angle(),
            hydrophobic: true,
            hydrophobic_dist: d_hydrophobic(),
            salt_bridges: true,
            salt_bridge_dist: d_salt(),
            pi_stacking: true,
            pi_stacking_dist: d_pi_stack(),
            pi_stacking_angle: d_pi_angle(),
            pi_stacking_offset: d_pi_offset(),
            pi_cation: true,
            pi_cation_dist: d_pi_cation(),
            pi_cation_offset: d_pi_offset(),
            halogen: true,
            halogen_dist: d_halogen(),
            halogen_angle: d_halogen_angle(),
            line_width: d_width(),
        }
    }
}

impl InteractionSettings {
    /// The detection cutoffs (a direct 1:1 mapping — every threshold is user-editable).
    pub fn detect(&self) -> DetectParams {
        DetectParams {
            hbonds: self.hbonds,
            hbond_dist_h: self.hbond_dist_h,
            hbond_dist_heavy: self.hbond_dist,
            hbond_angle_min: self.hbond_angle,
            hydrophobic: self.hydrophobic,
            hydrophobic_dist: self.hydrophobic_dist,
            salt_bridges: self.salt_bridges,
            salt_bridge_dist: self.salt_bridge_dist,
            pi_stacking: self.pi_stacking,
            pi_stacking_dist: self.pi_stacking_dist,
            pi_stacking_angle: self.pi_stacking_angle,
            pi_stacking_offset: self.pi_stacking_offset,
            pi_cation: self.pi_cation,
            pi_cation_dist: self.pi_cation_dist,
            pi_cation_offset: self.pi_cation_offset,
            halogen: self.halogen,
            halogen_dist: self.halogen_dist,
            halogen_angle_min: self.halogen_angle,
        }
    }
}

fn is_nos(atomicnum: u8) -> bool {
    matches!(atomicnum, 7 | 8 | 16)
}

fn is_halogen(atomicnum: u8) -> bool {
    matches!(atomicnum, 17 | 35 | 53)
}

/// Angle (degrees) between vectors `v1` and `v2` (0 for a degenerate vector).
fn angle_deg(v1: Vec3, v2: Vec3) -> f32 {
    let n = v1.length() * v2.length();
    if n <= 1e-12 {
        return 0.0;
    }
    (v1.dot(v2) / n).clamp(-1.0, 1.0).acos().to_degrees()
}

/// Whether `donor`→`acc` is a valid H-bond at heavy-atom distance `d` (nm).
fn hbond_ok(donor: &AtomInfo, acc: &AtomInfo, d: f32, p: &DetectParams) -> bool {
    if !is_nos(donor.atomicnum) || !is_nos(acc.atomicnum) {
        return false;
    }
    if donor.attached_h.is_empty() {
        d <= p.hbond_dist_heavy
    } else {
        d < p.hbond_dist_h
            && donor
                .attached_h
                .iter()
                .any(|h| angle_deg(donor.pos - *h, acc.pos - *h) > p.hbond_angle_min)
    }
}

/// Whether `x`(halogen)→`acc` is a valid halogen bond at distance `d` (nm): the C–X···A
/// angle (at X, between X→antecedent and X→acceptor) must be near-linear.
fn halogen_ok(x: &AtomInfo, acc: &AtomInfo, d: f32, p: &DetectParams) -> bool {
    if !is_halogen(x.atomicnum) || !is_nos(acc.atomicnum) || d >= p.halogen_dist {
        return false;
    }
    match x.antecedent {
        Some(c) => angle_deg(c - x.pos, acc.pos - x.pos) > p.halogen_angle_min,
        None => false,
    }
}

fn res_pair(a: u64, b: u64) -> (u64, u64) {
    (a.min(b), a.max(b))
}

/// Detect all enabled interaction types between sets `a` and `b`. Deterministic order.
pub fn detect(a: &InteractionSet, b: &InteractionSet, p: &DetectParams) -> Vec<Interaction> {
    let mut out = Vec::new();
    atom_level(a, b, p, &mut out);
    salt_bridges(a, b, p, &mut out);
    pi_stacking(a, b, p, &mut out);
    pi_cation(a, b, p, &mut out);
    out
}

/// H-bond / hydrophobic / halogen — all atom-level, so one grid pass over the atoms.
fn atom_level(a: &InteractionSet, b: &InteractionSet, p: &DetectParams, out: &mut Vec<Interaction>) {
    if a.atoms.is_empty() || b.atoms.is_empty() {
        return;
    }
    let mut max_cut = 0.0_f32;
    if p.hbonds {
        max_cut = max_cut.max(p.hbond_dist_h);
    }
    if p.hydrophobic {
        max_cut = max_cut.max(p.hydrophobic_dist);
    }
    if p.halogen {
        max_cut = max_cut.max(p.halogen_dist);
    }
    if max_cut <= 0.0 {
        return;
    }

    // Iterate the smaller set, build the grid over the larger.
    let (qs, gs) = if a.atoms.len() <= b.atoms.len() {
        (&a.atoms, &b.atoms)
    } else {
        (&b.atoms, &a.atoms)
    };
    let (min, max) = bbox(gs);
    let grid = AtomGrid::build(
        gs.iter().enumerate().map(|(i, x)| (i as u32, x.pos)),
        min,
        max,
        max_cut,
    );

    // Hydrophobic contacts reduce to the shortest per residue pair (avoid a hairball).
    let mut hydro: BTreeMap<(u64, u64), (f32, Vec3, Vec3)> = BTreeMap::new();

    for qi in qs {
        grid.neighbors_within(qi.pos, max_cut, |gidx| {
            let gi = &gs[gidx as usize];
            if qi.res_key == gi.res_key {
                return;
            }
            let d = (qi.pos - gi.pos).length();
            if d <= MIN_DIST {
                return;
            }
            if p.hydrophobic
                && d < p.hydrophobic_dist
                && qi.only_ch_neighbors
                && gi.only_ch_neighbors
            {
                let e = hydro
                    .entry(res_pair(qi.res_key, gi.res_key))
                    .or_insert((f32::INFINITY, Vec3::ZERO, Vec3::ZERO));
                if d < e.0 {
                    *e = (d, qi.pos, gi.pos);
                }
            }
            if p.hbonds && (hbond_ok(qi, gi, d, p) || hbond_ok(gi, qi, d, p)) {
                out.push(Interaction { kind: InteractionKind::HBond, a: qi.pos, b: gi.pos });
            }
            if p.halogen && (halogen_ok(qi, gi, d, p) || halogen_ok(gi, qi, d, p)) {
                out.push(Interaction { kind: InteractionKind::Halogen, a: qi.pos, b: gi.pos });
            }
        });
    }

    for (_, (_d, pa, pb)) in hydro {
        out.push(Interaction { kind: InteractionKind::Hydrophobic, a: pa, b: pb });
    }
}

fn salt_bridges(a: &InteractionSet, b: &InteractionSet, p: &DetectParams, out: &mut Vec<Interaction>) {
    if !p.salt_bridges {
        return;
    }
    let c2 = p.salt_bridge_dist * p.salt_bridge_dist;
    for (pos, neg) in [(&a.cations, &b.anions), (&b.cations, &a.anions)] {
        for cg in pos {
            for ng in neg {
                if cg.res_key == ng.res_key {
                    continue;
                }
                let d2 = (cg.center - ng.center).length_squared();
                if d2 > MIN_DIST * MIN_DIST && d2 <= c2 {
                    out.push(Interaction {
                        kind: InteractionKind::SaltBridge,
                        a: cg.center,
                        b: ng.center,
                    });
                }
            }
        }
    }
}

fn pi_stacking(a: &InteractionSet, b: &InteractionSet, p: &DetectParams, out: &mut Vec<Interaction>) {
    if !p.pi_stacking {
        return;
    }
    let c2 = p.pi_stacking_dist * p.pi_stacking_dist;
    for ra in &a.rings {
        for rb in &b.rings {
            if ra.res_key == rb.res_key {
                continue;
            }
            let cc = rb.center - ra.center;
            let d2 = cc.length_squared();
            if d2 <= MIN_DIST * MIN_DIST || d2 > c2 {
                continue;
            }
            // Angle between ring planes (0 = coplanar/parallel, 90 = perpendicular).
            let ang = ra.normal.dot(rb.normal).abs().clamp(0.0, 1.0).acos().to_degrees();
            let parallel = ang <= p.pi_stacking_angle;
            let tshaped = ang >= 90.0 - p.pi_stacking_angle;
            if !parallel && !tshaped {
                continue;
            }
            // For parallel stacks require the rings to actually overlap: the lateral
            // offset (component of the centroid vector ⟂ the ring normal) must be small.
            if parallel {
                let n = ra.normal.normalize_or_zero();
                let offset = (cc - n * cc.dot(n)).length();
                if offset > p.pi_stacking_offset {
                    continue;
                }
            }
            out.push(Interaction {
                kind: InteractionKind::PiStacking,
                a: ra.center,
                b: rb.center,
            });
        }
    }
}

fn pi_cation(a: &InteractionSet, b: &InteractionSet, p: &DetectParams, out: &mut Vec<Interaction>) {
    if !p.pi_cation {
        return;
    }
    let c2 = p.pi_cation_dist * p.pi_cation_dist;
    for (rings, cats) in [(&a.rings, &b.cations), (&b.rings, &a.cations)] {
        for r in rings {
            for c in cats {
                if r.res_key == c.res_key {
                    continue;
                }
                let cc = c.center - r.center;
                let d2 = cc.length_squared();
                if d2 <= MIN_DIST * MIN_DIST || d2 > c2 {
                    continue;
                }
                // The cation must sit roughly over the ring face: its lateral offset
                // from the ring axis (⟂ the normal) must be within the cutoff.
                let n = r.normal.normalize_or_zero();
                let offset = (cc - n * cc.dot(n)).length();
                if offset > p.pi_cation_offset {
                    continue;
                }
                out.push(Interaction {
                    kind: InteractionKind::PiCation,
                    a: r.center,
                    b: c.center,
                });
            }
        }
    }
}

fn bbox(atoms: &[AtomInfo]) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for a in atoms {
        min = min.min(a.pos);
        max = max.max(a.pos);
    }
    (min, max)
}

/// Ring plane normal via Newell's method (robust for a near-planar polygon). Returns a
/// unit vector (or Z if degenerate).
pub fn ring_normal(points: &[Vec3]) -> Vec3 {
    let mut n = Vec3::ZERO;
    for i in 0..points.len() {
        let c = points[i];
        let d = points[(i + 1) % points.len()];
        n.x += (c.y - d.y) * (c.z + d.z);
        n.y += (c.z - d.z) * (c.x + d.x);
        n.z += (c.x - d.x) * (c.y + d.y);
    }
    n.normalize_or(Vec3::Z)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom(pos: [f32; 3], atomicnum: u8, res: u64) -> AtomInfo {
        AtomInfo {
            pos: Vec3::from(pos),
            atomicnum,
            res_key: res,
            only_ch_neighbors: atomicnum == 6,
            attached_h: Vec::new(),
            antecedent: None,
        }
    }

    fn set(atoms: Vec<AtomInfo>) -> InteractionSet {
        InteractionSet { atoms, ..Default::default() }
    }

    fn count(v: &[Interaction], k: InteractionKind) -> usize {
        v.iter().filter(|i| i.kind == k).count()
    }

    #[test]
    fn hbond_with_explicit_h() {
        let mut donor = atom([0.0, 0.0, 0.0], 7, 0);
        donor.attached_h = vec![Vec3::new(0.10, 0.0, 0.0)];
        let acc = atom([0.30, 0.0, 0.0], 8, 1);
        let r = detect(&set(vec![donor]), &set(vec![acc]), &InteractionSettings::default().detect());
        assert_eq!(count(&r, InteractionKind::HBond), 1);
    }

    #[test]
    fn hbond_angle_too_small_is_rejected() {
        let mut donor = atom([0.0, 0.0, 0.0], 7, 0);
        donor.attached_h = vec![Vec3::new(-0.10, 0.0, 0.0)];
        let acc = atom([0.38, 0.0, 0.0], 8, 1);
        let r = detect(&set(vec![donor]), &set(vec![acc]), &InteractionSettings::default().detect());
        assert_eq!(count(&r, InteractionKind::HBond), 0);
    }

    #[test]
    fn hbond_heavy_fallback_without_hydrogens() {
        let p = InteractionSettings::default().detect();
        let close = detect(&set(vec![atom([0.0, 0.0, 0.0], 7, 0)]), &set(vec![atom([0.30, 0.0, 0.0], 8, 1)]), &p);
        assert_eq!(count(&close, InteractionKind::HBond), 1);
        let far = detect(&set(vec![atom([0.0, 0.0, 0.0], 7, 0)]), &set(vec![atom([0.40, 0.0, 0.0], 8, 1)]), &p);
        assert_eq!(count(&far, InteractionKind::HBond), 0);
    }

    #[test]
    fn hydrophobic_within_and_beyond_and_dedup() {
        let p = InteractionSettings::default().detect();
        let near = detect(&set(vec![atom([0.0, 0.0, 0.0], 6, 0)]), &set(vec![atom([0.38, 0.0, 0.0], 6, 1)]), &p);
        assert_eq!(count(&near, InteractionKind::Hydrophobic), 1);
        let far = detect(&set(vec![atom([0.0, 0.0, 0.0], 6, 0)]), &set(vec![atom([0.45, 0.0, 0.0], 6, 1)]), &p);
        assert_eq!(count(&far, InteractionKind::Hydrophobic), 0);
        // Two carbons of res 0 near two of res 1 → one line per residue pair.
        let a = vec![atom([0.0, 0.0, 0.0], 6, 0), atom([0.05, 0.0, 0.0], 6, 0)];
        let b = vec![atom([0.30, 0.0, 0.0], 6, 1), atom([0.32, 0.0, 0.0], 6, 1)];
        assert_eq!(count(&detect(&set(a), &set(b), &p), InteractionKind::Hydrophobic), 1);
    }

    #[test]
    fn halogen_bond_needs_linear_angle() {
        let p = InteractionSettings::default().detect();
        // C at origin, Cl at +x (C–Cl bond along +x); acceptor O beyond Cl (linear) → OK.
        let mut cl = atom([0.18, 0.0, 0.0], 17, 0);
        cl.antecedent = Some(Vec3::new(0.0, 0.0, 0.0)); // the C
        let acc = atom([0.50, 0.0, 0.0], 8, 1);
        assert_eq!(count(&detect(&set(vec![cl]), &set(vec![acc]), &p), InteractionKind::Halogen), 1);
        // Acceptor off to the side (bent) → rejected.
        let mut cl2 = atom([0.18, 0.0, 0.0], 17, 0);
        cl2.antecedent = Some(Vec3::new(0.0, 0.0, 0.0));
        let side = atom([0.18, 0.30, 0.0], 8, 1);
        assert_eq!(count(&detect(&set(vec![cl2]), &set(vec![side]), &p), InteractionKind::Halogen), 0);
    }

    #[test]
    fn salt_bridge_within_cutoff() {
        let p = InteractionSettings::default().detect();
        let a = InteractionSet { cations: vec![ChargeGroup { center: Vec3::ZERO, res_key: 0 }], ..Default::default() };
        let b = InteractionSet { anions: vec![ChargeGroup { center: Vec3::new(0.40, 0.0, 0.0), res_key: 1 }], ..Default::default() };
        assert_eq!(count(&detect(&a, &b, &p), InteractionKind::SaltBridge), 1);
        let far = InteractionSet { anions: vec![ChargeGroup { center: Vec3::new(0.80, 0.0, 0.0), res_key: 1 }], ..Default::default() };
        assert_eq!(count(&detect(&a, &far, &p), InteractionKind::SaltBridge), 0);
    }

    #[test]
    fn pi_stacking_parallel_and_offset() {
        let p = InteractionSettings::default().detect();
        // Two parallel rings (normals +z) stacked 0.38 nm apart, aligned → parallel stack.
        let a = InteractionSet { rings: vec![RingInfo { center: Vec3::ZERO, normal: Vec3::Z, res_key: 0 }], ..Default::default() };
        let b = InteractionSet { rings: vec![RingInfo { center: Vec3::new(0.0, 0.0, 0.38), normal: Vec3::Z, res_key: 1 }], ..Default::default() };
        assert_eq!(count(&detect(&a, &b, &p), InteractionKind::PiStacking), 1);
        // Same distance but laterally offset by 0.4 nm → no overlap → rejected.
        let off = InteractionSet { rings: vec![RingInfo { center: Vec3::new(0.40, 0.0, 0.38), normal: Vec3::Z, res_key: 1 }], ..Default::default() };
        assert_eq!(count(&detect(&a, &off, &p), InteractionKind::PiStacking), 0);
    }

    #[test]
    fn pi_cation_within_cutoff() {
        let p = InteractionSettings::default().detect();
        let a = InteractionSet { rings: vec![RingInfo { center: Vec3::ZERO, normal: Vec3::Z, res_key: 0 }], ..Default::default() };
        let b = InteractionSet { cations: vec![ChargeGroup { center: Vec3::new(0.0, 0.0, 0.40), res_key: 1 }], ..Default::default() };
        assert_eq!(count(&detect(&a, &b, &p), InteractionKind::PiCation), 1);
    }

    /// Brute-force reference for the atom-level types, to validate the grid path.
    fn brute_atoms(a: &[AtomInfo], b: &[AtomInfo], p: &DetectParams) -> (usize, usize, usize) {
        let (mut hb, mut hal) = (0usize, 0usize);
        let mut hydro: BTreeMap<(u64, u64), f32> = BTreeMap::new();
        for qi in a {
            for gi in b {
                if qi.res_key == gi.res_key {
                    continue;
                }
                let d = (qi.pos - gi.pos).length();
                if d <= MIN_DIST {
                    continue;
                }
                if p.hydrophobic && d < p.hydrophobic_dist && qi.only_ch_neighbors && gi.only_ch_neighbors {
                    let e = hydro.entry(res_pair(qi.res_key, gi.res_key)).or_insert(f32::INFINITY);
                    if d < *e {
                        *e = d;
                    }
                }
                if p.hbonds && (hbond_ok(qi, gi, d, p) || hbond_ok(gi, qi, d, p)) {
                    hb += 1;
                }
                if p.halogen && (halogen_ok(qi, gi, d, p) || halogen_ok(gi, qi, d, p)) {
                    hal += 1;
                }
            }
        }
        (hb, hydro.len(), hal)
    }

    #[test]
    fn grid_matches_brute_force() {
        let mk = |n: usize, res_off: u64, elem: u8| -> Vec<AtomInfo> {
            (0..n)
                .map(|i| {
                    let h = (i as u64).wrapping_mul(2654435761) ^ (res_off << 3);
                    let x = ((h & 0xff) as f32 / 255.0) * 2.0;
                    let y = (((h >> 8) & 0xff) as f32 / 255.0) * 2.0;
                    let z = (((h >> 16) & 0xff) as f32 / 255.0) * 2.0;
                    let mut a = atom([x, y, z], elem, res_off + i as u64);
                    if elem != 6 && h & 1 == 0 {
                        a.attached_h = vec![a.pos + Vec3::new(0.09, 0.0, 0.0)];
                    }
                    a
                })
                .collect()
        };
        let p = InteractionSettings::default().detect();
        for &(na, nb, ea, eb) in &[(30usize, 40usize, 6u8, 6u8), (25, 20, 7, 8), (50, 10, 6, 8)] {
            let a = mk(na, 0, ea);
            let b = mk(nb, 1000, eb);
            let got = detect(&set(a.iter().map(cloneinfo).collect()), &set(b.iter().map(cloneinfo).collect()), &p);
            let hb = count(&got, InteractionKind::HBond);
            let hy = count(&got, InteractionKind::Hydrophobic);
            let hal = count(&got, InteractionKind::Halogen);
            assert_eq!((hb, hy, hal), brute_atoms(&a, &b, &p), "grid vs brute mismatch");
        }
    }

    fn cloneinfo(a: &AtomInfo) -> AtomInfo {
        AtomInfo {
            pos: a.pos,
            atomicnum: a.atomicnum,
            res_key: a.res_key,
            only_ch_neighbors: a.only_ch_neighbors,
            attached_h: a.attached_h.clone(),
            antecedent: a.antecedent,
        }
    }
}
