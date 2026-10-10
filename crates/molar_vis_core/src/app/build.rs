//! Free-function builders for hover-detail, glow, pick id-buffer geometry.
use super::*;

use crate::interactions::InteractionSet;


/// Build the hover detail "lens": a distance-faded CPK ball-and-stick of the atoms
/// near the cursor view-line (`detail.atoms`, found by the spatial grid). Rendered
/// over a Cartoon/Surface rep to reveal local atomic detail the abstraction hides.
pub(super) fn build_hover_detail(
    data: &crate::moldata::MolData,
    bonds: &[Bond],
    detail: &crate::scene::HoverDetail,
    state: &molar::prelude::State,
    n_atoms: usize,
    dashed_pbc: bool,
) -> geometry::GeometryData {
    let Some(index_str) = pick::index_selection_string(&detail.atoms) else {
        return geometry::GeometryData::default();
    };
    let Ok((_, sel)) = data.evaluate(&index_str) else {
        return geometry::GeometryData::default();
    };
    let bound = data.bind_with_state(&sel, state);
    let params = RepParams::BallAndStick {
        sphere_scale: 0.25, bond_radius: 0.04, bond_smoothing: 0.0, bond_color_blend: 0.0,
    };
    let mut geom = geometry::build(
        &bound,
        n_atoms,
        bonds,
        &params,
        ColorMethod::Element.into(),
        crate::material::Material::Opaque,
        None,
        dashed_pbc,
    );
    fade_by_ray(&mut geom, detail.ray_o, detail.ray_d, detail.radius);
    geom
}

/// Set each element's alpha by its perpendicular distance from the ray `o + t·d`:
/// opaque on-axis, fading to 0 at `radius` — so the lens dissolves softly into the
/// ribbon. The alpha is the color's top byte (matching the geometry packing).
pub(super) fn fade_by_ray(geom: &mut geometry::GeometryData, o: glam::Vec3, d: glam::Vec3, radius: f32) {
    const MAX_A: f32 = 235.0;
    let d = d.normalize_or_zero();
    let radius = radius.max(1e-3);
    let alpha_of = |p: [f32; 3]| -> u32 {
        let w = glam::Vec3::from(p) - o;
        let perp = (w - d * w.dot(d)).length();
        let f = (1.0 - perp / radius).clamp(0.0, 1.0);
        (f * MAX_A) as u32
    };
    let set = |c: u32, a: u32| (c & 0x00ff_ffff) | (a << 24);
    for s in &mut geom.spheres {
        s.color = set(s.color, alpha_of(s.center));
    }
    for c in &mut geom.cylinders {
        let mid = [
            (c.p0[0] + c.p1[0]) * 0.5,
            (c.p0[1] + c.p1[1]) * 0.5,
            (c.p0[2] + c.p1[2]) * 0.5,
        ];
        c.color = set(c.color, alpha_of(mid));
    }
}

/// Desaturate + dim every element's colour so a representation reads as **inactive**.
/// Used in Draw mode for every rep except the one being edited, to show at a glance that
/// the others are not active and can't be interacted with. Opacity (the alpha byte) is
/// left as-is so the draw order does not change.
pub(super) fn gray_out(geom: &mut geometry::GeometryData) {
    // Blend each channel most of the way to the colour's own luminance (desaturate),
    // then darken — a dim, near-grey version of the original colour.
    fn gray(c: u32) -> u32 {
        let a = c & 0xff00_0000;
        let r = (c & 0xff) as f32;
        let g = ((c >> 8) & 0xff) as f32;
        let b = ((c >> 16) & 0xff) as f32;
        let l = 0.299 * r + 0.587 * g + 0.114 * b;
        const DESAT: f32 = 0.85; // fraction blended toward grey
        const DIM: f32 = 0.55; // brightness kept
        let ch = |v: f32| (((v * (1.0 - DESAT) + l * DESAT) * DIM) as u32).min(255);
        a | (ch(b) << 16) | (ch(g) << 8) | ch(r)
    }
    for s in &mut geom.spheres {
        s.color = gray(s.color);
    }
    for c in &mut geom.cylinders {
        c.color = gray(c.color);
        c.color1 = gray(c.color1);
    }
    for l in &mut geom.lines {
        l.color = gray(l.color);
    }
    for v in &mut geom.mesh.vertices {
        v.color = gray(v.color);
    }
}

/// Build the selection glow geometry for one molecule: for each visible rep, the
/// rep's selection intersected with the highlighted `atoms`, built in that rep's
/// own style/params, merged into one geometry. Used for both the pending (lasso)
/// selection and the hover highlight. The element colors/materials are irrelevant
/// (the glow shaders emit a fixed cyan Fresnel rim), so the rep's own values are
/// reused. Cartoon/SecStruct reps are skipped until their SS cache exists (it's
/// filled by the same `rebuild_dirty` pass, just before this).
pub(super) fn build_glow(
    data: &crate::moldata::MolData,
    bonds: &[Bond],
    reps: &[Representation],
    atoms: &[usize],
    state: &State,
    n_atoms: usize,
    dashed_pbc: bool,
) -> geometry::GeometryData {
    let Some(index_str) = pick::index_selection_string(atoms) else {
        return geometry::GeometryData::default();
    };
    // Highlighted residues (resindex), for extracting the Cartoon sub-ribbon.
    let topo = data.topology();
    let res_set: std::collections::HashSet<u32> = atoms
        .iter()
        .filter_map(|&a| topo.get_atom(a).map(|at| at.get_resindex() as u32))
        .collect();
    let mut out = geometry::GeometryData::default();
    for rep in reps {
        if !rep.visible {
            continue;
        }
        // Cartoon: don't rebuild a (degenerate, divergent) subset ribbon — extract the
        // chosen residues' triangles straight from the parent's *exact* cached mesh.
        // Coincident geometry passes the glow pass's `≤` depth test cleanly (no z-fight,
        // no inflation) and a single residue still yields its ribbon segment.
        if matches!(rep.kind, RepKind::Cartoon) {
            if let Some(cache) = &rep.mesh_cache {
                out.append(cartoon_submesh(&cache.mesh, &res_set));
            }
            continue;
        }
        if geometry::needs_ss(&rep.params, rep.color) && rep.ss_cache.is_none() {
            continue;
        }
        // (rep selection) ∩ (pending atoms): glow only this rep's own atoms, in its
        // own style. Skip on an empty/invalid intersection.
        let combined = format!("({}) and ({})", rep.sel_text, index_str);
        let Ok((_, sel)) = data.evaluate(&combined) else {
            continue;
        };
        let bound = data.bind_with_state(&sel, state);
        let mut geom = geometry::build(
            &bound, n_atoms, bonds, &rep.params, rep.color_spec(), rep.material,
            rep.ss_cache.as_ref(), dashed_pbc,
        );
        // Surface re-builds the glow over the *subset* of selected atoms, so its mesh
        // nearly — but not exactly — coincides with the parent's (the grid isosurface
        // shifts at the subset boundary). Two near-coplanar surfaces z-fight, so push
        // the glow mesh a hair *outward* along its normals into a thin shell just in
        // front of the parent. (The glow pass writes no depth, so the shell's back
        // still fails the depth test and stays hidden.) Impostor glows coincide
        // exactly and need no offset; Cartoon reuses the parent mesh (handled above).
        inflate_mesh(&mut geom.mesh, GLOW_INFLATE);
        out.append(geom);
    }
    out
}

/// Extract the sub-ribbon of a cached Cartoon mesh for the residues in `res_set`:
/// keep a triangle when a majority (≥2) of its vertices belong to chosen residues
/// (a clean cut at residue boundaries), compacting the referenced vertices. The
/// result shares the parent's exact vertex positions, so the glow is coincident.
pub(super) fn cartoon_submesh(
    mesh: &geometry::MeshData,
    res_set: &std::collections::HashSet<u32>,
) -> geometry::GeometryData {
    let mut vertices: Vec<crate::render::MeshVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut remap: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    for tri in mesh.indices.chunks_exact(3) {
        let chosen = tri
            .iter()
            .filter(|&&v| res_set.contains(&mesh.vert_res[v as usize]))
            .count();
        if chosen < 2 {
            continue;
        }
        for &v in tri {
            let nv = *remap.entry(v).or_insert_with(|| {
                vertices.push(mesh.vertices[v as usize]);
                (vertices.len() - 1) as u32
            });
            indices.push(nv);
        }
    }
    geometry::GeometryData {
        mesh: geometry::MeshData { vertices, indices, ..Default::default() },
        ..Default::default()
    }
}

