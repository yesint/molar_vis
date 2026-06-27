//! Free-function builders for hover-detail, glow, pick id-buffer geometry.
use super::*;


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
        ColorMethod::Element,
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
        .filter_map(|&a| topo.get_atom(a).map(|at| at.resindex as u32))
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
            &bound, n_atoms, bonds, &rep.params, rep.color, rep.material,
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
            if !pick::atom_in_rep(rep.kind, p.atom.name.as_str()) {
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
