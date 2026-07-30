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
    let params = RepParams::BallAndStick { sphere_scale: 0.25, bond_radius: 0.04 };
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
            if let Some(cache) = &rep.cartoon_cache {
                out.append(cartoon_submesh(cache, &res_set));
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
        mesh: geometry::MeshData { vertices, indices, vert_res: Vec::new() },
        ..Default::default()
    }
}

/// Atom-index bits in a pick id's y channel (the rest hold the rep index). 21 bits
/// → up to ~2M atoms/molecule and 2048 reps; ample for interactive systems.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const PICK_ATOM_BITS: u32 = 21;

/// Build the GPU **pick** geometry for one molecule (index `mi`): an id-stamped
/// sphere per *pickable* atom — exactly the atoms CPU `pick` ray-casts (eligible
/// atoms of each visible rep, at their displayed position and effective radius). The
/// id packs `[mi+1, rep<<21 | atom]` so the readback decodes back to (mol, rep, atom).
/// **Periodic images are baked in**: a rep with periodic display emits one sphere per
/// atom per drawn image (shifted by the lattice offset), so the single-camera pick
/// pass covers every image — matching what CPU `pick` tests. The id is the same for
/// all images, so a hit on any image still reports the (central) atom.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn build_pick(mol: &scene::Molecule, mi: usize, state: &State) -> geometry::GeometryData {
    // Box lattice vectors (columns of the box matrix), for periodic image offsets.
    let box_vecs = state.pbox.as_ref().map(|pb| {
        let m = pb.get_matrix();
        [
            glam::Vec3::new(m[(0, 0)], m[(1, 0)], m[(2, 0)]),
            glam::Vec3::new(m[(0, 1)], m[(1, 1)], m[(2, 1)]),
            glam::Vec3::new(m[(0, 2)], m[(1, 2)], m[(2, 2)]),
        ]
    });
    let mut spheres: Vec<SphereInstance> = Vec::new();
    for (rj, rep) in mol.reps.iter().enumerate() {
        if !rep.visible {
            continue;
        }
        let Some(sel) = &rep.sel else { continue };
        let smoothed = (rep.smooth_window > 1)
            .then(|| mol.trajectory.smoothed_state(rep.smooth_window))
            .flatten();
        let disp_state: &State = smoothed.as_ref().unwrap_or(state);
        let offsets = match box_vecs {
            Some([a, b, c]) => rep.periodic.offsets(a, b, c),
            None => vec![glam::Vec3::ZERO],
        };
        let bound = mol.data.bind_with_state(sel, disp_state);
        let pick_x = mi as u32 + 1;
        let pick_rep = (rj as u32) << PICK_ATOM_BITS;
        for p in bound.iter_particle() {
            if !pick::atom_in_rep(rep.kind, p.atom.get_name()) {
                continue;
            }
            let base = glam::Vec3::new(p.pos.x, p.pos.y, p.pos.z);
            let radius = pick::effective_radius(&rep.params, p.atom);
            let id = [pick_x, pick_rep | (p.id as u32)];
            for off in &offsets {
                let c = base + *off;
                spheres.push(SphereInstance {
                    center: [c.x, c.y, c.z],
                    radius,
                    color: 0,
                    mat: 0,
                    pick: id,
                });
            }
        }
    }
    geometry::GeometryData { spheres, ..Default::default() }
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
) -> bool {
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
            .any(|r| r.sel_dirty || r.geom_dirty || r.coords_dirty);
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
        // Whether any rep's geometry was (re)built this pass — if so and there's
        // an active selection, its glow must follow the new style/coords.
        let mut rep_geom_changed = false;
        for rep in &mut mol.reps {
            if rep.sel_dirty {
                // Parse + evaluate the selection (against the System's own
                // state). On error keep the previous selection/geometry and
                // just surface the message.
                match mol.data.evaluate(rep.sel_text.as_str()) {
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
                        changed = true;
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

            // Trajectory smoothing: a transient Savitzky–Golay blend of the
            // frames around `current`, computed here and dropped after the
            // build (nothing stored). Falls back to the raw current frame.
            let smoothed = (rep.smooth_window > 1)
                .then(|| mol.trajectory.smoothed_state(rep.smooth_window))
                .flatten();
            let state: &State = smoothed.as_ref().unwrap_or(render_state);

            if rep.geom_dirty {
                // Full structural rebuild: (re)compute secondary structure
                // into the cache, build geometry, recreate GPU buffers.
                let (geom, fresh_ss) = {
                    let bound = mol.data.bind_with_state(sel, state);
                    let ss = geometry::needs_ss(&rep.params, rep.color)
                        .then(|| SsMap::compute(&bound, rep.ss_algo));
                    let geom = geometry::build(
                        &bound, n_atoms, &mol.bonds, &rep.params, rep.color_spec(), rep.material,
                        ss.as_ref(), dashed,
                    );
                    (geom, ss)
                };
                rep.ss_cache = fresh_ss;
                rep.gpu = renderer.upload(rs, &geom);
                // Cache the cartoon ribbon CPU mesh (with residue tags) for the
                // selection glow to extract sub-ribbons from; clear for other styles.
                rep.cartoon_cache = if matches!(rep.kind, RepKind::Cartoon) {
                    Some(geom.mesh)
                } else {
                    None
                };
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
                    geometry::build(
                        &bound, n_atoms, &mol.bonds, &rep.params, rep.color_spec(), rep.material,
                        rep.ss_cache.as_ref(), dashed,
                    )
                };
                renderer.update(rs, &mut rep.gpu, &geom);
                if matches!(rep.kind, RepKind::Cartoon) {
                    rep.cartoon_cache = Some(geom.mesh); // keep the glow's cache fresh
                }
                rep.coords_dirty = false;
                changed = true;
                rep_geom_changed = true;
            }
        }
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
            mol.box_gpu = renderer.upload(rs, &geom);
            mol.box_dirty = false;
            changed = true;
        }

        // Aromatic-ring circles: depth-tested 3-D line geometry (built from the
        // perceived rings at the displayed coords), so they occlude correctly.
        if mol.aromatic_dirty || (rep_geom_changed && !mol.aromatic_rings.is_empty()) {
            let lines = geometry::aromatic_circles(&mol.aromatic_rings, &render_state.coords);
            let geom = geometry::GeometryData { lines, ..Default::default() };
            mol.aromatic_gpu = renderer.upload(rs, &geom);
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
            mol.glow_gpu = renderer.upload(rs, &geom);
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
            mol.hover_gpu = renderer.upload(rs, &geom);
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
            mol.hover_detail_gpu = renderer.upload(rs, &geom);
            mol.hover_detail_dirty = false;
            changed = true;
        }
        // GPU pick geometry (native): rebuild when the molecule's geometry/coords
        // changed (rep_geom_changed covers both) or it was flagged dirty (init /
        // structure change). Mirrors the atoms CPU `pick` would ray-cast.
        #[cfg(not(target_arch = "wasm32"))]
        if rep_geom_changed || mol.pick_dirty {
            let geom = build_pick(mol, mi, render_state);
            mol.pick_gpu = renderer.upload(rs, &geom);
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