/// Atom-index bits in a pick id's y channel (the rest hold the rep index). 21 bits
/// → up to ~2M atoms/molecule and 2048 reps; ample for interactive systems.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const PICK_ATOM_BITS: u32 = 21;

/// The molecule's GPU pick geometry: what each visible rep **draws**, id-stamped with the
/// atom a hit resolves to (`[mol + 1, rep << PICK_ATOM_BITS | atom]`; x = 0 = no atom).
/// Per-atom reps give one sphere per drawn atom; a mesh rep (Cartoon / Surface) gives its
/// cached mesh, each vertex stamped with its source atom (`MeshData::vert_atom`), so the
/// pick hits the ribbon / surface itself. Periodic images are baked in. Mirrors the CPU
/// [`pick::pick`].
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn build_pick(
    mol: &scene::Molecule,
    mi: usize,
    state: &State,
) -> crate::render::PickGeometry {
    let _timing = crate::performance::span("pick-prepare");
    // Box lattice vectors (columns of the box matrix), for periodic image offsets.
    let box_vecs = state.pbox.as_ref().map(|pb| {
        let m = pb.get_matrix();
        [
            glam::Vec3::new(m[(0, 0)], m[(1, 0)], m[(2, 0)]),
            glam::Vec3::new(m[(0, 1)], m[(1, 1)], m[(2, 1)]),
            glam::Vec3::new(m[(0, 2)], m[(1, 2)], m[(2, 2)]),
        ]
    });
    let mut out = crate::render::PickGeometry::default();
    let pick_x = mi as u32 + 1;
    let mut smoothed_states = std::collections::HashMap::new();
    for (rj, rep) in mol.reps.iter().enumerate() {
        if !rep.visible {
            continue;
        }
        let offsets = match box_vecs {
            Some([a, b, c]) => rep.periodic.offsets(a, b, c),
            None => vec![glam::Vec3::ZERO],
        };
        let pick_rep = (rj as u32) << PICK_ATOM_BITS;
        if rep.kind.draws_mesh() {
            let Some(cache) = &rep.mesh_cache else { continue };
            let mesh = &cache.mesh;
            for off in &offsets {
                let base = out.vertices.len() as u32;
                for (v, &a) in mesh.vertices.iter().zip(&mesh.vert_atom) {
                    let p = glam::Vec3::from(v.pos) + *off;
                    // A vertex with no source atom still occludes, but never hits.
                    let pick = if a == geometry::NO_ATOM { [0, 0] } else { [pick_x, pick_rep | a] };
                    out.vertices.push(crate::render::PickVertex { pos: p.to_array(), pick });
                }
                out.indices.extend(mesh.indices.iter().map(|i| i + base));
            }
            continue;
        }
        let Some(sel) = &rep.sel else { continue };
        let disp_state: &State = if rep.smooth_window > 1 {
            smoothed_states
                .entry(rep.smooth_window)
                .or_insert_with(|| mol.trajectory.smoothed_state(rep.smooth_window))
                .as_ref()
                .unwrap_or(state)
        } else {
            state
        };
        let bound = mol.data.bind_with_state(sel, disp_state);
        for p in bound.iter_particle() {
            if !pick::rep_draws_atom(rep, p.id) {
                continue;
            }
            let base = glam::Vec3::new(p.pos.x, p.pos.y, p.pos.z);
            let radius = pick::effective_radius(&rep.params, p.atom);
            let id = [pick_x, pick_rep | (p.id as u32)];
            for off in &offsets {
                let c = base + *off;
                out.spheres.push(SphereInstance {
                    center: [c.x, c.y, c.z],
                    radius,
                    color: 0,
                    mat: 0,
                    pick: id,
                });
            }
        }
    }
    out
}

/// A molecule's currently displayed coordinates: the active trajectory frame, or the
/// static structure state. (Per-rep smoothing is ignored for interaction detection.)
fn displayed_state(mol: &scene::Molecule) -> &State {
    mol.trajectory
        .frames
        .get(mol.trajectory.current)
        .unwrap_or_else(|| mol.data.state())
}

fn v3(p: &molar::prelude::Pos) -> glam::Vec3 {
    glam::Vec3::new(p.x, p.y, p.z)
}

/// Which molecule index (in `scene.molecules`) an Interactions rep's partner resolves to,
/// and its rep index — matched by [`MoleculeSource`]. `None` = unset / partner lost (the
/// molecule is gone or the rep index is out of range).
///
/// **Group-following:** if the partner molecule belongs to a [`MolGroup`], the reference
/// is redirected to the group's **currently-shown member** (same rep index — shared reps
/// are the identical prefix on every shown member). So an interactions rep pointing at a
/// group's ligand automatically follows the group slider to the newly-shown molecule.
pub(super) fn partner_index(scene: &Scene, rep: &Representation) -> Option<(usize, usize)> {
    let (src, pr) = rep.partner.as_ref()?;
    let mut pmi = scene.molecules.iter().position(|m| &m.source == src)?;
    if let Some(gid) = scene.molecules[pmi].group {
        if let Some(gi) = scene.group_index(gid) {
            let g = &scene.groups[gi];
            if let Some(shown_mi) = g.members.get(g.current).and_then(|&id| scene.mol_index(id)) {
                pmi = shown_mi;
            }
        }
    }
    // Validate the (possibly redirected) rep actually exists.
    scene.molecules.get(pmi)?.reps.get(*pr)?;
    Some((pmi, *pr))
}

/// Gather everything one rep's selection contributes to interaction detection: heavy
/// atoms (+ attached H / hydrophobic flag / halogen antecedent), aromatic rings within
/// the selection (centroid + normal from the displayed frame; ring atom sets come from
/// the molecule's cached `interaction_rings`), and charged groups. `res_key` is made
/// unique per (molecule, residue) via `mol_idx` so the detector's residue-level dedup
/// never merges same-index residues from two molecules.
fn gather_set(mol: &scene::Molecule, mol_idx: usize, sel: &molar::prelude::Sel, state: &State) -> InteractionSet {
    let topo = mol.data.topology();
    let n = state.coords.len();
    let res_base = (mol_idx as u64) << 40;
    let coord = |i: usize| state.coords.get(i).map(v3);

    // Adjacency over the whole molecule (bonds index the full topology).
    let mut neigh: Vec<Vec<u32>> = vec![Vec::new(); n];
    for bond in &mol.bonds {
        let [a, b] = bond.pair();
        if a < n && b < n {
            neigh[a].push(b as u32);
            neigh[b].push(a as u32);
        }
    }
    let anum_of = |i: usize| topo.get_atom(i).map(|a| a.get_atomic_number()).unwrap_or(0);

    // Selected atoms → heavy-atom AtomInfo + a membership mask (for rings/charges).
    let bound = mol.data.bind_with_state(sel, state);
    let mut in_sel = vec![false; n];
    let mut atoms = Vec::new();
    for p in bound.iter_particle() {
        in_sel[p.id] = true;
        let anum = p.atom.get_atomic_number();
        if anum == 1 {
            continue; // H rides in its heavy neighbour's `attached_h`
        }
        let mut only_ch = anum == 6;
        let mut attached_h = Vec::new();
        let mut antecedent = None;
        for &nb in &neigh[p.id] {
            let nb = nb as usize;
            let na = anum_of(nb);
            if only_ch && !matches!(na, 1 | 6) {
                only_ch = false;
            }
            if na == 1 {
                if let Some(c) = coord(nb) {
                    attached_h.push(c);
                }
            } else if antecedent.is_none() {
                antecedent = coord(nb);
            }
        }
        atoms.push(crate::interactions::AtomInfo {
            pos: glam::Vec3::new(p.pos.x, p.pos.y, p.pos.z),
            atomicnum: anum,
            res_key: res_base | (p.atom.get_resindex() as u64),
            only_ch_neighbors: only_ch,
            attached_h,
            antecedent,
        });
    }

    // Aromatic rings fully inside the selection → centroid + plane normal.
    let mut rings = Vec::new();
    for ring in mol.interaction_rings.as_deref().unwrap_or(&[]) {
        if ring.len() < 3 || !ring.iter().all(|&i| i < n && in_sel[i]) {
            continue;
        }
        let pts: Vec<glam::Vec3> = ring.iter().filter_map(|&i| coord(i)).collect();
        if pts.len() < 3 {
            continue;
        }
        let center = pts.iter().copied().sum::<glam::Vec3>() / pts.len() as f32;
        rings.push(crate::interactions::RingInfo {
            center,
            normal: crate::interactions::ring_normal(&pts),
            res_key: res_base | (topo.get_atom(ring[0]).map(|a| a.get_resindex() as u64).unwrap_or(0)),
        });
    }

    let (cations, anions) = charged_groups(topo, &neigh, state, &in_sel, res_base, n);
    InteractionSet { atoms, rings, cations, anions }
}

