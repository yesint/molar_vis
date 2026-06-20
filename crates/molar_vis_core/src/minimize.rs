//! Lightweight UFF-style force field + FIRE minimizer for on-the-fly geometry
//! cleanup of interactively drawn molecules.
//!
//! This is **not** a full UFF. It carries the terms needed to turn a hand-sketched
//! connectivity into a plausible 3-D geometry: harmonic bond stretch, harmonic
//! angle bend, and a purely-repulsive (WCA) van der Waals core that keeps
//! non-bonded atoms from overlapping. Parameters are tuned for *cleanup of small
//! organic sketches*, not spectroscopic accuracy. (A weak torsion term that
//! enforces sp2 planarity / sp3 staggering is a planned follow-up; the architecture
//! leaves room for it — see `TorsionTerm`, currently unused.)
//!
//! Everything is pure `glam::Vec3` math plus molar element data: single-threaded,
//! no rayon, no clocks — so it compiles and runs on `wasm32-unknown-unknown`.
//!
//! Units: coordinates are in **nm** (matching molar). Energies are in an internal
//! "ff-energy" unit; the minimizer only ever compares relative energies and follows
//! forces, so the unit need only be internally consistent. Forces are ff-energy/nm.

use std::collections::HashSet;

use glam::Vec3;
use molar::prelude::*;

// ---------------------------------------------------------------------------
// Bond order
// ---------------------------------------------------------------------------

/// Bond multiplicity as drawn in the editor. `Aromatic` is a sentinel treated as
/// effective order 1.5 (for the equilibrium length) and forces sp2 on its atoms.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum BondOrder {
    #[default]
    Single = 1,
    Double = 2,
    Triple = 3,
    Aromatic = 4,
}

impl BondOrder {
    /// Numeric order used for the equilibrium bond length (aromatic = 1.5).
    pub fn effective(self) -> f32 {
        match self {
            BondOrder::Single => 1.0,
            BondOrder::Double => 2.0,
            BondOrder::Triple => 3.0,
            BondOrder::Aromatic => 1.5,
        }
    }

    /// Next order when the user clicks a bond to cycle it (single→double→triple→single).
    /// Aromatic cycles back to single (it isn't reachable by clicking in the MVP).
    pub fn cycle(self) -> Self {
        match self {
            BondOrder::Single => BondOrder::Double,
            BondOrder::Double => BondOrder::Triple,
            BondOrder::Triple => BondOrder::Single,
            BondOrder::Aromatic => BondOrder::Single,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BondOrder::Single => "Single",
            BondOrder::Double => "Double",
            BondOrder::Triple => "Triple",
            BondOrder::Aromatic => "Aromatic",
        }
    }
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// Covalent single-bond radius (nm), Pyykkö & Atsumi 2009 for the organic subset,
/// with a generic fallback for everything else. Used for the equilibrium bond length.
fn covalent_radius(z: u8) -> f32 {
    match z {
        1 => 0.031,  // H
        6 => 0.076,  // C
        7 => 0.071,  // N
        8 => 0.066,  // O
        9 => 0.057,  // F
        15 => 0.107, // P
        16 => 0.105, // S
        17 => 0.102, // Cl
        35 => 0.120, // Br
        53 => 0.139, // I
        _ => 0.075,
    }
}

/// van der Waals radius (nm) for the WCA repulsion. Mirrors molar's element table
/// for the organic subset so the repulsion onset matches the rendered atom sizes.
fn vdw_radius(z: u8) -> f32 {
    match z {
        1 => 0.120,  // H
        6 => 0.170,  // C
        7 => 0.155,  // N
        8 => 0.152,  // O
        9 => 0.147,  // F
        15 => 0.180, // P
        16 => 0.180, // S
        17 => 0.175, // Cl
        35 => 0.185, // Br
        53 => 0.198, // I
        _ => 0.150,
    }
}

/// UFF bond-order length correction coefficient (UFF's 0.1332 Å), in nm.
const BOND_ORDER_LAMBDA: f32 = 0.01332;
/// Harmonic bond-stretch force constant (ff-energy / nm²).
const K_BOND: f32 = 5.0e4;
/// Harmonic angle-bend force constant (ff-energy / rad²).
const K_ANGLE: f32 = 500.0;
/// vdW well depth (ff-energy). Only scales the repulsion strength.
const VDW_EPS: f32 = 2.0;
/// Torsion barrier (ff-energy). Deliberately weak — it only biases sp3 bonds toward
/// staggering and sp2/aromatic bonds toward planarity; it never fights stretch/bend.
const TORSION_VN: f32 = 5.0;
/// 2^(1/6): the LJ minimum, where the WCA core is truncated.
const TWO_POW_1_6: f32 = 1.122_462_048;

/// Equilibrium bond length (nm) from the two covalent radii and the bond order.
fn equilibrium_bond_length(zi: u8, zj: u8, order: BondOrder) -> f32 {
    let r = covalent_radius(zi) + covalent_radius(zj)
        - BOND_ORDER_LAMBDA * order.effective().ln();
    r.max(0.05)
}

// ---------------------------------------------------------------------------
// Hybridization perception
// ---------------------------------------------------------------------------

/// Perceived local geometry of an atom, driving its ideal bond angle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hybrid {
    /// Linear, ~180° (one triple bond or two cumulated doubles).
    Sp,
    /// Trigonal planar, ~120° (a double or aromatic bond).
    Sp2,
    /// Tetrahedral, ~109.5° (all single bonds).
    Sp3,
    /// One neighbour: defines no angle of its own.
    Terminal,
    /// No bonds: contributes nothing.
    Unknown,
}