/// Detect charged groups in the selection: standard amino-acid sidechains / termini by
/// residue+atom name, plus ligand functional groups (carboxylate, phosphate/sulfate,
/// guanidinium) by connectivity. Heuristic — real formal charges aren't available; this
/// covers the common protein–ligand salt-bridge / π-cation cases.
fn charged_groups(
    topo: &molar::prelude::Topology,
    neigh: &[Vec<u32>],
    state: &State,
    in_sel: &[bool],
    res_base: u64,
    n: usize,
) -> (Vec<crate::interactions::ChargeGroup>, Vec<crate::interactions::ChargeGroup>) {
    use crate::interactions::ChargeGroup;
    use std::collections::HashMap;
    let coord = |i: usize| state.coords.get(i).map(v3);
    let name = |i: usize| topo.get_atom(i).map(|a| a.get_name().to_string()).unwrap_or_default();
    let anum = |i: usize| topo.get_atom(i).map(|a| a.get_atomic_number()).unwrap_or(0);

    // Group selected atoms by residue.
    let mut byres: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, &sel) in in_sel.iter().enumerate().take(n) {
        if sel {
            if let Some(a) = topo.get_atom(i) {
                byres.entry(a.get_resindex()).or_default().push(i);
            }
        }
    }
    let centroid = |ids: &[usize]| -> Option<glam::Vec3> {
        let mut sum = glam::Vec3::ZERO;
        let mut k = 0;
        for &i in ids {
            if let Some(c) = coord(i) {
                sum += c;
                k += 1;
            }
        }
        (k > 0).then(|| sum / k as f32)
    };

    let mut cations = Vec::new();
    let mut anions = Vec::new();
    for (ridx, ids) in &byres {
        let rk = res_base | (*ridx as u64);
        let resname = topo
            .get_atom(ids[0])
            .map(|a| a.get_resname().to_ascii_uppercase())
            .unwrap_or_default();
        let pick = |names: &[&str]| -> Vec<usize> {
            ids.iter().copied().filter(|&i| names.contains(&name(i).as_str())).collect()
        };
        let mut push = |grp: Vec<usize>, positive: bool| {
            if let Some(c) = centroid(&grp) {
                let cg = ChargeGroup { center: c, res_key: rk };
                if positive {
                    cations.push(cg);
                } else {
                    anions.push(cg);
                }
            }
        };
        match resname.as_str() {
            "ARG" => push(pick(&["CZ", "NH1", "NH2", "NE"]), true),
            "LYS" => push(pick(&["NZ"]), true),
            "HIS" | "HID" | "HIE" | "HIP" | "HSD" | "HSE" | "HSP" => {
                push(pick(&["ND1", "NE2", "CE1", "CG", "CD2"]), true)
            }
            "ASP" => push(pick(&["CG", "OD1", "OD2"]), false),
            "GLU" => push(pick(&["CD", "OE1", "OE2"]), false),
            _ => {
                // Ligand / non-standard residue: functional groups by connectivity.
                for &i in ids {
                    let deg_heavy = |j: usize| neigh[j].iter().filter(|&&k| anum(k as usize) > 1).count();
                    if anum(i) == 6 {
                        // Carboxylate: C bonded to ≥2 terminal O.
                        let os: Vec<usize> = neigh[i]
                            .iter()
                            .map(|&k| k as usize)
                            .filter(|&k| anum(k) == 8 && deg_heavy(k) <= 1)
                            .collect();
                        if os.len() >= 2 {
                            let mut g = os;
                            g.push(i);
                            push(g, false);
                        }
                        // Guanidinium / amidinium: C bonded to ≥3 N.
                        let ns = neigh[i].iter().filter(|&&k| anum(k as usize) == 7).count();
                        if ns >= 3 {
                            let mut g: Vec<usize> = neigh[i].iter().map(|&k| k as usize).collect();
                            g.push(i);
                            push(g, true);
                        }
                    } else if matches!(anum(i), 15 | 16) {
                        // Phosphate / sulfate: P/S bonded to ≥3 O.
                        let os: Vec<usize> =
                            neigh[i].iter().map(|&k| k as usize).filter(|&k| anum(k) == 8).collect();
                        if os.len() >= 3 {
                            let mut g = os;
                            g.push(i);
                            push(g, false);
                        }
                    }
                }
            }
        }
        // C-terminal carboxylate (any residue carrying a terminal-oxygen name).
        let oxt = pick(&["OXT", "OT1", "OT2", "OT"]);
        if !oxt.is_empty() {
            let mut g = oxt;
            g.extend(pick(&["C", "O"]));
            push(g, false);
        }
    }
    (cations, anions)
}

/// Build the dashed contact-line geometry for an **Interactions** rep (`mol[self_mi]
/// .reps[rep_idx]`): detect the enabled interaction types between this rep's selection and
/// its partner rep's selection (possibly in another molecule) and emit colored dashed
/// lines. Returns empty geometry if the partner is unset / stale / self / has no selection.
/// Reads two molecules, so it runs outside the `&mut`-iterator rebuild loop (the ring
/// caches of both molecules must already be populated — see `rebuild_dirty`).
pub(crate) fn build_interactions(
    scene: &Scene,
    self_mi: usize,
    rep_idx: usize,
) -> geometry::GeometryData {
    let empty = geometry::GeometryData::default;
    let Some(mol) = scene.molecules.get(self_mi) else {
        return empty();
    };
    let Some(rep) = mol.reps.get(rep_idx) else {
        return empty();
    };
    let RepParams::Interactions { settings } = rep.params else {
        return empty();
    };
    let Some((pmi, prep_idx)) = partner_index(scene, rep) else {
        return empty(); // unset / partner lost
    };
    if pmi == self_mi && prep_idx == rep_idx {
        return empty(); // a rep can't point at itself
    }
    let Some(pmol) = scene.molecules.get(pmi) else {
        return empty();
    };
    let Some(prep) = pmol.reps.get(prep_idx) else {
        return empty();
    };
    let (Some(sel_a), Some(sel_b)) = (&rep.sel, &prep.sel) else {
        return empty();
    };

    let set_a = gather_set(mol, self_mi, sel_a, displayed_state(mol));
    let set_b = gather_set(pmol, pmi, sel_b, displayed_state(pmol));
    let found = crate::interactions::detect(&set_a, &set_b, &settings.detect());
    geometry::GeometryData {
        lines: geometry::interaction_lines(&found, settings.line_width),
        ..Default::default()
    }
}

/// World-space (nm) outward shell offset for the active-selection glow mesh — large
/// enough to dominate the sub-Ångström divergence between the subset and parent
/// cartoon splines (so no z-fighting), small enough to read as a tight halo.
pub(super) const GLOW_INFLATE: f32 = 0.025;

/// Offset every mesh vertex outward along its normal by `d` nm (a thin shell).
pub(super) fn inflate_mesh(mesh: &mut geometry::MeshData, d: f32) {
    for v in &mut mesh.vertices {
        v.pos[0] += v.normal[0] * d;
        v.pos[1] += v.normal[1] * d;
        v.pos[2] += v.normal[2] * d;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tpath(f: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/")).join(f)
    }

    /// An Interactions rep whose partner is a MolGroup member follows the group's
    /// currently-shown member — so sliding the group updates the interactions target.
    #[test]
    fn partner_follows_group_shown_member() {
        let rd = crate::settings::RepDefaults::default();
        let bp = crate::data::bonds::BondParams::default();
        let host = crate::data::load(&tpath("2lao.pdb")).expect("load 2lao.pdb");
        let records = crate::data::load_records(&tpath("ligands20.sdf"), &bp).expect("load sdf");
        assert!(records.len() >= 3, "need a few group members");

        let mut scene = scene::Scene::default();
        scene.add(host, &rd); // mol 0 = host, carries the interactions rep
        scene.add_group(
            records,
            scene::MoleculeSource::File(tpath("ligands20.sdf")),
            "ligands".into(),
            &rd,
        );

        // Interactions rep on the host, partner = group member 0's shared rep (index 0).
        let member0_src = scene.molecules[1].source.clone();
        let mut rep = Representation::new(RepKind::Interactions);
        rep.partner = Some((member0_src, 0));
        scene.molecules[0].reps.push(rep);
        let irep = scene.molecules[0].reps.len() - 1;

        // Shown member is 0 → resolves to member 0's molecule.
        let (pmi0, _) = partner_index(&scene, &scene.molecules[0].reps[irep]).unwrap();
        assert_eq!(pmi0, scene.mol_index(scene.groups[0].members[0]).unwrap());

        // Slide to member 2 → the partner follows the newly-shown member.
        assert!(scene.switch_group_member(0, 2));
        let (pmi2, _) = partner_index(&scene, &scene.molecules[0].reps[irep]).unwrap();
        assert_eq!(pmi2, scene.mol_index(scene.groups[0].members[2]).unwrap());
        assert_ne!(pmi0, pmi2, "partner molecule changed with the group slider");
    }
}


/// Evaluate a dirty selection without constructing or uploading geometry.
/// Returns true when an empty selection cleared previously drawn geometry.
pub(super) fn refresh_selection(data: &crate::moldata::MolData, rep: &mut Representation) -> bool {
    if !rep.sel_dirty {
        return false;
    }
    let _timing = crate::performance::span("selection");
    let mut cleared = false;
    // Parse + evaluate the selection (against the System's own
    // state). On error keep the previous selection/geometry and
    // just surface the message.
    match data.evaluate(rep.sel_text.as_str()) {
        Ok((expr, sel)) => {
            rep.expr = Some(expr);
            rep.sel = Some(sel);
            rep.sel_error = None;
            rep.sel_error_span = None;
            rep.sel_empty = false;
            rep.geom_dirty = true;
        }
        // Valid selection that matches no atoms: not an error — drop
        // the geometry (render nothing), keep the text, and flag the
        // field. The viewport must re-render to clear the old mesh.
        Err(scene::EvalError::Empty) => {
            rep.expr = None;
            rep.sel = None;
            rep.sel_error = None;
            rep.sel_error_span = None;
            rep.sel_empty = true;
            rep.gpu = Default::default();
            rep.primitive_cache = None;
            rep.cartoon_cache = None;
            #[cfg(not(target_arch = "wasm32"))]
            { rep.geometry_job = None; rep.geometry_waiting = false; }
            rep.cache_geometry(Default::default(), 0, false, true);
            rep.mesh_cache = None;
            cleared = true;
        }
        Err(scene::EvalError::Invalid { message, span }) => {
            // molar trims the input before parsing, so shift the span
            // past any leading whitespace to align it with the field's
            // text (leading whitespace is ASCII, so bytes == chars).
            let lead = rep
                .sel_text
                .bytes()
                .take_while(|b| *b == b' ' || *b == b'\t')
                .count();
            rep.sel_error = Some(message);
            rep.sel_error_span = span.map(|r| r.start + lead..r.end + lead);
            rep.sel_empty = false;
        }
    }
    rep.sel_dirty = false;
    cleared
}

pub(super) struct UnobstructedAtoms {
    pub target: Vec<(glam::Vec3, f32)>,
    pub occluders: Vec<(glam::Vec3, f32)>,
    pub visual_bounds: Vec<(glam::Vec3, f32)>,
}

/// Gather each atom once, refreshing only selections relevant to this view.
pub(super) fn gather_unobstructed_atoms(
    scene: &mut Scene,
    targets: &[(usize, usize)],
) -> Result<UnobstructedAtoms, String> {
    if targets.is_empty() {
        return Err("no target reps given".to_string());
    }
    for &(mi, ri) in targets {
        let mol = scene
            .molecules
            .get(mi)
            .ok_or_else(|| format!("no molecule {mi}"))?;
        if mol.reps.get(ri).is_none() {
            return Err(format!("no rep {ri} on molecule {mi}"));
        }
    }
    let target_reps: std::collections::HashSet<_> = targets.iter().copied().collect();
    for (mi, mol) in scene.molecules.iter_mut().enumerate() {
        for (ri, rep) in mol.reps.iter_mut().enumerate() {
            if target_reps.contains(&(mi, ri))
                || (mol.visible && rep.visible && rep.kind != RepKind::Interactions)
            {
                if refresh_selection(&mol.data, rep) {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        mol.pick_dirty = true;
                    }
                }
            }
        }
    }
    let mut out = UnobstructedAtoms {
        target: Vec::new(),
        occluders: Vec::new(),
        visual_bounds: Vec::new(),
    };
    let mut seen = std::collections::HashSet::new();
    for &(mi, ri) in targets {
        let mol = &scene.molecules[mi];
        let rep = &mol.reps[ri];
        let sel = rep
            .sel
            .as_ref()
            .ok_or("target rep has no evaluated selection")?;
        let smoothed = (rep.smooth_window > 1)
            .then(|| mol.trajectory.smoothed_state(rep.smooth_window)).flatten();
        let bound = mol.data.bind_with_state(sel, smoothed.as_ref().unwrap_or(mol.render_state()));
        for p in bound.iter_particle() {
            let pos = glam::Vec3::new(p.pos.x, p.pos.y, p.pos.z);
            // Bounds belong to representations: overlapping selections may render the
            // same atom at different sizes. Deduplicate only the visibility search.
            let radius = rep.params.visual_radius(p.atom.vdw());
            out.visual_bounds.push((pos, radius));
            if seen.insert((mi, p.id)) {
                out.target.push((pos, p.atom.vdw()));
            }
        }
        if let Some(geom) = rep.cached_geometry(false) {
            for vertex in &geom.mesh.vertices {
                let pos = glam::Vec3::from_array(vertex.pos);
                out.visual_bounds.push((pos, 0.0));
            }
        }
    }
    if out.target.is_empty() {
        return Err("target reps select no atoms".to_string());
    }
    for (mi, mol) in scene.molecules.iter().enumerate() {
        if !mol.visible {
            continue;
        }
        for (ri, rep) in mol.reps.iter().enumerate() {
            if target_reps.contains(&(mi, ri)) || !rep.visible || rep.kind == RepKind::Interactions
            {
                continue;
            }
            let Some(sel) = &rep.sel else { continue };
            let bound = mol.data.bind_with_state(sel, mol.render_state());
            for p in bound.iter_particle() {
                // Target atoms are already blockers in the combined projection.
                if seen.insert((mi, p.id)) {
                    out.occluders
                        .push((glam::Vec3::new(p.pos.x, p.pos.y, p.pos.z), p.atom.vdw()));
                }
            }
        }
    }
    Ok(out)
}

enum GeometryBuild {
    Ready(geometry::GeometryData),
    #[cfg(not(target_arch = "wasm32"))]
    Background(crate::geometry_jobs::Input),
}