impl Hybrid {
    /// Ideal central bond angle (radians); `None` when the atom is never an angle center.
    fn theta0(self) -> Option<f32> {
        match self {
            Hybrid::Sp => Some(std::f32::consts::PI),
            Hybrid::Sp2 => Some(2.094_395_1),  // 120°
            Hybrid::Sp3 => Some(1.910_633_2),  // 109.47°
            Hybrid::Terminal | Hybrid::Unknown => None,
        }
    }
}

/// Classify every atom's hybridization from the bond graph: neighbour count plus the
/// maximum incident bond order. Halogens and hydrogen are always terminal; carbon and
/// the other organic centers fall back to sp3 unless a multiple bond raises them.
pub fn perceive_hybridization(
    atomic_numbers: &[u8],
    bonds: &[[usize; 2]],
    orders: &[BondOrder],
) -> Vec<Hybrid> {
    let n = atomic_numbers.len();
    let mut degree = vec![0u32; n];
    let mut max_order = vec![0.0f32; n];
    let mut aromatic = vec![false; n];
    for (b, &[i, j]) in bonds.iter().enumerate() {
        if i >= n || j >= n {
            continue;
        }
        let o = orders.get(b).copied().unwrap_or_default();
        degree[i] += 1;
        degree[j] += 1;
        max_order[i] = max_order[i].max(o.effective());
        max_order[j] = max_order[j].max(o.effective());
        if o == BondOrder::Aromatic {
            aromatic[i] = true;
            aromatic[j] = true;
        }
    }

    (0..n)
        .map(|i| {
            let z = atomic_numbers[i];
            match degree[i] {
                0 => Hybrid::Unknown,
                1 => Hybrid::Terminal,
                _ => {
                    // Halogens never act as a multi-coordinate center.
                    if matches!(z, 9 | 17 | 35 | 53 | 1) {
                        return Hybrid::Terminal;
                    }
                    if aromatic[i] {
                        Hybrid::Sp2
                    } else if max_order[i] >= 3.0 {
                        Hybrid::Sp
                    } else if max_order[i] >= 2.0 {
                        Hybrid::Sp2
                    } else {
                        Hybrid::Sp3
                    }
                }
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Force-field model
// ---------------------------------------------------------------------------

struct BondTerm {
    i: usize,
    j: usize,
    r0: f32,
    k: f32,
}

struct AngleTerm {
    i: usize,
    j: usize,
    k: usize,
    theta0: f32,
    kt: f32,
}

/// A four-body torsion term `E = (Vn/2)·(1 + cos(n·φ − phase))` about the central
/// bond `j–k`. sp3–sp3 bonds use `n=3, phase=0` (staggering); sp2/aromatic–sp2 bonds
/// use `n=2, phase=π` (planarity).
struct TorsionTerm {
    i: usize,
    j: usize,
    k: usize,
    l: usize,
    vn: f32,
    n: f32,
    phase: f32,
}

/// Torsion parameters `(Vn, periodicity, phase)` for a central bond whose two atoms
/// have hybridizations `hj`/`hk`. Returns `None` when no meaningful torsion applies
/// (a linear/terminal end, or a mixed sp2/sp3 bond whose barrier is negligible).
fn torsion_params(hj: Hybrid, hk: Hybrid) -> Option<(f32, f32, f32)> {
    use Hybrid::*;
    match (hj, hk) {
        (Sp3, Sp3) => Some((TORSION_VN, 3.0, 0.0)),
        (Sp2, Sp2) => Some((TORSION_VN, 2.0, std::f32::consts::PI)),
        _ => None,
    }
}

/// A typed, parameterized force field for one molecule's current topology. Built
/// once per committed edit (cheap, O(N·deg²)) and reused across every FIRE step.
/// Holds no coordinates, so it stays valid as long as the topology is unchanged.
pub struct FfModel {
    n: usize,
    bonds: Vec<BondTerm>,
    angles: Vec<AngleTerm>,
    torsions: Vec<TorsionTerm>,
    /// Per-atom WCA σ (nm) = vdW radius · 2^(-1/6), so the LJ minimum sits at the
    /// vdW radius.
    sigma: Vec<f32>,
    /// Non-bonded exclusions: 1-2 (bonded) and 1-3 (angle end) pairs, `(min,max)`.
    excluded: HashSet<(u32, u32)>,
}

fn excl_key(a: usize, b: usize) -> (u32, u32) {
    if a < b {
        (a as u32, b as u32)
    } else {
        (b as u32, a as u32)
    }
}

/// Build the force-field model from per-atom elements + the editor's bonds/orders.
/// Out-of-range bond endpoints (e.g. a stale bond after an atom deletion) are skipped.
pub fn build_model(atomic_numbers: &[u8], bonds: &[[usize; 2]], orders: &[BondOrder]) -> FfModel {
    let n = atomic_numbers.len();
    let hyb = perceive_hybridization(atomic_numbers, bonds, orders);

    // Adjacency (only valid, in-range bonds).
    let mut nbr: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut bond_terms = Vec::with_capacity(bonds.len());
    let mut excluded: HashSet<(u32, u32)> = HashSet::new();
    for (b, &[i, j]) in bonds.iter().enumerate() {
        if i >= n || j >= n || i == j {
            continue;
        }
        nbr[i].push(j);
        nbr[j].push(i);
        let order = orders.get(b).copied().unwrap_or_default();
        bond_terms.push(BondTerm {
            i,
            j,
            r0: equilibrium_bond_length(atomic_numbers[i], atomic_numbers[j], order),
            k: K_BOND,
        });
        excluded.insert(excl_key(i, j));
    }

    // Angles: every i–j–k with j the central atom.
    let mut angle_terms = Vec::new();
    for j in 0..n {
        let Some(theta0) = hyb[j].theta0() else {
            continue;
        };
        let ns = &nbr[j];
        for a in 0..ns.len() {
            for b in (a + 1)..ns.len() {
                let (i, k) = (ns[a], ns[b]);
                angle_terms.push(AngleTerm {
                    i,
                    j,
                    k,
                    theta0,
                    kt: K_ANGLE,
                });
                excluded.insert(excl_key(i, k));
            }
        }
    }

    // Torsions: i–j–k–l about each central bond j–k (staggering / planarity).
    let mut torsion_terms = Vec::new();
    for bt in &bond_terms {
        let (j, k) = (bt.i, bt.j);
        let Some((vn, period, phase)) = torsion_params(hyb[j], hyb[k]) else {
            continue;
        };
        for &i in &nbr[j] {
            if i == k {
                continue;
            }
            for &l in &nbr[k] {
                if l == j || l == i {
                    continue;
                }
                torsion_terms.push(TorsionTerm { i, j, k, l, vn, n: period, phase });
            }
        }
    }

    let sigma = (0..n)
        .map(|i| vdw_radius(atomic_numbers[i]) / TWO_POW_1_6)
        .collect();

    FfModel {
        n,
        bonds: bond_terms,
        angles: angle_terms,
        torsions: torsion_terms,
        sigma,
        excluded,
    }
}

// ---------------------------------------------------------------------------
// Energy + analytic gradients
// ---------------------------------------------------------------------------

/// A tiny separation used to keep the gradient kernels finite when two atoms sit on
/// top of each other (a freshly placed atom can coincide with another).
const EPS_DIST: f32 = 1.0e-6;

/// Accumulate the total energy and per-atom forces (`forces[i] = −∂E/∂x_i`).
/// `forces` is zeroed on entry. Returns the total ff-energy.
fn energy_forces(coords: &[Vec3], model: &FfModel, forces: &mut [Vec3]) -> f32 {
    for f in forces.iter_mut() {
        *f = Vec3::ZERO;
    }
    let mut energy = 0.0f32;

    // Bond stretch: E = k·(r − r0)².
    for t in &model.bonds {
        let mut d = coords[t.i] - coords[t.j];
        let mut r = d.length();
        if r < EPS_DIST {
            // Coincident atoms: push apart along a fixed axis instead of /0.
            d = Vec3::X * EPS_DIST;
            r = EPS_DIST;
        }
        let dr = r - t.r0;
        energy += t.k * dr * dr;
        let f = d * (-2.0 * t.k * dr / r); // force on i
        forces[t.i] += f;
        forces[t.j] -= f;
    }

    // Angle bend: E = kt·(θ − θ0)².
    for t in &model.angles {
        let rij = coords[t.i] - coords[t.j];
        let rkj = coords[t.k] - coords[t.j];
        let lij = rij.length();
        let lkj = rkj.length();
        if lij < EPS_DIST || lkj < EPS_DIST {
            continue;
        }
        let cos = (rij.dot(rkj) / (lij * lkj)).clamp(-1.0 + 1.0e-7, 1.0 - 1.0e-7);
        let theta = cos.acos();
        let sin = (1.0 - cos * cos).sqrt().max(1.0e-7);
        let dth = theta - t.theta0;
        energy += t.kt * dth * dth;
        let de_dtheta = 2.0 * t.kt * dth;
        let coef = de_dtheta / sin;
        // fi = (dE/dθ / sinθ)·(rkj/(lij·lkj) − cosθ·rij/lij²)
        let fi = (rkj / (lij * lkj) - rij * (cos / (lij * lij))) * coef;
        let fk = (rij / (lij * lkj) - rkj * (cos / (lkj * lkj))) * coef;
        forces[t.i] += fi;
        forces[t.k] += fk;
        forces[t.j] -= fi + fk;
    }

    // Torsion: E = (Vn/2)·(1 + cos(n·φ − phase)), Blondel–Karplus forces (no acos).
    for t in &model.torsions {
        let b1 = coords[t.j] - coords[t.i];
        let b2 = coords[t.k] - coords[t.j];
        let b3 = coords[t.l] - coords[t.k];
        let n1 = b1.cross(b2);
        let n2 = b2.cross(b3);
        let n1n = n1.length_squared();
        let n2n = n2.length_squared();
        let b2len = b2.length();
        if n1n < 1.0e-12 || n2n < 1.0e-12 || b2len < EPS_DIST {
            continue; // collinear i-j-k or j-k-l → dihedral undefined
        }
        // Signed dihedral via atan2 (robust near 0/π).
        let m1 = n1.cross(b2 / b2len);
        let phi = m1.dot(n2).atan2(n1.dot(n2));
        let arg = t.n * phi - t.phase;
        energy += 0.5 * t.vn * (1.0 + arg.cos());
        let de_dphi = -0.5 * t.vn * t.n * arg.sin();
        let fi = n1 * (-de_dphi * b2len / n1n);
        let fl = n2 * (de_dphi * b2len / n2n);
        // Middle-atom distribution (Blondel–Karplus). The projection scalars use the
        // GROMACS vector convention (r_ij = i−j, r_kl = k−l), which is the negation
        // of our b1 = j−i and b3 = l−k, hence the leading minus.
        let s = -b1.dot(b2) / (b2len * b2len);
        let u = -b3.dot(b2) / (b2len * b2len);
        let fj = fi * (s - 1.0) - fl * u;
        let fk = fl * (u - 1.0) - fi * s;
        forces[t.i] += fi;
        forces[t.j] += fj;
        forces[t.k] += fk;
        forces[t.l] += fl;
    }

    // van der Waals: purely-repulsive WCA core between non-excluded pairs.
    let n = model.n;
    for i in 0..n {
        for j in (i + 1)..n {
            if model.excluded.contains(&(i as u32, j as u32)) {
                continue;
            }
            let sigma = 0.5 * (model.sigma[i] + model.sigma[j]);
            let r_cut = TWO_POW_1_6 * sigma;
            let mut d = coords[i] - coords[j];
            let mut r = d.length();
            if r >= r_cut {
                continue;
            }
            if r < EPS_DIST {
                d = Vec3::X * EPS_DIST;
                r = EPS_DIST;
            }
            let sr6 = (sigma / r).powi(6);
            let sr12 = sr6 * sr6;
            energy += 4.0 * VDW_EPS * (sr12 - sr6) + VDW_EPS; // shifted up → 0 at cutoff
            let fmag = 24.0 * VDW_EPS * (2.0 * sr12 - sr6) / r; // > 0, repulsive
            let f = d * (fmag / r);
            forces[i] += f;
            forces[j] -= f;
        }
    }

    energy
}

// ---------------------------------------------------------------------------
// Minimizer (FIRE)
// ---------------------------------------------------------------------------

/// Tunables for a minimization run.
#[derive(Clone, Copy, Debug)]
pub struct MinimizeOpts {
    pub max_steps: u32,
    /// Converged when the largest per-atom force magnitude drops below this.
    pub force_tol: f32,
    pub dt_start: f32,
    pub dt_max: f32,
    /// Per-step displacement clamp (nm) — the anti-explosion guard.
    pub max_disp: f32,
}

impl MinimizeOpts {
    /// A short, loose relaxation: run after each committed edit.
    pub fn quick_relax() -> Self {
        Self {
            max_steps: 60,
            force_tol: 2.0e3,
            dt_start: 5.0e-4,
            dt_max: 2.0e-3,
            max_disp: 3.0e-3,
        }
    }

    /// A tight relaxation to (near) convergence: the "Clean up" button.
    pub fn full_cleanup() -> Self {
        Self {
            max_steps: 1000,
            force_tol: 50.0,
            dt_start: 5.0e-4,
            dt_max: 3.0e-3,
            max_disp: 5.0e-3,
        }
    }
}

/// Outcome of a minimization run.
#[derive(Clone, Copy, Debug)]
pub struct MinimizeResult {
    pub steps: u32,
    /// Largest per-atom force magnitude at the final step.
    pub final_force_norm: f32,
    pub converged: bool,
    /// A non-finite value was detected and sanitized during the run.
    pub had_nan: bool,
}

// FIRE constants (Bitzek et al. 2006).
const FIRE_ALPHA_START: f32 = 0.1;
const FIRE_F_INC: f32 = 1.1;
const FIRE_F_DEC: f32 = 0.5;
const FIRE_F_ALPHA: f32 = 0.99;
const FIRE_N_MIN: u32 = 5;

/// Relax `coords` in place under `model` with the FIRE integrator (unit mass).
/// Pure CPU, single-threaded; allocates two scratch buffers up front and nothing in
/// the step loop. `coords.len()` must equal `model.num_atoms()`.
pub fn minimize(coords: &mut [Vec3], model: &FfModel, opts: MinimizeOpts) -> MinimizeResult {
    let n = coords.len();
    debug_assert_eq!(n, model.n);
    if n == 0 {
        return MinimizeResult { steps: 0, final_force_norm: 0.0, converged: true, had_nan: false };
    }

    let mut vel = vec![Vec3::ZERO; n];
    let mut forces = vec![Vec3::ZERO; n];
    let mut dt = opts.dt_start;
    let mut alpha = FIRE_ALPHA_START;
    let mut n_pos = 0u32;
    let mut had_nan = false;
    let mut fmax = 0.0f32;
    let mut steps = 0u32;

    while steps < opts.max_steps {
        steps += 1;
        energy_forces(coords, model, &mut forces);

        // Sanitize: a non-finite force on an atom is zeroed so one bad term can't
        // poison the whole system.
        for f in forces.iter_mut() {
            if !f.is_finite() {
                *f = Vec3::ZERO;
                had_nan = true;
            }
        }

        // Convergence check on the max per-atom force.
        fmax = forces.iter().map(|f| f.length()).fold(0.0, f32::max);
        if fmax < opts.force_tol {
            return MinimizeResult { steps, final_force_norm: fmax, converged: true, had_nan };
        }

        // FIRE power: are we moving downhill?
        let power: f32 = forces.iter().zip(&vel).map(|(f, v)| f.dot(*v)).sum();
        if power > 0.0 {
            let fnorm = forces.iter().map(|f| f.length_squared()).sum::<f32>().sqrt();
            let vnorm = vel.iter().map(|v| v.length_squared()).sum::<f32>().sqrt();
            if fnorm > EPS_DIST {
                let scale = alpha * vnorm / fnorm;
                for (v, f) in vel.iter_mut().zip(&forces) {
                    *v = *v * (1.0 - alpha) + *f * scale;
                }
            }
            n_pos += 1;
            if n_pos > FIRE_N_MIN {
                dt = (dt * FIRE_F_INC).min(opts.dt_max);
                alpha *= FIRE_F_ALPHA;
            }
        } else {
            // Uphill: freeze, shrink the step, reset mixing.
            for v in vel.iter_mut() {
                *v = Vec3::ZERO;
            }
            dt *= FIRE_F_DEC;
            alpha = FIRE_ALPHA_START;
            n_pos = 0;
        }

        // Semi-implicit Euler step with a per-atom displacement clamp.
        for (c, (v, f)) in coords.iter_mut().zip(vel.iter_mut().zip(&forces)) {
            *v += *f * dt;
            let mut dx = *v * dt;
            let len = dx.length();
            if len > opts.max_disp {
                dx *= opts.max_disp / len;
            }
            *c += dx;
        }
    }

    MinimizeResult { steps, final_force_norm: fmax, converged: false, had_nan }
}

// ---------------------------------------------------------------------------
// molar bridge
// ---------------------------------------------------------------------------

/// Which relaxation profile to run.
#[derive(Clone, Copy, Debug)]
pub enum RelaxKind {
    /// Short relaxation after each committed edit.
    Quick,
    /// Minimize to (near) convergence — the "Clean up" button.
    Cleanup,
}

/// Read the coordinates + elements out of `system`, relax them under a model built
/// from `bonds`/`orders`, and write the relaxed coordinates back in place
/// (preserving the box / velocities). The only function in this module that touches
/// molar; everything below it is pure `glam` math.
pub fn relax_in_system(
    system: &mut System,
    bonds: &[[usize; 2]],
    orders: &[BondOrder],
    kind: RelaxKind,
) -> MinimizeResult {
    let (mut coords, atomic_numbers): (Vec<Vec3>, Vec<u8>) = {
        let b = system.select_all_bound();
        let coords = b.iter_pos().map(|p| Vec3::new(p.x, p.y, p.z)).collect();
        let z = b.iter_atoms().map(|a| a.atomic_number).collect();
        (coords, z)
    };

    let model = build_model(&atomic_numbers, bonds, orders);
    let opts = match kind {
        RelaxKind::Quick => MinimizeOpts::quick_relax(),
        RelaxKind::Cleanup => MinimizeOpts::full_cleanup(),
    };
    let res = minimize(&mut coords, &model, opts);

    if res.had_nan {
        log::warn!("minimize: sanitized a non-finite force (degenerate geometry?)");
    }

    let mut bmut = system.select_all_bound_mut();
    for (p, c) in bmut.iter_pos_mut().zip(&coords) {
        *p = Pos::new(c.x, c.y, c.z);
    }
    res
}

// ---------------------------------------------------------------------------
// Tests (no GPU; pure coordinate math)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Central finite-difference check of an analytic kernel's forces against −ΔE/Δx.
    fn grad_check(coords: &[Vec3], model: &FfModel) {
        let n = coords.len();
        let mut analytic = vec![Vec3::ZERO; n];
        energy_forces(coords, model, &mut analytic);
        let h = 1.0e-5f32;
        for i in 0..n {
            for axis in 0..3 {
                let mut plus = coords.to_vec();
                let mut minus = coords.to_vec();
                plus[i][axis] += h;
                minus[i][axis] -= h;
                let mut scratch = vec![Vec3::ZERO; n];
                let ep = energy_forces(&plus, model, &mut scratch);
                let em = energy_forces(&minus, model, &mut scratch);
                let numeric_force = -(ep - em) / (2.0 * h); // force = −dE/dx
                let analytic_force = analytic[i][axis];
                // Relative tolerance with an absolute floor: these forces are stiff
                // (hundreds–thousands), so an f32 central difference loses ~1% to
                // catastrophic cancellation. A real gradient bug is off by far more.
                let tol = 0.02 * analytic_force.abs().max(1.0) + 0.5;
                assert!(
                    (numeric_force - analytic_force).abs() <= tol,
                    "grad mismatch atom {i} axis {axis}: analytic {analytic_force}, numeric {numeric_force}"
                );
            }
        }
    }

    #[test]
    fn bond_gradient_matches_finite_difference() {
        let z = [6u8, 8u8];
        let bonds = [[0usize, 1usize]];
        let orders = [BondOrder::Single];
        let model = build_model(&z, &bonds, &orders);
        let coords = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.18, 0.02, -0.01)];
        grad_check(&coords, &model);
    }

    #[test]
    fn angle_gradient_matches_finite_difference() {
        let z = [1u8, 8u8, 1u8];
        let bonds = [[0usize, 1usize], [1usize, 2usize]];
        let orders = [BondOrder::Single, BondOrder::Single];
        let model = build_model(&z, &bonds, &orders);
        let coords = [
            Vec3::new(0.09, 0.01, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(-0.03, 0.085, 0.02),
        ];
        grad_check(&coords, &model);
    }

    #[test]
    fn vdw_gradient_matches_finite_difference() {
        // Two unbonded carbons inside the WCA cutoff.
        let z = [6u8, 6u8];
        let bonds: [[usize; 2]; 0] = [];
        let orders: [BondOrder; 0] = [];
        let model = build_model(&z, &bonds, &orders);
        let coords = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.12, 0.03, 0.0)];
        grad_check(&coords, &model);
    }

    #[test]
    fn vdw_zero_beyond_cutoff() {
        let z = [6u8, 6u8];
        let model = build_model(&z, &[], &[]);
        // Two carbons well past r_cut (~0.34 nm): no energy, no force.
        let coords = [Vec3::ZERO, Vec3::new(0.6, 0.0, 0.0)];
        let mut f = vec![Vec3::ZERO; 2];
        let e = energy_forces(&coords, &model, &mut f);
        assert!(e.abs() < 1.0e-6, "energy should be 0 beyond cutoff, got {e}");
        assert!(f[0].length() < 1.0e-6 && f[1].length() < 1.0e-6);
    }

    #[test]
    fn single_bond_relaxes_to_equilibrium() {
        let z = [6u8, 6u8];
        let bonds = [[0usize, 1usize]];
        let orders = [BondOrder::Single];
        let model = build_model(&z, &bonds, &orders);
        let r0 = equilibrium_bond_length(6, 6, BondOrder::Single);
        // Start far too long.
        let mut coords = [Vec3::ZERO, Vec3::new(0.3, 0.0, 0.0)];
        let res = minimize(&mut coords, &model, MinimizeOpts::full_cleanup());
        let r = (coords[0] - coords[1]).length();
        assert!(!res.had_nan);
        assert!((r - r0).abs() < 2.0e-3, "C–C relaxed to {r}, expected {r0}");
    }

    #[test]
    fn water_relaxes_to_sensible_geometry() {
        let z = [8u8, 1u8, 1u8];
        let bonds = [[0usize, 1usize], [0usize, 2usize]];
        let orders = [BondOrder::Single, BondOrder::Single];
        let model = build_model(&z, &bonds, &orders);
        // Distorted start: wrong lengths, ~70° angle.
        let mut coords = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.13, 0.0, 0.0),
            Vec3::new(0.05, 0.12, 0.0),
        ];
        let res = minimize(&mut coords, &model, MinimizeOpts::full_cleanup());
        assert!(!res.had_nan);
        assert!(res.converged, "water should converge, fmax={}", res.final_force_norm);
        let oh1 = (coords[1] - coords[0]).length();
        let oh2 = (coords[2] - coords[0]).length();
        let r0 = equilibrium_bond_length(8, 1, BondOrder::Single);
        assert!((oh1 - r0).abs() < 3.0e-3, "O–H1 {oh1} vs {r0}");
        assert!((oh2 - r0).abs() < 3.0e-3, "O–H2 {oh2} vs {r0}");
        let a = (coords[1] - coords[0]).normalize();
        let b = (coords[2] - coords[0]).normalize();
        let angle = a.dot(b).clamp(-1.0, 1.0).acos().to_degrees();
        assert!((angle - 109.47).abs() < 5.0, "H–O–H angle {angle}°");
    }

    #[test]
    fn coincident_atoms_separate_without_nan() {
        let z = [6u8, 6u8];
        let bonds = [[0usize, 1usize]];
        let orders = [BondOrder::Single];
        let model = build_model(&z, &bonds, &orders);
        // Both atoms at exactly the same point.
        let mut coords = [Vec3::ZERO, Vec3::ZERO];
        let res = minimize(&mut coords, &model, MinimizeOpts::full_cleanup());
        assert!(!res.had_nan);
        let r = (coords[0] - coords[1]).length();
        let r0 = equilibrium_bond_length(6, 6, BondOrder::Single);
        assert!(coords.iter().all(|c| c.is_finite()));
        assert!((r - r0).abs() < 5.0e-3, "coincident pair separated to {r}");
    }

    #[test]
    fn isolated_atom_is_left_alone() {
        let z = [6u8];
        let model = build_model(&z, &[], &[]);
        let mut coords = [Vec3::new(0.1, 0.2, 0.3)];
        let res = minimize(&mut coords, &model, MinimizeOpts::full_cleanup());
        assert!(res.converged);
        assert_eq!(coords[0], Vec3::new(0.1, 0.2, 0.3));
    }

    #[test]
    fn disconnected_fragments_separate() {
        // Two unbonded carbons placed on top of each other must be pushed apart by vdW.
        let z = [6u8, 6u8];
        let model = build_model(&z, &[], &[]);
        let mut coords = [Vec3::ZERO, Vec3::new(0.02, 0.0, 0.0)];
        let res = minimize(&mut coords, &model, MinimizeOpts::full_cleanup());
        assert!(!res.had_nan);
        let r = (coords[0] - coords[1]).length();
        let sigma = vdw_radius(6) / TWO_POW_1_6;
        let r_cut = TWO_POW_1_6 * sigma;
        assert!(r >= r_cut - 1.0e-3, "vdW should separate to ~{r_cut}, got {r}");
    }

    /// A C–C–C–C chain with the central C1–C2 dihedral set to `phi_deg`. Bonds at
    /// equilibrium, ~109.5° angles; only the dihedral varies, so it isolates torsion
    /// (+ the 1-4 vdW between the end carbons).
    fn carbon_chain(phi_deg: f32) -> ([u8; 4], [[usize; 2]; 3], [BondOrder; 3], [Vec3; 4]) {
        let z = [6u8; 4];
        let bonds = [[0usize, 1], [1, 2], [2, 3]];
        let orders = [BondOrder::Single; 3];
        let l = equilibrium_bond_length(6, 6, BondOrder::Single);
        let theta = 109.5_f32.to_radians();
        let c1 = Vec3::ZERO;
        let c2 = Vec3::new(l, 0.0, 0.0);
        let c0 = c1 + l * Vec3::new(theta.cos(), theta.sin(), 0.0);
        // C2→C3 base direction (phi=0 → cis to C1→C0, both +y), rotated about x by phi.
        let base = Vec3::new(-theta.cos(), theta.sin(), 0.0);
        let phi = phi_deg.to_radians();
        let rot = Vec3::new(
            base.x,
            base.y * phi.cos() - base.z * phi.sin(),
            base.y * phi.sin() + base.z * phi.cos(),
        );
        let c3 = c2 + l * rot;
        (z, bonds, orders, [c0, c1, c2, c3])
    }

    #[test]
    fn torsion_gradient_matches_finite_difference() {
        let (z, bonds, orders, coords) = carbon_chain(40.0);
        let model = build_model(&z, &bonds, &orders);
        assert_eq!(model.torsions.len(), 1, "one sp3–sp3 torsion expected");
        grad_check(&coords, &model);
    }

    #[test]
    fn torsion_prefers_staggered_over_eclipsed() {
        let (z, bonds, orders, _) = carbon_chain(0.0);
        let model = build_model(&z, &bonds, &orders);
        let mut f = vec![Vec3::ZERO; 4];
        let e_eclipsed = energy_forces(&carbon_chain(0.0).3, &model, &mut f);
        let e_staggered = energy_forces(&carbon_chain(60.0).3, &model, &mut f);
        assert!(
            e_eclipsed > e_staggered,
            "eclipsed {e_eclipsed} should cost more than staggered {e_staggered}"
        );
    }

    #[test]
    fn hybridization_perception() {
        // Ethene-like: C=C, each carbon sp2; the triple-bonded carbons sp.
        let z = [6u8, 6u8];
        let bonds = [[0usize, 1usize]];
        assert_eq!(
            perceive_hybridization(&z, &bonds, &[BondOrder::Double]),
            vec![Hybrid::Terminal, Hybrid::Terminal] // degree 1 each → terminal
        );
        // Central carbon with two single bonds → sp3.
        let z3 = [1u8, 6u8, 1u8];
        let bonds3 = [[0usize, 1usize], [1usize, 2usize]];
        let h = perceive_hybridization(&z3, &bonds3, &[BondOrder::Single, BondOrder::Single]);
        assert_eq!(h[1], Hybrid::Sp3);
        // Central carbon with a double bond → sp2.
        let h2 = perceive_hybridization(&z3, &bonds3, &[BondOrder::Double, BondOrder::Single]);
        assert_eq!(h2[1], Hybrid::Sp2);
    }
}