fn prepare_geometry(
    bound: &(impl ParticleIterProvider + AtomProvider),
    state: &State,
    n_atoms: usize,
    params: &geometry::RepParams,
    color: crate::color::ColorSpec,
    material: crate::material::Material,
    ss: Option<&SsMap>,
    cartoon: Option<&geometry::CartoonCache>,
    dashed: bool,
    background: bool,
    build: impl FnOnce() -> geometry::GeometryData,
) -> GeometryBuild {
    #[cfg(not(target_arch = "wasm32"))]
    if background {
        use crate::geometry_jobs::Input;
        let input = if let Some(cartoon) = cartoon {
            Some(Input::Cartoon(cartoon.snapshot(state, dashed)))
        } else if let geometry::RepParams::Surface {
            probe,
            quality,
            smoothing,
        } = *params
        {
            let colors = crate::color::Colorizer::new(color, bound, n_atoms, ss);
            Some(Input::Surface(
                geometry::SurfaceInput::new(bound, &colors, probe, quality, smoothing),
                material,
            ))
        } else {
            None
        };
        if let Some(input) = input.filter(|i| i.worth_offloading()) {
            return GeometryBuild::Background(input);
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (
        bound, state, n_atoms, params, color, material, ss, cartoon, dashed, background,
    );
    GeometryBuild::Ready(build())
}

#[cfg(not(target_arch = "wasm32"))]
fn queue_geometry(rep: &mut Representation, input: crate::geometry_jobs::Input) {
    match crate::geometry_jobs::Job::try_start(input) {
        Ok(job) => {
            rep.geometry_job = Some(job);
            rep.geometry_waiting = false;
            rep.geom_dirty = false;
            rep.coords_dirty = false;
        }
        Err(_) => rep.geometry_waiting = true, // Retry latest input; never queue stale frames.
    }
}

/// Recompile dirty selections and rebuild/reupload dirty geometry. Returns true if any
/// geometry was uploaded (so the frame needs re-rendering).
///
/// A free function rather than an `App` method: it reaches exactly four of `App`'s fields,
/// and they are disjoint at every call site — so naming them in the signature says what the
/// hottest function in the app actually touches, and lets a caller keep the rest of `self`
/// borrowed while it runs.
pub(super) fn rebuild_dirty(
scene: &mut Scene,
renderer: &SceneRenderer,
settings: &Settings,
view_dirty: bool,
rs: &eframe::egui_wgpu::RenderState,
// The rep currently open in the drawing editor, as `(molecule, rep index)`. When set,
// every *other* visible rep is greyed out (built desaturated + dim) to show it's inactive
// in Draw mode. `None` when not drawing → nothing greyed. The caller marks all reps
// `geom_dirty` when this changes, so a rebuild reaches every rep.
gray_active: Option<(crate::scene::MolId, usize)>,
background: bool,
) -> bool {
    #[cfg(target_arch = "wasm32")]
    let _ = background;
    let mut changed = false;
    // Whether wrapping bonds are drawn as dashed minimum-image half-bonds (read
    // once: the molecule loop below borrows `self.scene` mutably).
    let dashed = settings.behavior.dashed_pbc_bonds;
    // A structural change (molecule add/remove/reorder/visibility) shifts molecule
    // indices, so the GPU pick geometry's baked `mol+1` ids must be rebuilt.
    #[cfg(not(target_arch = "wasm32"))]
    let structure_changed = view_dirty;
    // Which molecules had geometry/coords (re)built this pass — used by the
    // Interactions second pass to rebuild a contact rep when either endpoint
    // molecule changed (its own or the partner's coords/selection).
    let mut mol_changed = vec![false; scene.molecules.len()];
    for (mi, mol) in scene.molecules.iter_mut().enumerate() {
        // Only visible molecules are drawn into the pick id-buffer, so don't build
        // pick geometry for hidden ones (e.g. the N−1 unshown members of a group).
        // A hidden molecule made visible later sets `view_dirty`, re-marking it.
        #[cfg(not(target_arch = "wasm32"))]
        if structure_changed && mol.visible {
            mol.pick_dirty = true;
        }
        #[cfg(not(target_arch = "wasm32"))]
        let pick_pending = mol.pick_dirty;
        #[cfg(target_arch = "wasm32")]
        let pick_pending = false;
        let any_rep_dirty = mol
            .reps
            .iter()
            .any(|r| r.sel_dirty || r.geom_dirty || r.coords_dirty || r.geometry_pending());
        if !(any_rep_dirty
            || (mol.show_box && mol.box_dirty)
            || mol.aromatic_dirty
            || mol.glow_dirty
            || mol.hover_dirty
            || mol.hover_detail_dirty
            || pick_pending)
        {
            continue;
        }
        // The coordinates to render: the current trajectory frame, read by
        // reference (no copy into the System), or the static structure state.
        let render_state: &State = match mol.trajectory.frames.get(mol.trajectory.current) {
            Some(frame) => frame,
            None => mol.data.state(),
        };
        let n_atoms = mol.n_atoms;
        let mol_id = mol.id;
        // Whether any rep's geometry was (re)built this pass — if so and there's
        // an active selection, its glow must follow the new style/coords.
        let mut rep_geom_changed = false;
        // Reuse each smoothing window within this rebuild; no stale frame cache.
        let mut smoothed_states = std::collections::HashMap::new();
        for (j, rep) in mol.reps.iter_mut().enumerate() {
            // Grey out every rep except the one open in the editor (Draw mode only).
            let grayed = matches!(gray_active, Some((tid, tr)) if !(mol_id == tid && j == tr));
            changed |= refresh_selection(&mol.data, rep);
            #[cfg(not(target_arch = "wasm32"))]
            let background_allowed = background && gray_active.is_none()
                && mol.trajectory.frames.len() <= 1 && !mol.trajectory.playing
                && !rep.geometry_jobs_disabled;
            #[cfg(target_arch = "wasm32")]
            let background_allowed = false;
            #[cfg(not(target_arch = "wasm32"))]
            {
                if rep.geometry_job.is_some() && (rep.geom_dirty || rep.coords_dirty || !background_allowed) {
                    rep.geometry_job = None;
                    rep.geometry_waiting = false;
                    // A synchronous capture/edit must install the current state now.
                    if !background_allowed { rep.geom_dirty = true; }
                }
                if let Some(job) = &rep.geometry_job {
                    match job.poll() {
                        crate::geometry_jobs::Poll::Pending => continue,
                        crate::geometry_jobs::Poll::Complete(geom) => {
                            rep.geometry_job = None;
                            renderer.update(rs, &mut rep.gpu, &geom);
                            rep.cache_geometry(geom, n_atoms, dashed, false);
                            changed = true;
                            rep_geom_changed = true;
                            continue;
                        }
                        crate::geometry_jobs::Poll::Failed => {
                            rep.geometry_job = None;
                            rep.geometry_jobs_disabled = true;
                            rep.geom_dirty = true;
                            log::warn!("geometry worker failed; reverting to synchronous builds");
                        }
                    }
                }
                if !background_allowed { rep.geometry_waiting = false; }
                if background_allowed && rep.geometry_waiting
                    && matches!(rep.kind, RepKind::Cartoon | RepKind::Surface)
                    && !crate::geometry_jobs::Job::capacity_available()
                { continue; }
            }
            // Interactions reps read a *partner* molecule, so they can't be built
            // inside this `&mut`-iterator loop — a second pass below handles them.
            // Their selection was just evaluated (above); leave the geometry dirty
            // flags for that pass to consume.
            if matches!(rep.kind, RepKind::Interactions) {
                continue;
            }
            let Some(sel) = &rep.sel else {
                rep.geom_dirty = false;
                rep.coords_dirty = false;
                continue;
            };

            if !rep.geom_dirty && !rep.coords_dirty {
                continue;
            }
            let state: &State = if rep.smooth_window > 1 {
                smoothed_states
                    .entry(rep.smooth_window)
                    .or_insert_with(|| mol.trajectory.smoothed_state(rep.smooth_window))
                    .as_ref()
                    .unwrap_or(render_state)
            } else {
                render_state
            };

            if rep.geom_dirty {
                // Full structural rebuild: (re)compute secondary structure
                // into the cache, build geometry, recreate GPU buffers.
                let (geom, fresh_ss, primitive_cache, cartoon_cache) = {
                    let bound = mol.data.bind_with_state(sel, state);
                    let ss = geometry::needs_ss(&rep.params, rep.color)
                        .then(|| SsMap::compute(&bound, rep.ss_algo));
                    let primitive_cache = geometry::PrimitiveCache::new(
                        &bound, n_atoms, &mol.bonds, &rep.params, rep.color_spec(), rep.material,
                        ss.as_ref(),
                    );
                    let cartoon_cache = geometry::CartoonCache::new(
                        &bound, n_atoms, &rep.params, rep.color_spec(), rep.material, ss.as_ref(),
                    );
                    let mut result = prepare_geometry(&bound, state, n_atoms, &rep.params,
                        rep.color_spec(), rep.material, ss.as_ref(), cartoon_cache.as_ref(), dashed,
                        background_allowed, || primitive_cache.as_ref().map_or_else(
                            || cartoon_cache.as_ref().map_or_else(
                                || geometry::build(&bound, n_atoms, &mol.bonds, &rep.params, rep.color_spec(), rep.material, ss.as_ref(), dashed),
                                |cache| cache.build(state, dashed),
                            ),
                            |cache| cache.build(state, dashed),
                        ));
                    match &mut result {
                        GeometryBuild::Ready(geom) => { if grayed { gray_out(geom); } }
                        #[cfg(not(target_arch = "wasm32"))]
                        GeometryBuild::Background(_) => (),
                    }
                    (result, ss, primitive_cache, cartoon_cache)
                };
                rep.ss_cache = fresh_ss;
                rep.primitive_cache = primitive_cache;
                rep.cartoon_cache = cartoon_cache;
                let geom = match geom {
                    GeometryBuild::Ready(geom) => geom,
                    #[cfg(not(target_arch = "wasm32"))]
                    GeometryBuild::Background(input) => { queue_geometry(rep, input); continue; }
                };
                rep.gpu = renderer.upload(rs, &geom);
                rep.cache_geometry(geom, n_atoms, dashed, grayed);
                rep.geom_dirty = false;
                rep.coords_dirty = false;
                changed = true;
                rep_geom_changed = true;
            } else if rep.coords_dirty {
                // Coordinates-only frame change: rebuild geometry reusing the
                // cached secondary structure (no DSSP), then update the
                // existing GPU buffers in place (no reallocation).
                let geom = {
                    let bound = mol.data.bind_with_state(sel, state);
                    let mut result = prepare_geometry(&bound, state, n_atoms, &rep.params,
                        rep.color_spec(), rep.material, rep.ss_cache.as_ref(), rep.cartoon_cache.as_ref(),
                        dashed, background_allowed, || rep.primitive_cache.as_ref().map_or_else(
                            || rep.cartoon_cache.as_ref().map_or_else(
                                || geometry::build(&bound, n_atoms, &mol.bonds, &rep.params, rep.color_spec(), rep.material, rep.ss_cache.as_ref(), dashed),
                                |cache| cache.build(state, dashed),
                            ),
                            |cache| cache.build(state, dashed),
                        ));
                    match &mut result {
                        GeometryBuild::Ready(geom) => { if grayed { gray_out(geom); } }
                        #[cfg(not(target_arch = "wasm32"))]
                        GeometryBuild::Background(_) => (),
                    }
                    result
                };
                let geom = match geom {
                    GeometryBuild::Ready(geom) => geom,
                    #[cfg(not(target_arch = "wasm32"))]
                    GeometryBuild::Background(input) => { queue_geometry(rep, input); continue; }
                };
                renderer.update(rs, &mut rep.gpu, &geom);
                rep.cache_geometry(geom, n_atoms, dashed, grayed);
                rep.coords_dirty = false;
                changed = true;
                rep_geom_changed = true;
            }
        }
        drop(smoothed_states);
        mol_changed[mi] = rep_geom_changed;
        // Periodic-box wireframe: (re)build when dirty, regardless of whether
        // it's currently shown — both the molecule-level box toggle *and* a
        // rep's periodic `Box` toggle draw this geometry, and the latter isn't
        // tracked by `box_dirty`, so keep `box_gpu` ready whenever a box exists.
        // Use the current frame's box (tracks NPT box changes); fall back to the
        // structure's own box when a trajectory frame carries none.
        if mol.box_dirty {
            let pb = render_state
                .pbox
                .as_ref()
                .or_else(|| mol.data.state().pbox.as_ref());
            let lines = pb.map(geometry::box_wireframe).unwrap_or_default();
            let geom = geometry::GeometryData { lines, ..Default::default() };
            renderer.update(rs, &mut mol.box_gpu, &geom);
            mol.box_dirty = false;
            changed = true;
        }

        // Aromatic-ring circles: depth-tested 3-D line geometry (built from the
        // perceived rings at the displayed coords), so they occlude correctly.
        if mol.aromatic_dirty || (rep_geom_changed && !mol.aromatic_rings.is_empty()) {
            let lines = geometry::aromatic_circles(&mol.aromatic_rings, &render_state.coords);
            let geom = geometry::GeometryData { lines, ..Default::default() };
            renderer.update(rs, &mut mol.aromatic_gpu, &geom);
            mol.aromatic_dirty = false;
            changed = true;
        }

        // If any rep's geometry was rebuilt (style/selection/coords changed) and
        // there's a pending/hover highlight, rebuild its glow so it follows.
        if rep_geom_changed && mol.pending.is_some() {
            mol.glow_dirty = true;
        }
        if rep_geom_changed && mol.hover.is_some() {
            mol.hover_dirty = true;
        }
        if rep_geom_changed && mol.hover_detail.is_some() {
            mol.hover_detail_dirty = true;
        }
        if rep_geom_changed {
            mol.hover_grid = None; // its filtered atom set depends on the reps/coords
        }

        // Active-selection glow: rebuild the pending atoms in each rep's own
        // style (so the highlight glows in the current style), or clear it. Runs
        // after the rep loop so Cartoon reps' `ss_cache` is already populated.
        if mol.glow_dirty {
            let geom = match &mol.pending {
                Some(pending) => build_glow(
                    &mol.data, &mol.bonds, &mol.reps, &pending.atoms, render_state, n_atoms,
                    dashed,
                ),
                None => geometry::GeometryData::default(),
            };
            renderer.update(rs, &mut mol.glow_gpu, &geom);
            mol.glow_dirty = false;
            changed = true;
        }
        // Hover highlight: same builder, the hovered residue's atoms (steady glow).
        if mol.hover_dirty {
            let geom = match &mol.hover {
                Some(atoms) => build_glow(
                    &mol.data, &mol.bonds, &mol.reps, atoms, render_state, n_atoms, dashed,
                ),
                None => geometry::GeometryData::default(),
            };
            renderer.update(rs, &mut mol.hover_gpu, &geom);
            mol.hover_dirty = false;
            changed = true;
        }
        // Hover detail lens: faded CPK ball-and-stick of the atoms near the
        // cursor view-line (built from `hover_detail`), over a Cartoon/Surface rep.
        if mol.hover_detail_dirty {
            let geom = match &mol.hover_detail {
                Some(d) => {
                    build_hover_detail(&mol.data, &mol.bonds, d, render_state, n_atoms, dashed)
                }
                None => geometry::GeometryData::default(),
            };
            renderer.update(rs, &mut mol.hover_detail_gpu, &geom);
            mol.hover_detail_dirty = false;
            changed = true;
        }
        // GPU pick geometry (native): rebuild when the molecule's geometry/coords
        // changed (rep_geom_changed covers both) or it was flagged dirty (init /
        // structure change). Mirrors the atoms CPU `pick` would ray-cast.
        #[cfg(not(target_arch = "wasm32"))]
        if rep_geom_changed || mol.pick_dirty {
            let geom = build_pick(mol, mi, render_state);
            renderer.update_pick(rs, &mut mol.pick_gpu, &geom);
            mol.pick_dirty = false;
            // No `changed = true`: pick geometry isn't drawn in render_scene, so
            // it doesn't require a scene re-render on its own.
        }
    }

    // --- Second pass: Interactions reps ---
    // They render contacts between their own selection and a *partner* rep in
    // (possibly) another molecule, so they must read two molecules at once —
    // impossible inside the `&mut`-iterator loop above. Rebuild one when its own
    // flags are dirty OR either endpoint molecule's geometry/coords changed.
    let mut inter_jobs: Vec<(usize, usize)> = Vec::new();
    let mut need_rings: std::collections::HashSet<usize> = std::collections::HashSet::new();
    // A structural change (visibility toggle, group member switch via the slider,
    // molecule add/remove) doesn't rebuild any rep's geometry, but it can change
    // which molecule a partner resolves to (group-following) or its visibility — so
    // rebuild every interactions rep on it too.
    let structural = view_dirty;
    for (mi, mol) in scene.molecules.iter().enumerate() {
        for (ri, rep) in mol.reps.iter().enumerate() {
            if !matches!(rep.kind, RepKind::Interactions) {
                continue;
            }
            let partner = partner_index(scene, rep).map(|(p, _)| p);
            let self_changed =
                rep.geom_dirty || rep.coords_dirty || mol_changed[mi] || structural;
            let partner_changed = partner.is_some_and(|pmi| mol_changed[pmi]);
            if self_changed || partner_changed {
                inter_jobs.push((mi, ri));
                // Both endpoint molecules need their aromatic-ring cache for the π
                // interactions (populated mutably here, before the immutable build).
                need_rings.insert(mi);
                if let Some(pmi) = partner {
                    need_rings.insert(pmi);
                }
            }
        }
    }
    for mi in need_rings {
        scene.molecules[mi].ensure_interaction_rings();
    }
    for (mi, ri) in inter_jobs {
        // Compute (immutable scene borrow), then upload + store (drops the borrow
        // first, so mutating the rep afterwards is fine).
        let geom = build_interactions(scene, mi, ri);
        log::debug!(
            "interactions rep {mi}:{ri} → {} dashed line vertices",
            geom.lines.len()
        );
        let gpu = renderer.upload(rs, &geom);
        let rep = &mut scene.molecules[mi].reps[ri];
        rep.gpu = gpu;
        rep.geom_dirty = false;
        rep.coords_dirty = false;
        changed = true;
    }
    changed
}

#[cfg(test)]
mod gray_out_tests {
    use super::*;
    use crate::render::SphereInstance;

    /// A saturated colour comes back desaturated (its channels pulled toward each other),
    /// dimmed (its dominant channel darker), with its opacity (alpha) untouched.
    #[test]
    fn gray_out_desaturates_dims_and_keeps_alpha() {
        // Saturated red, fully opaque (RGBA little-endian: R=0xFF, A=0xFF).
        let red = 0xFF00_00FFu32;
        let mut geom = geometry::GeometryData {
            spheres: vec![SphereInstance {
                center: [0.0; 3],
                radius: 1.0,
                color: red,
                mat: 0,
                pick: [0, 0],
            }],
            ..Default::default()
        };
        gray_out(&mut geom);
        let c = geom.spheres[0].color;
        let (r, g, b, a) = (
            c & 0xff,
            (c >> 8) & 0xff,
            (c >> 16) & 0xff,
            (c >> 24) & 0xff,
        );
        assert_eq!(a, 0xff, "opacity must be preserved");
        assert!(r < 0xff, "dominant channel must be dimmed: {r}");
        assert!(g > 0 && b > 0, "grey channels must lift toward luminance");
        // Much closer to neutral grey than the original (|R−G| was 255).
        assert!((r as i32 - g as i32).abs() < 64, "must read as desaturated");
    }
}

#[cfg(test)]
mod unobstructed_tests {
    use super::*;
    use molar::prelude::LenProvider;

    fn fixture_scene() -> Scene {
        let path =
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb"));
        let mut scene = Scene::default();
        for _ in 0..2 {
            let raw = crate::data::load_records(path, &crate::data::bonds::BondParams::default())
                .unwrap()
                .remove(0);
            scene.add(raw, &crate::settings::RepDefaults::default());
        }
        scene
    }

    fn rep(selection: &str) -> Representation {
        let mut rep = Representation::new(RepKind::Vdw);
        rep.sel_text = selection.into();
        rep.geom_dirty = false;
        rep
    }

    #[test]
    fn deduplicates_targets_and_blockers_without_merging_distinct_molecules() {
        let mut scene = fixture_scene();
        scene.molecules[0].reps = vec![
            rep("index 0 1"),
            rep("index 1 2"),
            rep("index 0 1 2 3"),
            rep("index 3 4"),
        ];
        scene.molecules[1].reps = vec![rep("index 0 1")];
        let atoms = gather_unobstructed_atoms(&mut scene, &[(0, 0), (0, 1), (0, 0)]).unwrap();
        assert_eq!(atoms.target.len(), 3);
        assert_eq!(atoms.occluders.len(), 4); // 3,4 on mol 0; 0,1 on mol 1.
        assert!(scene.molecules[0].reps.iter().all(|r| !r.gpu.has_geometry()));
        assert!(scene.molecules[0]
            .reps
            .iter()
            .all(|r| !r.sel_dirty && r.geom_dirty));
    }

    #[test]
    fn framing_includes_visual_radii_for_overlapping_targets() {
        let mut scene = fixture_scene();
        scene.molecules[0].reps = vec![rep("index 0"), rep("index 0")];
        scene.molecules[0].reps[0].params = RepParams::Licorice {
            bond_radius: 0.03, bond_color_blend: 0.0,
        };
        scene.molecules[0].reps[1].params = RepParams::Vdw { scale: 2.0 };
        let small = gather_unobstructed_atoms(&mut scene, &[(0, 0)]).unwrap();
        let bounds = |atoms: &UnobstructedAtoms| atoms.visual_bounds.iter().fold(
            (glam::Vec3::splat(f32::INFINITY), glam::Vec3::splat(f32::NEG_INFINITY)),
            |(min, max), &(p, r)| (min.min(p - glam::Vec3::splat(r)), max.max(p + glam::Vec3::splat(r))));
        let (min, max) = bounds(&small);
        assert!((max - min - glam::Vec3::splat(0.06)).length() < 1e-5);
        let both = gather_unobstructed_atoms(&mut scene, &[(0, 0), (0, 1)]).unwrap();
        assert_eq!(both.target.len(), 1);
        let (pos, vdw) = both.target[0];
        let (min, max) = bounds(&both);
        assert!((min - (pos - glam::Vec3::splat(vdw * 2.0))).length() < 1e-5);
        assert!((max - (pos + glam::Vec3::splat(vdw * 2.0))).length() < 1e-5);
        let reverse = gather_unobstructed_atoms(&mut scene, &[(0, 1), (0, 0)]).unwrap();
        assert_eq!(bounds(&both), bounds(&reverse));
    }

    #[test]
    fn skips_hidden_molecules_reps_and_interactions() {
        let mut scene = fixture_scene();
        let mut hidden = rep("index 3");
        hidden.visible = false;
        let mut interactions = rep("index 4");
        interactions.kind = RepKind::Interactions;
        scene.molecules[0].reps = vec![rep("index 0"), rep("index 1"), hidden, interactions];
        scene.molecules[1].visible = false;
        let atoms = gather_unobstructed_atoms(&mut scene, &[(0, 0)]).unwrap();
        assert_eq!(atoms.target.len(), 1);
        assert_eq!(atoms.occluders.len(), 1);
        assert!(scene.molecules[0].reps[2].sel_dirty);
        assert!(scene.molecules[0].reps[3].sel_dirty);
        assert!(scene.molecules[1].reps[0].sel_dirty);
    }

    #[test]
    fn selection_refresh_preserves_invalid_selection_and_handles_empty_selection() {
        let mut scene = fixture_scene();
        let mol = &mut scene.molecules[0];
        let mut r = rep("index 0 1");
        assert!(!refresh_selection(&mol.data, &mut r));
        r.sel_text = "  definitely_invalid_keyword".into();
        r.sel_dirty = true;
        refresh_selection(&mol.data, &mut r);
        assert_eq!(r.sel.as_ref().unwrap().len(), 2);
        assert!(r.sel_error.is_some());
        r.sel_text = "index 999999".into();
        r.sel_dirty = true;
        assert!(refresh_selection(&mol.data, &mut r));
        assert!(r.sel.is_none() && r.sel_empty && r.sel_error.is_none());
        assert!(!r.sel_dirty);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod geometry_job_tests {
    use super::*;

    #[test]
    #[ignore = "requires native GPU; verifies asynchronous mesh invalidation and capture barriers"]
    fn background_meshes_follow_edits_and_never_replace_newer_geometry() {
        let rs = crate::render::test_gpu();
        let settings = Settings::default();
        let renderer = SceneRenderer::new(&rs, &settings.rendering);
        let raw = crate::data::load(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb"
        ))).unwrap();
        let mut scene = Scene::default(); scene.add(raw, &settings.reps);
        scene.molecules[0].reps = vec![Representation::new(RepKind::Surface)];
        rebuild_dirty(&mut scene, &renderer, &settings, true, &rs, None, true);
        assert!(scene.molecules[0].reps[0].geometry_pending());
        assert!(scene.molecules[0].reps[0].cached_geometry(settings.behavior.dashed_pbc_bonds).is_none());
        {
            let rep = &mut scene.molecules[0].reps[0];
            rep.color = crate::color::ColorMethod::Solid([255, 0, 0, 255]);
            rep.geom_dirty = true;
        }
        rebuild_dirty(&mut scene, &renderer, &settings, false, &rs, None, true);
        // The synchronous capture path must cancel old work and install the current edit.
        rebuild_dirty(&mut scene, &renderer, &settings, false, &rs, None, false);
        let rep = &scene.molecules[0].reps[0];
        assert!(!rep.geometry_pending());
        let mesh = &rep.mesh_cache.as_ref().unwrap().mesh;
        assert!(!mesh.vertices.is_empty());
        assert!(mesh.vertices.iter().all(|v| v.color == 0xff0000ff));
        let expected = mesh.clone();
        for _ in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            rebuild_dirty(&mut scene, &renderer, &settings, false, &rs, None, true);
        }
        let mesh = &scene.molecules[0].reps[0].mesh_cache.as_ref().unwrap().mesh;
        assert_eq!(bytemuck::cast_slice::<_,u8>(&mesh.vertices), bytemuck::cast_slice::<_,u8>(&expected.vertices));
        {
            let rep = &mut scene.molecules[0].reps[0];
            rep.geom_dirty = true;
        }
        rebuild_dirty(&mut scene, &renderer, &settings, false, &rs, None, true);
        {
            let rep = &mut scene.molecules[0].reps[0];
            rep.sel_text = "name NONEXISTENT".into(); rep.sel_dirty = true;
        }
        rebuild_dirty(&mut scene, &renderer, &settings, false, &rs, None, true);
        let rep = &scene.molecules[0].reps[0];
        assert!(!rep.geometry_pending() && rep.mesh_cache.is_none() && !rep.gpu.has_geometry());
        // Removal/replacement also drops the receiver/cancellation token.
        scene.molecules[0].reps = vec![Representation::new(RepKind::Surface)];
        rebuild_dirty(&mut scene, &renderer, &settings, false, &rs, None, true);
        scene.molecules[0].reps = vec![Representation::new(RepKind::Vdw)];
        rebuild_dirty(&mut scene, &renderer, &settings, false, &rs, None, false);
        assert!(scene.molecules[0].reps[0].gpu.has_geometry());
        assert!(scene.molecules[0].reps[0].mesh_cache.is_none());
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod unobstructed_render_tests {
    use super::*;
    use crate::camera::{Camera, Projection};
    use glam::Vec3;

    fn bounds(spheres: &[(Vec3, f32)]) -> (Vec3, Vec3) {
        spheres.iter().fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(min, max), &(p, r)| (min.min(p - Vec3::splat(r)), max.max(p + Vec3::splat(r))))
    }

    #[test]
    #[ignore = "requires GPU; renders the actual unobstructed ligand fit at several viewport shapes"]
    fn unobstructed_ligand_fits_rendered_viewport() {
        let rs = crate::render::test_gpu();
        let settings = Settings::default();
        let mut renderer = SceneRenderer::new(&rs, &settings.rendering);
        let raw = crate::data::load_records(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"),
            "/../../tests/ligands20.sdf")), &Default::default()).unwrap().remove(0);
        let mut scene = Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        let capture = |renderer: &mut SceneRenderer, scene: &Scene, camera: &Camera, w, h| {
            let cap = renderer.capture_begin(&rs, w, h, camera.view(), camera.proj(w as f32 / h as f32),
                camera.is_perspective(), camera.cue_uniform(), camera.ao_uniform(),
                camera.shadow_uniform(), camera.background, camera.eye_depth_range(), scene);
            rs.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            cap.read()
        };
        for kind in [RepKind::Licorice, RepKind::Vdw, RepKind::BallAndStick] {
            let mut rep = Representation::new(kind);
            rep.sel_text = "all".into();
            scene.molecules[0].reps = vec![rep];
            rebuild_dirty(&mut scene, &renderer, &settings, true, &rs, None, false);
            let atoms = gather_unobstructed_atoms(&mut scene, &[(0, 0)]).unwrap();
            let dir = crate::unobstructed::best_unobstructed_direction(&atoms.target, &atoms.occluders, 256);
            let (min, max) = bounds(&atoms.visual_bounds);
            let mut camera = Camera::frame_bbox(min, max, 0.9);
            camera.orientation = crate::unobstructed::look_along_quat(dir);
            camera.depth_cue.enabled = false;
            camera.ao.enabled = false;
            camera.shadow.enabled = false;
            camera.background = crate::camera::Background::for_theme(false);
            for projection in [Projection::Orthographic, Projection::Perspective] {
                camera.projection = projection;
                for (w, h) in [(585, 687), (320, 320), (800, 400)] {
                    if kind == RepKind::Licorice && projection == Projection::Orthographic && w == 585 {
                        camera.focus_bbox(min, max);
                        let before = capture(&mut renderer, &scene, &camera, w, h);
                        if let Ok(dir) = std::env::var("MOLAR_VIS_TEST_IMAGES") {
                            std::fs::create_dir_all(&dir).unwrap();
                            before.save(std::path::Path::new(&dir).join("unobstructed_before.png")).unwrap();
                        }
                    }
                    camera.focus_visual_bounds(&atoms.visual_bounds, w as f32 / h as f32, 1.0);
                    let image = capture(&mut renderer, &scene, &camera, w, h);
                    let background = image.get_pixel(0, 0).0;
                    let is_background = |x, y| (0..3).all(|i| image.get_pixel(x, y)[i].abs_diff(background[i]) <= 1);
                    assert!((0..w).all(|x| is_background(x, 0) && is_background(x, h - 1))
                        && (0..h).all(|y| is_background(0, y) && is_background(w - 1, y)),
                        "clipped image: {kind:?}/{projection:?}/{w}x{h}");
                    let mut min_px = [w, h];
                    let mut max_px = [0, 0];
                    for (x, y, _) in image.enumerate_pixels() {
                        if !is_background(x, y) {
                            min_px[0] = min_px[0].min(x); min_px[1] = min_px[1].min(y);
                            max_px[0] = max_px[0].max(x); max_px[1] = max_px[1].max(y);
                        }
                    }
                    let fill = ((max_px[0] - min_px[0]) as f32 / w as f32)
                        .max((max_px[1] - min_px[1]) as f32 / h as f32);
                    assert!(fill > 0.7 && fill < 0.92, "not a tight visual fit: {kind:?}/{projection:?}/{w}x{h}: {fill}");
                    if let Ok(dir) = std::env::var("MOLAR_VIS_TEST_IMAGES") {
                        std::fs::create_dir_all(&dir).unwrap();
                        image.save(std::path::Path::new(&dir).join(format!("unobstructed_{kind:?}_{projection:?}_{w}x{h}.png"))).unwrap();
                    }
                }
            }
        }
    }
}
