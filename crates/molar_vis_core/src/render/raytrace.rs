//! GPU ray tracer (VMD-Tachyon / PyMOL-`ray` quality): ray-traced ambient occlusion,
//! shadows, and lighting over the scene primitives. This module holds the **CPU side** —
//! gathering the scene's primitives into GPU-friendly arrays and building one BVH over
//! all of them — plus the GPU half (storage buffers + the compute tracer + resolve). A
//! BVH is mandatory even for spheres: a brute-force per-pixel × AO/shadow-sample loop
//! over thousands of atoms is far too slow.
//!
//! All representation kinds are traced: VDW / ball-and-stick / licorice **spheres** and
//! **cylinders** (analytic), and cartoon / surface **triangle meshes**. One BVH spans all
//! three; leaves carry **type-tagged** primitive indices.
//!
//! The tracer is **WebGPU/native only** (it needs storage buffers + compute, which WebGL2
//! lacks); callers gate on `DownlevelFlags::COMPUTE_SHADERS`. Layout is shared with
//! `shaders/raytrace.wgsl`, so the structs here are `#[repr(C)]` and packed into `vec4`
//! lanes to avoid std430 padding surprises.

use bytemuck::{Pod, Zeroable};
use eframe::egui_wgpu::RenderState;
use glam::Vec3;
use wgpu::util::DeviceExt as _;

use crate::scene::Scene;

/// Max primitives per BVH leaf.
const LEAF_SIZE: usize = 4;
/// SAH bin count (longest-axis binning).
const BINS: usize = 12;
/// Root depth 0; depth 31 leaves fit all 32-entry shader DFS stacks.
const MAX_BVH_DEPTH: usize = 31;

// Primitive type tags, packed into the top 2 bits of each `prim_indices` entry.
const TAG_SHIFT: u32 = 30;
const TAG_MASK: u32 = (1 << TAG_SHIFT) - 1;
const TAG_SPHERE: u32 = 0;
const TAG_CYLINDER: u32 = 1;
const TAG_TRIANGLE: u32 = 2;
fn tag(typ: u32, idx: usize) -> u32 {
    (typ << TAG_SHIFT) | (idx as u32 & TAG_MASK)
}

/// A sphere primitive: `c = (center.xyz, radius)`, `m = (color, mat, envelope_group, _)`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct GpuSphere {
    pub c: [f32; 4],
    pub m: [u32; 4],
}

/// A **capsule** primitive (cylinder wall + a hemispherical cap at each end, like the
/// rasterizer's bonds): `c0 = (p0.xyz, radius)`, `c1 = (p1.xyz, color_blend)`,
/// The high bits of `flags` store the envelope group (0 disables clipping).
/// `m = (color_p0, mat, color_p1, flags)` — two-tone, split at the midpoint.
/// `flags & FLAG_FLAT_ENDS` drops the caps (for stand-ins for flat-ended line quads).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct GpuCylinder {
    pub profile: [f32; 4],
    pub lane: [f32; 4],
    pub c0: [f32; 4],
    pub c1: [f32; 4],
    pub m: [u32; 4],
}

/// A shared mesh vertex: `p = (pos.xyz, bitcast(color))`, `n = (normal.xyz, bitcast(mat))`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct GpuMeshVertex {
    pub p: [f32; 4],
    pub n: [f32; 4],
}

/// A mesh triangle: three vertex indices; `.w` is 1 for a closed surface or cartoon.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct GpuTriangle {
    pub i: [u32; 4],
}

fn mesh_closed_flag(kind: crate::geometry::RepKind) -> u32 {
    // Cartoons are capped, thick ribbons/tubes, including their concave inner side.
    u32::from(matches!(kind, crate::geometry::RepKind::Surface | crate::geometry::RepKind::Cartoon))
}

/// A flattened BVH node, 32 bytes (two `vec4`): `lo.xyz` / `hi.xyz` are the AABB; the
/// `.w` lanes carry the link + count as bit-cast `u32`s. **`count == 0` ⇒ interior**
/// (`lo.w` = left child index; right child is `left + 1`, allocated contiguously);
/// **`count > 0` ⇒ leaf** (`lo.w` = first index into `prim_indices`, `hi.w` = count).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct BvhNode {
    pub lo: [f32; 4],
    pub hi: [f32; 4],
}

// `link`/`count`/`min`/`max` mirror the WGSL bit-unpacking and are exercised by the CPU
// traversal in the tests; `new` is the one the Rust builder calls.
#[cfg_attr(not(test), allow(dead_code))]
impl BvhNode {
    fn new(min: Vec3, max: Vec3, link: u32, count: u32) -> Self {
        Self {
            lo: [min.x, min.y, min.z, f32::from_bits(link)],
            hi: [max.x, max.y, max.z, f32::from_bits(count)],
        }
    }
    fn link(&self) -> u32 {
        self.lo[3].to_bits()
    }
    fn count(&self) -> u32 {
        self.hi[3].to_bits()
    }
    fn min(&self) -> Vec3 {
        Vec3::new(self.lo[0], self.lo[1], self.lo[2])
    }
    fn max(&self) -> Vec3 {
        Vec3::new(self.hi[0], self.hi[1], self.hi[2])
    }
}

/// The CPU-side ray-tracing scene: the flattened primitive arrays + one BVH over all of
/// them. Uploaded to storage buffers by the GPU half ([`Self::gather`]).
#[derive(Default)]
pub struct RtScene {
    pub spheres: Vec<GpuSphere>,
    pub cylinders: Vec<GpuCylinder>,
    pub mesh_verts: Vec<GpuMeshVertex>,
    pub triangles: Vec<GpuTriangle>,
    /// BVH nodes (node 0 is the root). Empty when there are no primitives.
    pub nodes: Vec<BvhNode>,
    /// Per-leaf primitive references: `(type << 30) | local_index`, partitioned so each
    /// leaf owns a contiguous slice.
    pub prim_indices: Vec<u32>,
}

impl RtScene {
    #[cfg(test)]
    pub(super) fn from_test_mesh(mesh: &crate::geometry::MeshData, kind: crate::geometry::RepKind) -> Self {
        let mut s = Self::default();
        s.mesh_verts = mesh
            .vertices
            .iter()
            .map(|v| GpuMeshVertex {
                p: [v.pos[0], v.pos[1], v.pos[2], f32::from_bits(v.color)],
                n: [v.normal[0], v.normal[1], v.normal[2], f32::from_bits(v.mat)],
            })
            .collect();
        s.triangles = mesh
            .indices
            .chunks_exact(3)
            .map(|t| GpuTriangle {
                i: [t[0], t[1], t[2], mesh_closed_flag(kind)],
            })
            .collect();
        let aabbs: Vec<_> = s
            .triangles
            .iter()
            .map(|t| triangle_aabb(&s.mesh_verts, t.i[0], t.i[1], t.i[2]))
            .collect();
        let _timing = crate::performance::span("rt-bvh");
        let (nodes, order) = build_bvh(&aabbs);
        s.nodes = nodes;
        s.prim_indices = order
            .into_iter()
            .map(|i| tag(TAG_TRIANGLE, i as usize))
            .collect();
        s
    }

    /// Whether there's anything to trace.
    pub fn is_empty(&self) -> bool {
        self.prim_indices.is_empty()
    }

    /// Gather visible primitives, reusing clean raster CPU geometry when available,
    /// and build the BVH. Dirty or uncached reps use the displayed frame / smoothing.
    /// `dashed_pbc` matches the live render's setting.
    #[cfg(test)]
    pub fn gather(scene: &Scene, view: RtView, dashed_pbc: bool) -> Self {
        let mut s = Self::default();
        let mut aabbs: Vec<Aabb> = Vec::new();
        let mut tags: Vec<u32> = Vec::new();
        {
            let _timing = crate::performance::span("rt-collect");
            s.collect(scene, view, dashed_pbc, &mut aabbs, &mut tags);
        }
        if aabbs.is_empty() {
            return s;
        }
        let _timing = crate::performance::span("rt-bvh");
        let (nodes, order) = build_bvh(&aabbs);
        s.nodes = nodes;
        s.prim_indices = order.into_iter().map(|i| tags[i as usize]).collect();
        s
    }

    /// Append each visible rep's geometry, accumulating primitive AABBs + type-tags.
    /// Camera-dependent line widths and bond offsets are converted on every gather.
    fn gather_with_bvh_cache(
        scene: &Scene, view: RtView, dashed: bool, cache: &mut Option<BvhCache>,
    ) -> Self {
        let mut data = Self::default();
        let mut bounds = Vec::new();
        let mut tags = Vec::new();
        {
            let _timing = crate::performance::span("rt-collect");
            data.collect(scene, view, dashed, &mut bounds, &mut tags);
        }
        if bounds.is_empty() { *cache = None; return data; }
        let counts = [data.spheres.len(), data.cylinders.len(), data.triangles.len()];
        if let Some(previous) = cache.as_mut().filter(|c| c.counts == counts && c.refits < 8) {
            let _timing = crate::performance::span("rt-bvh-refit");
            let mut by_type: [Vec<Aabb>; 3] = std::array::from_fn(|i| vec![Aabb::empty(); counts[i]]);
            for (&tagged, &bound) in tags.iter().zip(&bounds) {
                by_type[(tagged >> TAG_SHIFT) as usize][(tagged & TAG_MASK) as usize] = bound;
            }
            for index in (0..previous.nodes.len()).rev() {
                let node = previous.nodes[index];
                let mut bound = Aabb::empty();
                if node.count() == 0 {
                    let left = previous.nodes[node.link() as usize];
                    let right = previous.nodes[node.link() as usize + 1];
                    bound = Aabb { min: left.min().min(right.min()), max: left.max().max(right.max()) };
                } else {
                    for &tagged in &previous.order[node.link() as usize..(node.link() + node.count()) as usize] {
                        bound.extend(by_type[(tagged >> TAG_SHIFT) as usize][(tagged & TAG_MASK) as usize]);
                    }
                }
                previous.nodes[index] = BvhNode::new(bound.min, bound.max, node.link(), node.count());
            }
            if bvh_cost(&previous.nodes) <= previous.build_cost * 1.5 {
                previous.refits += 1;
                data.nodes = std::mem::take(&mut previous.nodes);
                data.prim_indices = std::mem::take(&mut previous.order);
                return data;
            }
        }
        let _timing = crate::performance::span("rt-bvh");
        let (nodes, order) = build_bvh(&bounds);
        data.nodes = nodes;
        data.prim_indices = order.into_iter().map(|i| tags[i as usize]).collect();
        *cache = Some(BvhCache { nodes: Vec::new(), order: Vec::new(), counts,
            build_cost: bvh_cost(&data.nodes), refits: 0 });
        data
    }

    fn collect(
        &mut self,
        scene: &Scene,
        view: RtView,
        dashed_pbc: bool,
        aabbs: &mut Vec<Aabb>,
        tags: &mut Vec<u32>,
    ) {
        use crate::geometry;
        use crate::secstruct::SsMap;

        let mut next_envelope_group = 1u32;
        for (mi, mol) in scene.molecules.iter().enumerate() {
            if !mol.visible {
                continue;
            }
            let render_state = match mol.trajectory.frames.get(mol.trajectory.current) {
                Some(frame) => frame,
                None => mol.data.state(),
            };
            // Periodic-image translations, read from the same box `render_scene` uses
            // (`mol.data.state().pbox`) so the trace replicates exactly the images the
            // rasterizer draws. `None` (no box) → the single central copy.
            let box_vecs = mol.data.state().pbox.as_ref().map(|pb| {
                let m = pb.get_matrix();
                // Columns of the box matrix are the lattice vectors a, b, c (nm).
                [
                    Vec3::new(m[(0, 0)], m[(1, 0)], m[(2, 0)]),
                    Vec3::new(m[(0, 1)], m[(1, 1)], m[(2, 1)]),
                    Vec3::new(m[(0, 2)], m[(1, 2)], m[(2, 2)]),
                ]
            });
            for (ri, rep) in mol.reps.iter().enumerate() {
                if !rep.visible {
                    continue;
                }
                // Every image this rep draws (central included iff `self_img`); a single
                // `[0,0,0]` when the molecule has no box. The tracer has no per-image camera,
                // so the replication the rasterizer gets from `images[mi][ri]` is baked in
                // here — each primitive below is emitted once per offset, shifted by it.
                let offsets = match box_vecs {
                    Some([a, b, c]) => rep.periodic.offsets(a, b, c),
                    None => vec![Vec3::ZERO],
                };
                // Interaction dashes are built from *two* molecules, so they don't come out of
                // `geometry::build` (which returns nothing for this style) but from the same
                // second-pass builder `rebuild_dirty` uses. Without this the ray trace lost every
                // contact line — the one thing a docking picture is *about*.
                if matches!(rep.params, crate::geometry::RepParams::Interactions { .. }) {
                    let geom = crate::app::build::build_interactions(scene, mi, ri);
                    for &off in &offsets {
                        for seg in geom.lines.chunks_exact(2) {
                            let (v0, v1) =
                                (shift_line_vertex(seg[0], off), shift_line_vertex(seg[1], off));
                            if let Some(gc) = line_capsule(&v0, &v1, view) {
                                aabbs.push(cylinder_aabb(&gc));
                                tags.push(tag(TAG_CYLINDER, self.cylinders.len()));
                                self.cylinders.push(gc);
                            }
                        }
                    }
                    continue;
                }
                let Some(sel) = rep.sel.as_ref() else { continue };
                // Camera changes need new screen-space strand/line conversion and
                // a BVH, but not another surface, cartoon, or atom geometry build.
                let fresh;
                let geom = if let Some(cached) = rep.cached_geometry(dashed_pbc) {
                    cached
                } else {
                    let smoothed = (rep.smooth_window > 1)
                        .then(|| mol.trajectory.smoothed_state(rep.smooth_window))
                        .flatten();
                    let state = smoothed.as_ref().unwrap_or(render_state);
                    let bound = mol.data.bind_with_state(sel, state);
                    let ss = geometry::needs_ss(&rep.params, rep.color)
                        .then(|| SsMap::compute(&bound, rep.ss_algo));
                    fresh = geometry::build(
                        &bound, mol.n_atoms, &mol.bonds, &rep.params, rep.color_spec(),
                        rep.material, ss.as_ref(), dashed_pbc,
                    );
                    fresh.as_ref()
                };

                for &off in &offsets {
                    let envelope_group = if super::envelope::needed_primitives(geom.spheres, geom.cylinders) {
                        let group = next_envelope_group;
                        next_envelope_group += 1;
                        group
                    } else { 0 };
                    for sp in geom.spheres {
                        let gs = GpuSphere {
                            c: [
                                sp.center[0] + off.x,
                                sp.center[1] + off.y,
                                sp.center[2] + off.z,
                                sp.radius,
                            ],
                            m: [sp.color, sp.mat, envelope_group, 0],
                        };
                        aabbs.push(sphere_aabb(&gs));
                        tags.push(tag(TAG_SPHERE, self.spheres.len()));
                        self.spheres.push(gs);
                    }
                    for cy in geom.cylinders {
                        // Multi-order bonds' parallel strands are shifted by the *rasterizer's
                        // vertex shader*, from the camera, so they stay side-by-side at any angle
                        // (see `cylinder.wgsl`). The tracer has no vertex stage, so the same shift
                        // is baked in here — otherwise a double/triple/aromatic bond traced as a
                        // single strand, its siblings hidden inside it. The strand shift is
                        // translation-invariant, so the periodic `off` just adds on top of it.
                        let [p0, p1] = strand_offset(cy, view.view);
                        let smooth = cy.profile[1] > 0.0;
                        let shift = if smooth { p0 - Vec3::from(cy.p0) } else { Vec3::ZERO };
                        let (p0, p1) = if smooth {
                            (Vec3::from(cy.p0) + off, Vec3::from(cy.p1) + off)
                        } else { (p0 + off, p1 + off) };
                        let gc = GpuCylinder {
                            profile: cy.profile,
                            lane: [shift.x, shift.y, shift.z, cy.smoothing],
                            c0: [p0[0], p0[1], p0[2], cy.radius],
                            c1: [p1[0], p1[1], p1[2], cy.color_blend],
                            m: [cy.color, cy.mat, cy.color1, envelope_group << 1],
                        };
                        aabbs.push(cylinder_aabb(&gc));
                        tags.push(tag(TAG_CYLINDER, self.cylinders.len()));
                        self.cylinders.push(gc);
                    }
                    // Lines (the Lines rep, interaction dashes, the periodic box) are screen-space
                    // quads in the rasterizer — a constant *pixel* width, which a ray has no notion
                    // of. Traced as **thin capsules** whose world radius is that pixel width
                    // converted at the traced camera, so the trace shows what the raster shows: a
                    // lines-only receptor used to vanish completely from a ray-traced docking view,
                    // which is the whole backdrop of the picture. (Each image's capsule is built
                    // from the shifted endpoints, so a perspective view's per-image line width is
                    // correct too.)
                    for seg in geom.lines.chunks_exact(2) {
                        let (v0, v1) =
                            (shift_line_vertex(seg[0], off), shift_line_vertex(seg[1], off));
                        if let Some(gc) = line_capsule(&v0, &v1, view) {
                            aabbs.push(cylinder_aabb(&gc));
                            tags.push(tag(TAG_CYLINDER, self.cylinders.len()));
                            self.cylinders.push(gc);
                        }
                    }
                    // Mesh: append vertices (offset triangle indices into the shared array).
                    let base = self.mesh_verts.len() as u32;
                    for v in &geom.mesh.vertices {
                        self.mesh_verts.push(GpuMeshVertex {
                            p: [
                                v.pos[0] + off.x,
                                v.pos[1] + off.y,
                                v.pos[2] + off.z,
                                f32::from_bits(v.color),
                            ],
                            n: [v.normal[0], v.normal[1], v.normal[2], f32::from_bits(v.mat)],
                        });
                    }
                    for t in geom.mesh.indices.chunks_exact(3) {
                        let (i0, i1, i2) = (base + t[0], base + t[1], base + t[2]);
                        aabbs.push(triangle_aabb(&self.mesh_verts, i0, i1, i2));
                        tags.push(tag(TAG_TRIANGLE, self.triangles.len()));
                        let closed = mesh_closed_flag(rep.kind);
                        self.triangles.push(GpuTriangle {
                            i: [i0, i1, i2, closed],
                        });
                    }
                }
            }
            // Periodic-box wireframe: the molecule-level box (central) iff `mol.show_box`,
            // plus a replica at each image cell of any visible rep with `periodic.show_box`
            // on — exactly the cells `render_scene`'s `draw_reps` draws it at. The tracer
            // renders no box at all otherwise, so a periodic view lost every cell outline.
            if let Some(pbox) = mol.data.state().pbox.as_ref() {
                let mut box_offsets: Vec<Vec3> = Vec::new();
                if mol.show_box {
                    box_offsets.push(Vec3::ZERO);
                }
                if let Some([a, b, c]) = box_vecs {
                    for rep in &mol.reps {
                        if rep.visible && rep.periodic.show_box {
                            box_offsets.extend(rep.periodic.offsets(a, b, c));
                        }
                    }
                }
                // Dedup coincident cells (e.g. `mol.show_box` + a rep's central image), so
                // the same box outline isn't traced twice.
                box_offsets.sort_by(|u, v| {
                    u.to_array().partial_cmp(&v.to_array()).unwrap_or(std::cmp::Ordering::Equal)
                });
                box_offsets.dedup();
                if !box_offsets.is_empty() {
                    let edges = geometry::box_wireframe(pbox);
                    for off in box_offsets {
                        for seg in edges.chunks_exact(2) {
                            let (v0, v1) =
                                (shift_line_vertex(seg[0], off), shift_line_vertex(seg[1], off));
                            if let Some(gc) = line_capsule(&v0, &v1, view) {
                                aabbs.push(cylinder_aabb(&gc));
                                tags.push(tag(TAG_CYLINDER, self.cylinders.len()));
                                self.cylinders.push(gc);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// An axis-aligned bounding box.
#[derive(Clone, Copy)]
struct Aabb {
    min: Vec3,
    max: Vec3,
}

impl Aabb {
    fn empty() -> Self {
        Self { min: Vec3::splat(f32::INFINITY), max: Vec3::splat(f32::NEG_INFINITY) }
    }
    fn union(self, o: Aabb) -> Aabb {
        Aabb { min: self.min.min(o.min), max: self.max.max(o.max) }
    }
    fn extend(&mut self, o: Aabb) {
        self.min = self.min.min(o.min);
        self.max = self.max.max(o.max);
    }
    fn point(p: Vec3) -> Aabb {
        Aabb { min: p, max: p }
    }
    fn centroid(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
    /// Surface area (the SAH metric). Zero/negative extents clamp to 0.
    fn area(&self) -> f32 {
        let d = (self.max - self.min).max(Vec3::ZERO);
        2.0 * (d.x * d.y + d.y * d.z + d.z * d.x)
    }
}

fn sphere_aabb(s: &GpuSphere) -> Aabb {
    let center = Vec3::new(s.c[0], s.c[1], s.c[2]);
    let r = Vec3::splat(s.c[3].max(0.0));
    Aabb { min: center - r, max: center + r }
}

/// Cylinder flag: no end caps — the primitive stands in for a flat-ended line quad. Rounded ends
/// would lengthen each dash of a dashed line by its radius at both ends, which at line widths of a
/// few pixels is enough to close the gaps and turn a dashed contact line into a solid one.
const FLAG_FLAT_ENDS: u32 = 1;

/// The camera the scene is being gathered *for*. Two pieces of geometry are view-dependent —
/// multi-order bond strand offsets and the pixel widths of lines — so unlike the rest of the
/// primitives they can't be baked once; see `App::rt_scene_dirty`.
#[derive(Clone, Copy, PartialEq)]
pub struct RtView {
    pub view: glam::Mat4,
    pub proj: glam::Mat4,
    /// Logical viewport height in pixels — what a "width in px" is measured against. The
    /// **logical** one, not the output image's, so a 2× save keeps line weights proportional to
    /// the view it was taken from, exactly as the rasterized capture does.
    pub viewport_h: f32,
}

impl RtView {
    /// World units per screen pixel at `p`. In view space `ndc_y = proj[1][1]·y / w`, and a pixel
    /// is `2/height` of NDC — so one pixel spans `2·w / (proj[1][1]·height)` world units. Exact
    /// for both projections (an orthographic `w` is 1, giving the constant scale it should).
    fn world_per_px(&self, p: glam::Vec3) -> f32 {
        let clip = self.proj * self.view * p.extend(1.0);
        2.0 * clip.w.abs().max(1e-6) / (self.proj.y_axis.y.abs().max(1e-6) * self.viewport_h.max(1.0))
    }
}

/// One line segment as a thin capsule: the pixel width becomes a world radius at the traced
/// camera, and the multi-order strand offset (also in pixels) becomes a world shift along the
/// segment's screen perpendicular. Returns `None` for a degenerate segment.
///
/// The material is **flat** (ambient 1, no diffuse or specular) so a line traces as the unlit
/// constant color the rasterizer draws, rather than as a shaded tube — a line is a schematic,
/// not a physical stick. AO/shadow still multiply it, as they do every other surface.
fn line_capsule(
    v0: &crate::render::line::LineVertex,
    v1: &crate::render::line::LineVertex,
    view: RtView,
) -> Option<GpuCylinder> {
    const FLAT_MAT: u32 = 0xff; // ambient = 255, diffuse/specular/shininess = 0
    let p0 = glam::Vec3::from(v0.pos);
    let p1 = glam::Vec3::from(v1.pos);
    if p0.distance_squared(p1) < 1e-12 {
        return None;
    }
    // One scale for the whole segment (a capsule has a single radius): its midpoint's.
    let scale = view.world_per_px((p0 + p1) * 0.5);
    let radius = (0.5 * v0.width.max(0.5) * scale).max(1e-5);
    let (mut a, mut b) = (p0, p1);
    if v0.offset_px != 0.0 {
        let a0 = view.view.transform_point3(p0);
        let a1 = view.view.transform_point3(p1);
        let ax = a1 - a0;
        let dir = if ax.length() > 1e-8 { ax.normalize() } else { glam::Vec3::X };
        let sp = dir.cross(glam::Vec3::Z);
        let sp = if sp.length() > 1e-4 { sp.normalize() } else { glam::Vec3::X };
        let shift = view.view.inverse().transform_vector3(sp) * (v0.offset_px * scale);
        a += shift;
        b += shift;
    }
    Some(GpuCylinder {
        profile: [0.0; 4],
        lane: [0.0; 4],
        c0: [a[0], a[1], a[2], radius],
        c1: [b[0], b[1], b[2], 0.0],
        m: [v0.color, FLAT_MAT, v1.color, FLAG_FLAT_ENDS],
    })
}

/// A copy of `v` translated by `off` (nm) — used to replicate a line at each periodic image
/// before it's converted to a capsule, so `line_capsule`'s view-dependent width/offset are
/// computed at the image's real position.
fn shift_line_vertex(v: crate::render::line::LineVertex, off: Vec3) -> crate::render::line::LineVertex {
    crate::render::line::LineVertex {
        pos: [v.pos[0] + off.x, v.pos[1] + off.y, v.pos[2] + off.z],
        ..v
    }
}

/// A multi-order bond strand's endpoints, with the same screen-plane shift the rasterizer's
/// vertex shader applies: the screen plane in view space is XY, so the bond's screen
/// perpendicular is `cross(axis_view, +Z)` — perpendicular to the bond *and* in the screen plane,
/// which is what keeps the strands side-by-side instead of collapsing edge-on. Computed in view
/// space and rotated back to world, since the tracer's primitives are world-space.
/// `offset = [0, 0]` (single/unspecified bonds) is a no-op.
fn strand_offset(cy: &crate::render::cylinder::CylinderInstance, view: glam::Mat4) -> [glam::Vec3; 2] {
    let p0 = glam::Vec3::from(cy.p0);
    let p1 = glam::Vec3::from(cy.p1);
    if cy.offset[1] == 0.0 {
        return [p0, p1];
    }
    let a0 = view.transform_point3(p0);
    let a1 = view.transform_point3(p1);
    let ax = a1 - a0;
    let dir = if ax.length() > 1e-8 { ax.normalize() } else { glam::Vec3::X };
    let sp = dir.cross(glam::Vec3::Z);
    let sp = if sp.length() > 1e-4 { sp.normalize() } else { glam::Vec3::X };
    // View→world is a pure rotation+translation, so the shift only needs the rotation part.
    let shift = view.inverse().transform_vector3(sp) * (cy.offset[0] * cy.offset[1]);
    [p0 + shift, p1 + shift]
}

/// Cylinder AABB = union of the two end-spheres (`p0±r`, `p1±r`) — correct and never
/// degenerate (a segment-only box would be a zero-thickness slab for an axis-aligned bond).
fn cylinder_aabb(c: &GpuCylinder) -> Aabb {
    let p0 = Vec3::new(c.c0[0], c.c0[1], c.c0[2]);
    let p1 = Vec3::new(c.c1[0], c.c1[1], c.c1[2]);
    let r0 = c.profile[0].hypot(c.profile[1]);
    let r1 = (p0.distance(p1) - c.profile[2]).hypot(c.profile[3]);
    let flare = if c.profile[1] > 0.0 { r0.max(r1) + c.lane[3] * r0.min(r1) / 3.0 } else { 0.0 };
    let r = Vec3::splat(c.c0[3].max(flare).max(0.0) + Vec3::new(c.lane[0], c.lane[1], c.lane[2]).length());
    Aabb { min: p0 - r, max: p0 + r }.union(Aabb { min: p1 - r, max: p1 + r })
}

/// Triangle AABB (bounds of its 3 vertices), padded by a small epsilon so an
/// axis-aligned/coplanar triangle isn't a zero-thickness slab.
fn triangle_aabb(verts: &[GpuMeshVertex], i0: u32, i1: u32, i2: u32) -> Aabb {
    let p = |i: u32| {
        let v = verts[i as usize].p;
        Vec3::new(v[0], v[1], v[2])
    };
    let mut a = Aabb::point(p(i0));
    a.extend(Aabb::point(p(i1)));
    a.extend(Aabb::point(p(i2)));
    let eps = Vec3::splat(1e-5);
    Aabb { min: a.min - eps, max: a.max + eps }
}

/// Build a binned-SAH BVH over the primitive AABBs. Returns the flat node array
/// (root = node 0) and the primitive order — a `0..N` permutation; each leaf owns a
/// contiguous slice of it. Children are allocated contiguously so an interior node's
/// right child is `left + 1`.
fn build_bvh(aabbs: &[Aabb]) -> (Vec<BvhNode>, Vec<u32>) {
    if aabbs.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let centroids: Vec<Vec3> = aabbs.iter().map(|a| a.centroid()).collect();
    let mut order: Vec<u32> = (0..aabbs.len() as u32).collect();

    let mut nodes: Vec<BvhNode> = vec![BvhNode::zeroed()]; // root placeholder
    let mut stack: Vec<(usize, usize, usize, usize)> = vec![(0, 0, order.len(), 0)];

    while let Some((node, start, end, depth)) = stack.pop() {
        let mut bounds = Aabb::empty();
        for &i in &order[start..end] {
            bounds.extend(aabbs[i as usize]);
        }
        let count = end - start;
        let make_leaf = |nodes: &mut Vec<BvhNode>| {
            nodes[node] = BvhNode::new(bounds.min, bounds.max, start as u32, count as u32);
        };
        if count <= LEAF_SIZE || depth >= MAX_BVH_DEPTH {
            make_leaf(&mut nodes);
            continue;
        }

        // Split on the longest centroid axis, binned by SAH.
        let mut cbounds = Aabb::empty();
        for &i in &order[start..end] {
            cbounds.extend(Aabb::point(centroids[i as usize]));
        }
        let extent = cbounds.max - cbounds.min;
        let axis = if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        };
        if extent[axis] <= 1e-12 {
            make_leaf(&mut nodes);
            continue;
        }

        let mut bin_box = [Aabb::empty(); BINS];
        let mut bin_cnt = [0usize; BINS];
        let scale = BINS as f32 / extent[axis];
        let bin_of = |c: Vec3| -> usize {
            (((c[axis] - cbounds.min[axis]) * scale) as usize).min(BINS - 1)
        };
        for &i in &order[start..end] {
            let b = bin_of(centroids[i as usize]);
            bin_box[b].extend(aabbs[i as usize]);
            bin_cnt[b] += 1;
        }

        // Prefix/suffix bounds make all candidate split evaluations O(BINS).
        let mut prefix_box = [Aabb::empty(); BINS];
        let mut suffix_box = [Aabb::empty(); BINS];
        let mut prefix_count = [0usize; BINS];
        let mut suffix_count = [0usize; BINS];
        for bin in 0..BINS {
            prefix_box[bin] = if bin == 0 { bin_box[bin] } else { prefix_box[bin - 1].union(bin_box[bin]) };
            prefix_count[bin] = bin_cnt[bin] + if bin == 0 { 0 } else { prefix_count[bin - 1] };
        }
        for bin in (0..BINS).rev() {
            suffix_box[bin] = if bin + 1 == BINS { bin_box[bin] } else { suffix_box[bin + 1].union(bin_box[bin]) };
            suffix_count[bin] = bin_cnt[bin] + if bin + 1 == BINS { 0 } else { suffix_count[bin + 1] };
        }
        let mut best_cost = f32::INFINITY;
        let mut best_split = 0usize;
        for split in 1..BINS {
            let (lb, rb) = (prefix_box[split - 1], suffix_box[split]);
            let (lc, rc) = (prefix_count[split - 1], suffix_count[split]);
            if lc == 0 || rc == 0 {
                continue;
            }
            let cost = lb.area() * lc as f32 + rb.area() * rc as f32;
            if cost < best_cost {
                best_cost = cost;
                best_split = split;
            }
        }
        if best_split == 0 {
            make_leaf(&mut nodes);
            continue;
        }

        let mut mid = start;
        for i in start..end {
            if bin_of(centroids[order[i] as usize]) < best_split {
                order.swap(i, mid);
                mid += 1;
            }
        }
        if mid == start || mid == end {
            make_leaf(&mut nodes);
            continue;
        }

        let left = nodes.len();
        nodes.push(BvhNode::zeroed());
        nodes.push(BvhNode::zeroed());
        nodes[node] = BvhNode::new(bounds.min, bounds.max, left as u32, 0);
        stack.push((left, start, mid, depth + 1));
        stack.push((left + 1, mid, end, depth + 1));
    }

    (nodes, order)
}

#[cfg(test)]
fn build_bvh_reference(aabbs: &[Aabb]) -> (Vec<BvhNode>, Vec<u32>) {
    if aabbs.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let centroids: Vec<Vec3> = aabbs.iter().map(|a| a.centroid()).collect();
    let mut order: Vec<u32> = (0..aabbs.len() as u32).collect();

    let mut nodes: Vec<BvhNode> = vec![BvhNode::zeroed()]; // root placeholder
    let mut stack: Vec<(usize, usize, usize)> = vec![(0, 0, order.len())];

    while let Some((node, start, end)) = stack.pop() {
        let mut bounds = Aabb::empty();
        for &i in &order[start..end] {
            bounds.extend(aabbs[i as usize]);
        }
        let count = end - start;
        let make_leaf = |nodes: &mut Vec<BvhNode>| {
            nodes[node] = BvhNode::new(bounds.min, bounds.max, start as u32, count as u32);
        };
        if count <= LEAF_SIZE {
            make_leaf(&mut nodes);
            continue;
        }

        // Split on the longest centroid axis, binned by SAH.
        let mut cbounds = Aabb::empty();
        for &i in &order[start..end] {
            cbounds.extend(Aabb::point(centroids[i as usize]));
        }
        let extent = cbounds.max - cbounds.min;
        let axis = if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        };
        if extent[axis] <= 1e-12 {
            make_leaf(&mut nodes);
            continue;
        }

        let mut bin_box = [Aabb::empty(); BINS];
        let mut bin_cnt = [0usize; BINS];
        let scale = BINS as f32 / extent[axis];
        let bin_of = |c: Vec3| -> usize {
            (((c[axis] - cbounds.min[axis]) * scale) as usize).min(BINS - 1)
        };
        for &i in &order[start..end] {
            let b = bin_of(centroids[i as usize]);
            bin_box[b].extend(aabbs[i as usize]);
            bin_cnt[b] += 1;
        }

        let mut best_cost = f32::INFINITY;
        let mut best_split = 0usize;
        for split in 1..BINS {
            let (mut lb, mut rb) = (Aabb::empty(), Aabb::empty());
            let (mut lc, mut rc) = (0usize, 0usize);
            for b in 0..split {
                lb = lb.union(bin_box[b]);
                lc += bin_cnt[b];
            }
            for b in split..BINS {
                rb = rb.union(bin_box[b]);
                rc += bin_cnt[b];
            }
            if lc == 0 || rc == 0 {
                continue;
            }
            let cost = lb.area() * lc as f32 + rb.area() * rc as f32;
            if cost < best_cost {
                best_cost = cost;
                best_split = split;
            }
        }
        if best_split == 0 {
            make_leaf(&mut nodes);
            continue;
        }

        let mut mid = start;
        for i in start..end {
            if bin_of(centroids[order[i] as usize]) < best_split {
                order.swap(i, mid);
                mid += 1;
            }
        }
        if mid == start || mid == end {
            make_leaf(&mut nodes);
            continue;
        }

        let left = nodes.len();
        nodes.push(BvhNode::zeroed());
        nodes.push(BvhNode::zeroed());
        nodes[node] = BvhNode::new(bounds.min, bounds.max, left as u32, 0);
        stack.push((left, start, mid));
        stack.push((left + 1, mid, end));
    }

    (nodes, order)
}

// ===========================================================================
// GPU half: storage buffers + the compute tracer + the resolve pass.
// ===========================================================================

/// Per-render uniform for the tracer. Mirrors `RtUniform` in `raytrace.wgsl`
/// Matrices and vec4 fields are 16-byte aligned.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct RtUniform {
    pub inv_view_proj: [[f32; 4]; 4],
    pub view: [[f32; 4]; 4],
    pub proj: [[f32; 4]; 4],
    /// World space -> light clip space; shared with the raster shadow camera.
    pub shadow_matrix: [[f32; 4]; 4],
    /// World-space displacements of one shadow-map texel along its axes.
    pub shadow_u: [f32; 4],
    pub shadow_v: [f32; 4],
    pub bg_top: [f32; 4],
    pub bg_bottom: [f32; 4],
    pub depth_range: [f32; 4],
    /// xyz = eye world pos; w = perspective flag (1 persp / 0 ortho).
    pub eye: [f32; 4],
    /// xyz = world-space direction toward the key light (shadow ray).
    pub light_dir: [f32; 4],
    /// radius (nm), bias, strength, enabled.
    pub ao: [f32; 4],
    /// strength, world-space bias (nm), enabled, softness.
    pub shadow: [f32; 4],
    /// Background clear color, w = GI strength.
    pub bg: [f32; 4],
    /// Depth cue (fog), exactly as the rasterizer's camera uniform carries it:
    /// `near, far` (eye-space distances), `strength`, `mode` (0 linear / 1 exp / 2 exp²).
    pub cue: [f32; 4],
    /// Color the fog fades toward (the background, or a gradient's midpoint), w unused.
    pub fog_color: [f32; 4],
    /// width, height, samples-this-step, frame_seed. Set by `render`.
    pub dims: [u32; 4],
    /// Progressive accumulation: prior_total_samples, reset(0/1), _, _. Set by `render`.
    pub accum: [u32; 4],
}

/// GPU ray-tracing resources (compute tracer + fullscreen resolve). Created only on a
/// device with compute + `Rgba32Float` storage (WebGPU/native); `None` on WebGL2.
/// Only the tree is retained: large converted primitive/mesh arrays are not duplicated.
struct BvhCache {
    nodes: Vec<BvhNode>,
    order: Vec<u32>,
    counts: [usize; 3],
    build_cost: f32,
    refits: u32,
}
fn bvh_cost(nodes: &[BvhNode]) -> f32 {
    let area = |node: &BvhNode| Aabb { min: node.min(), max: node.max() }.area();
    let root = nodes.first().map_or(0.0, area);
    if root <= 0.0 { return f32::INFINITY; }
    nodes.iter().map(|n| area(n) * n.count().max(1) as f32).sum::<f32>() / root
}

#[derive(PartialEq)]
struct PreparedKey {
    molecules: Vec<PreparedMolecule>,
    view: Option<RtView>,
}
#[derive(PartialEq)]
struct PreparedMolecule {
    id: crate::scene::MolId,
    show_box: bool,
    box_matrix: Option<[u32; 9]>,
    reps: Vec<(usize, u64, crate::scene::PeriodicParams)>,
}
impl PreparedKey {
    fn new(scene: &Scene, view: RtView, dashed: bool) -> Option<Self> {
        let mut molecules = Vec::new();
        let mut view_dependent = false;
        for mol in scene.molecules.iter().filter(|m| m.visible) {
            let box_matrix = mol.data.state().pbox.as_ref().map(|b| {
                let m = b.get_matrix();
                std::array::from_fn(|i| m.as_slice()[i].to_bits())
            });
            let mut reps = Vec::new();
            for (index, rep) in mol.reps.iter().enumerate().filter(|(_, r)| r.visible) {
                // Interactions depend on another molecule; retain their full-gather path.
                if matches!(rep.kind, crate::geometry::RepKind::Interactions) {
                    return None;
                }
                rep.cached_geometry(dashed)?;
                view_dependent |= rep.geometry_view_dependent
                    || (box_matrix.is_some() && rep.periodic.show_box);
                reps.push((index, rep.geometry_revision, rep.periodic));
            }
            view_dependent |= box_matrix.is_some() && mol.show_box;
            molecules.push(PreparedMolecule { id: mol.id, show_box: mol.show_box, box_matrix, reps });
        }
        Some(Self { molecules, view: view_dependent.then_some(view) })
    }
}

pub struct Raytracer {
    trace_pipeline: wgpu::ComputePipeline,
    trace_bgl: wgpu::BindGroupLayout,
    resolve_pipeline: wgpu::RenderPipeline,
    resolve_bgl: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
    // Scene storage buffers (recreated on `upload`); `None` until a non-empty scene. Each
    // is at least 16 bytes so an empty primitive class still binds (WGSL needs a buffer).
    spheres: Option<wgpu::Buffer>,
    cylinders: Option<wgpu::Buffer>,
    mesh_verts: Option<wgpu::Buffer>,
    triangles: Option<wgpu::Buffer>,
    nodes: Option<wgpu::Buffer>,
    prim_indices: Option<wgpu::Buffer>,
    bvh_cache: Option<BvhCache>,
    prepared_key: Option<PreparedKey>,
    has_scene: bool,
    has_transparent: bool,
    // Linear HDR accumulators (ping-pong: read one, write the other, swap). Each holds the
    // running *average* radiance. Recreated on size change.
    accum: Option<[(wgpu::Texture, wgpu::TextureView); 2]>,
    accum_size: [u32; 2],
    trace_bindings: Option<[wgpu::BindGroup; 2]>,
    resolve_bindings: Option<[wgpu::BindGroup; 2]>,
    /// Which accumulator is the current (latest) one to read from / resolve.
    read_idx: usize,
    /// Samples accumulated so far (the running-average weight). Reset on camera change.
    total_samples: u32,
    /// In-progress tiled trace (a resumable cursor); `None` when idle. Driven a bounded
    /// number of tile-submits per frame so the trace spreads across frames (responsive UI +
    /// progressive refinement) instead of one blocking dispatch.
    cursor: Option<TraceCursor>,
}

/// Resumable state for a tiled trace: the target sample count + the tile sweep position.
#[derive(Clone, Copy)]
struct TraceCursor {
    size: [u32; 2],
    uniform: RtUniform,
    total: u32,
    chunk_cap: u32,
    tile_size: u32,
    ox: u32,
    oy: u32,
}

/// Block (tile) dimension for the tiled trace; one tile × one sample-chunk is one GPU submit.
const TRACE_TILE: u32 = 256;
/// Diffuse path-tracing bounces when GI is on (must match `GI_BOUNCES` in `raytrace.wgsl`).
const GI_BOUNCES: u32 = 3;

impl Raytracer {
    /// Build the ray-tracing pipelines, or `None` if the device can't support them
    /// (no compute, or no `Rgba32Float` storage). `color_format` is the scene color
    /// target the resolve writes into.
    pub fn new(rs: &RenderState, color_format: wgpu::TextureFormat) -> Option<Self> {
        let device = &rs.device;
        if !rs
            .adapter
            .get_texture_format_features(wgpu::TextureFormat::Rgba32Float)
            .allowed_usages
            .contains(wgpu::TextureUsages::STORAGE_BINDING)
        {
            log::warn!("ray tracer unavailable: device lacks Rgba32Float storage textures");
            return None;
        }

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("raytrace"),
            source: wgpu::ShaderSource::Wgsl(super::lit_shader_source(include_str!("shaders/raytrace.wgsl")).into()),
        });

        let storage = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let store_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: storage,
            count: None,
        };
        let trace_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rt-trace-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                store_entry(1), // spheres
                store_entry(2), // cylinders
                store_entry(3), // mesh vertices
                store_entry(4), // triangles
                store_entry(5), // bvh nodes
                store_entry(6), // prim indices
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba32Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                // Previous accumulator (the running average to extend) — read as a sampled
                // texture for the ping-pong; ignored when the reset flag is set.
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let resolve_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rt-resolve-bgl"),
            entries: &[
                // The uniform (binding 0) so `fs_resolve` can read the GI flag (U.bg.w) and
                // pick its tonemap (clamp for tier-1, ACES for GI).
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let trace_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rt-trace-layout"),
            bind_group_layouts: &[Some(&trace_bgl)],
            immediate_size: 0,
        });
        let trace_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("rt-trace"),
            layout: Some(&trace_layout),
            module: &module,
            entry_point: Some("cs_trace"),
            compilation_options: Default::default(),
            cache: None,
        });

        let resolve_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rt-resolve-layout"),
            bind_group_layouts: &[Some(&resolve_bgl)],
            immediate_size: 0,
        });
        let resolve_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rt-resolve"),
            layout: Some(&resolve_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_resolve"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_resolve"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rt-uniform"),
            size: std::mem::size_of::<RtUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Some(Self {
            trace_pipeline,
            trace_bgl,
            resolve_pipeline,
            resolve_bgl,
            uniform_buf,
            spheres: None,
            cylinders: None,
            mesh_verts: None,
            triangles: None,
            nodes: None,
            prim_indices: None,
            bvh_cache: None,
            prepared_key: None,
            has_scene: false,
            has_transparent: false,
            accum: None,
            accum_size: [0, 0],
            trace_bindings: None,
            resolve_bindings: None,
            read_idx: 0,
            total_samples: 0,
            cursor: None,
        })
    }

    /// (Re)upload the scene's primitive + BVH buffers. Call when geometry changes.
    pub fn prepare(&mut self, rs: &RenderState, scene: &Scene, view: RtView, dashed: bool) {
        let key = PreparedKey::new(scene, view, dashed);
        if key.is_some() && self.prepared_key == key {
            let _timing = crate::performance::span("rt-scene-cache-hit");
            return;
        }
        let mut cache = self.bvh_cache.take();
        let mut data = RtScene::gather_with_bvh_cache(scene, view, dashed, &mut cache);
        self.upload(rs, &data);
        if let Some(cache) = cache.as_mut() {
            cache.nodes = std::mem::take(&mut data.nodes);
            cache.order = std::mem::take(&mut data.prim_indices);
        }
        self.bvh_cache = cache;
        self.prepared_key = key;
    }

    pub fn upload(&mut self, rs: &RenderState, scene: &RtScene) {
        let _timing = crate::performance::span("rt-upload");
        self.prepared_key = None;
        self.bvh_cache = None;
        self.has_scene = !scene.is_empty();
        self.has_transparent = scene.spheres.iter().any(|s| s.m[0] >> 24 < 255)
            || scene.cylinders.iter().any(|c| c.m[0] >> 24 < 255 || c.m[2] >> 24 < 255)
            || scene.mesh_verts.iter().any(|v| v.p[3].to_bits() >> 24 < 255);
        if !self.has_scene {
            self.trace_bindings = None;
            self.spheres = None; self.cylinders = None; self.mesh_verts = None;
            self.triangles = None; self.nodes = None; self.prim_indices = None;
            return;
        }
        // Only buffer replacement changes bind groups. Active primitive ranges
        // come from leaf tags, so stale data beyond a shrinking scene is unreachable.
        let mut replaced = false;
        let mut write = |slot: &mut Option<wgpu::Buffer>, bytes: &[u8], stride: usize, label| {
            let zeros = [0u8; 80];
            let bytes = if bytes.is_empty() { &zeros[..stride] } else { bytes };
            if let Some(buffer) = slot.as_ref().filter(|b| b.size() >= bytes.len() as u64) {
                rs.queue.write_buffer(buffer, 0, bytes);
            } else {
                *slot = Some(rs.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label), contents: bytes,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                }));
                replaced = true;
            }
        };
        write(&mut self.spheres, bytemuck::cast_slice(&scene.spheres), std::mem::size_of::<GpuSphere>(), "rt-spheres");
        write(&mut self.cylinders, bytemuck::cast_slice(&scene.cylinders), std::mem::size_of::<GpuCylinder>(), "rt-cylinders");
        write(&mut self.mesh_verts, bytemuck::cast_slice(&scene.mesh_verts), std::mem::size_of::<GpuMeshVertex>(), "rt-mesh-verts");
        write(&mut self.triangles, bytemuck::cast_slice(&scene.triangles), std::mem::size_of::<GpuTriangle>(), "rt-triangles");
        write(&mut self.nodes, bytemuck::cast_slice(&scene.nodes), std::mem::size_of::<BvhNode>(), "rt-bvh");
        write(&mut self.prim_indices, bytemuck::cast_slice(&scene.prim_indices), std::mem::size_of::<u32>(), "rt-prim-indices");
        if replaced { self.trace_bindings = None; }
    }

    /// Whether a scene has been uploaded (non-empty).
    pub fn has_scene(&self) -> bool {
        self.has_scene
    }

    fn ensure_accum(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.accum.is_none() || self.accum_size != size {
            let mk = || {
                let tex = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("rt-accum"),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba32Float,
                    usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
                (tex, view)
            };
            self.trace_bindings = None;
            self.resolve_bindings = None;
            self.accum = Some([mk(), mk()]);
            self.accum_size = size;
            self.read_idx = 0;
            self.total_samples = 0;
        }
    }


    // Bindings depend on buffer/texture identities, not tile uniforms. Cache both
    // ping-pong directions and invalidate them when those resources are replaced.
    fn ensure_bindings(&mut self, rs: &RenderState) {
        let accums = self.accum.as_ref().unwrap();
        if self.trace_bindings.is_none() {
            self.trace_bindings = Some(std::array::from_fn(|read_idx| {
                let write_idx = 1 - read_idx;
                rs.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("rt-trace-bg"),
                    layout: &self.trace_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: self.uniform_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: self.spheres.as_ref().unwrap().as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: self.cylinders.as_ref().unwrap().as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: self.mesh_verts.as_ref().unwrap().as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: self.triangles.as_ref().unwrap().as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: self.nodes.as_ref().unwrap().as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: self.prim_indices.as_ref().unwrap().as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 7,
                            resource: wgpu::BindingResource::TextureView(&accums[write_idx].1),
                        },
                        wgpu::BindGroupEntry {
                            binding: 9,
                            resource: wgpu::BindingResource::TextureView(&accums[read_idx].1),
                        },
                    ],
                })
            }));
        }
        if self.resolve_bindings.is_none() {
            self.resolve_bindings = Some(std::array::from_fn(|read_idx| {
                rs.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("rt-resolve-bg"),
                    layout: &self.resolve_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: self.uniform_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 8,
                            resource: wgpu::BindingResource::TextureView(&accums[read_idx].1),
                        },
                    ],
                })
            }));
        }
    }

    /// Tiled, multi-submit converged render: trace `total_samples` paths/pixel by sweeping the
    /// image in `TILE`×`TILE` blocks over many short GPU submits (a bounded sample-chunk per
    /// block, polling between submits), then resolve once into `target`. Keeping each submit
    /// well under the driver's command-timeout is what stops a huge scene from hanging a
    /// single whole-image dispatch and **losing the device** (the reported crash). The sole
    /// trace entry point — drives both the "Save image" file render and the R-key viewport
    /// still (`SceneRenderer::render_raytrace_still`).
    /// Resolve the current running average (`read_idx`) into `target`, tonemapped per the GI
    /// flag (`fs_resolve`). The accumulator at `read_idx` always holds a *complete* image (the
    /// last finished sample-chunk), so this is seam-free even mid-chunk.
    fn resolve_into(&self, rs: &RenderState, target: &wgpu::TextureView) {
        let resolve_bg = &self.resolve_bindings.as_ref().unwrap()[self.read_idx];
        let mut gpu_timing = crate::performance::GpuFrame::begin(&rs.device);
            let mut encoder = rs
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("rt-resolve") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rt-resolve-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: gpu_timing.as_mut().and_then(|f| f.render("rt-resolve-pass")),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.resolve_pipeline);
            pass.set_bind_group(0, resolve_bg, &[]);
            pass.draw(0..3, 0..1);
        }
        crate::performance::submit(rs, encoder, gpu_timing);
    }

    /// Begin a tiled converged trace of `total_samples` paths/pixel at `size`. Drive it with
    /// [`trace_step`](Self::trace_step) (a bounded number of tile-submits per frame) until it
    /// returns `true`. Spreading the submits across frames keeps the UI responsive and the
    /// image refines progressively, while each submit's GPU time stays bounded — so no submit
    /// trips the watchdog (a single whole-image dispatch on a big scene loses the device).
    pub fn trace_begin(
        &mut self,
        rs: &RenderState,
        size: [u32; 2],
        uniform: RtUniform,
        total_samples: u32,
    ) {
        if !self.has_scene {
            return;
        }
        self.ensure_accum(&rs.device, size);
        self.ensure_bindings(rs);
        self.total_samples = 0;
        self.read_idx = 0;
        // Per-submit sample chunk, bounded by *BVH-ray traversals* counting the rays cast per
        // sample: AO + finite-light quadrature + GI bounces dominate the cost.
        // Keep the CPU budget in sync with the shader's secondary-ray counts.
        const AO_RAYS: u32 = 4; // must match raytrace.wgsl
        const SHADOW_RAYS: u32 = 4; // must match raytrace.wgsl
        const RAY_BUDGET: u32 = 2_000_000;
        let ao_on = uniform.ao[3] > 0.5;
        let shadow_on = uniform.shadow[2] > 0.5;
        let shadow_rays = if !shadow_on { 0 } else if uniform.shadow[3] > 0.0 { SHADOW_RAYS } else { 1 };
        // bg.w is the GI strength (0 = off); GI path-traces `GI_BOUNCES` extra bounces.
        let gi_bounces = if uniform.bg[3] > 0.001 { GI_BOUNCES } else { 0 };
        let mut rays_per_sample =
            (1 + if ao_on { AO_RAYS } else { 0 } + shadow_rays) * (1 + gi_bounces);
        // Weighted transparency may visit many layers per path. Shrink the tile
        // as well as the sample chunk so one dispatch remains bounded.
        if self.has_transparent {
            const MAX_TRANSPARENT_LAYERS: u32 = 256; // same limit as cs_trace
            rays_per_sample += MAX_TRANSPARENT_LAYERS * (1 + gi_bounces) * (1 + u32::from(shadow_on));
        }
        let mut tile_size = TRACE_TILE;
        while tile_size * tile_size * rays_per_sample > RAY_BUDGET && tile_size > 8 {
            tile_size /= 2;
        }
        let chunk_cap = (RAY_BUDGET / (tile_size * tile_size * rays_per_sample)).max(1);
        self.cursor = Some(TraceCursor {
            size,
            uniform,
            total: total_samples.max(1),
            chunk_cap,
            tile_size,
            ox: 0,
            oy: 0,
        });
    }

    /// Advance the in-progress trace by up to `max_submits` tile dispatches, then resolve the
    /// current (complete) average into `target`. Returns `true` once the sample target is
    /// reached (the cursor is then cleared); `true` immediately if no trace is in progress.
    pub fn trace_step(&mut self, rs: &RenderState, target: &wgpu::TextureView, max_submits: u32) -> bool {
        let Some(mut cur) = self.cursor.take() else {
            return true;
        };
        let [w, h] = cur.size;
        let mut submits = 0u32;
        while self.total_samples < cur.total && submits < max_submits.max(1) {
            let prior = self.total_samples;
            let chunk = cur.chunk_cap.min(cur.total - prior);
            let reset = prior == 0;
            let read_idx = self.read_idx;
            let write_idx = 1 - read_idx;
            let trace_bg = &self.trace_bindings.as_ref().unwrap()[read_idx];
            let tw = cur.tile_size.min(w - cur.ox);
            let th = cur.tile_size.min(h - cur.oy);
            let mut u = cur.uniform;
            u.dims = [w, h, chunk, prior];
            u.accum = [prior, u32::from(reset), cur.ox, cur.oy];
            rs.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&u));
            let mut gpu_timing = crate::performance::GpuFrame::begin(&rs.device);
            let mut encoder = rs
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("rt-tile-encoder") });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("rt-tile-pass"),
                    timestamp_writes: gpu_timing.as_mut().and_then(|f| f.compute("rt-tile-pass")),
                });
                pass.set_pipeline(&self.trace_pipeline);
                pass.set_bind_group(0, trace_bg, &[]);
                pass.dispatch_workgroups(tw.div_ceil(8), th.div_ceil(8), 1);
            }
            crate::performance::submit(rs, encoder, gpu_timing);
            submits += 1;
            // Advance the tile sweep; completing the last tile finishes this sample-chunk
            // (bump the running total + swap the ping-pong) and restarts the sweep.
            cur.ox += cur.tile_size;
            if cur.ox >= w {
                cur.ox = 0;
                cur.oy += cur.tile_size;
                if cur.oy >= h {
                    cur.oy = 0;
                    self.total_samples += chunk;
                    self.read_idx = write_idx;
                }
            }
        }
        // Show the latest *complete* chunk (skip until the first one lands → no garbage frame).
        if self.total_samples > 0 {
            self.resolve_into(rs, target);
        }
        let done = self.total_samples >= cur.total;
        if !done {
            self.cursor = Some(cur);
        }
        done
    }

    /// Samples accumulated into the current average (0 until the first chunk completes). The
    /// driver paints the still only once this is > 0, so no pre-first-chunk garbage shows.
    pub fn samples(&self) -> u32 {
        self.total_samples
    }

    /// Abort an in-progress trace (e.g. the camera moved): drop the cursor so no more submits
    /// are issued. The accumulator is left as-is and overwritten by the next `trace_begin`.
    pub fn trace_cancel(&mut self) {
        self.cursor = None;
    }

    /// Blocking tiled render — begin + step to completion (used by the headless debug hook,
    /// where a frozen frame is fine). Polls between batches so the queue can't pile up.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub fn render_tiled(
        &mut self,
        rs: &RenderState,
        target: &wgpu::TextureView,
        size: [u32; 2],
        uniform: RtUniform,
        total_samples: u32,
    ) {
        self.trace_begin(rs, size, uniform, total_samples);
        while !self.trace_step(rs, target, 32) {
            let _ = rs.device.poll(wgpu::PollType::wait_indefinitely());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_geometry_matches_fresh_trace_and_rejects_dirty_reps() {
        use crate::{geometry::{self, RepKind}, scene::Representation};
        let raw = crate::data::load(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb"
        ))).unwrap();
        let mut scene = Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        scene.molecules[0].show_box = false;
        for kind in [RepKind::Vdw, RepKind::Licorice, RepKind::BallAndStick,
            RepKind::Lines, RepKind::Cartoon, RepKind::Surface]
        {
            let mol = &mut scene.molecules[0];
            let mut rep = Representation::new(kind);
            rep.sel = Some(mol.data.evaluate(if kind.draws_mesh() { "name CA" } else { "all" }).unwrap().1);
            rep.material = crate::material::Material::Transparent;
            let bound = mol.data.bind_with_state(rep.sel.as_ref().unwrap(), mol.render_state());
            let ss = geometry::needs_ss(&rep.params, rep.color)
                .then(|| crate::secstruct::SsMap::compute(&bound, rep.ss_algo));
            let geom = geometry::build(&bound, mol.n_atoms, &mol.bonds, &rep.params,
                rep.color_spec(), rep.material, ss.as_ref(), false);
            let mesh_ptr = geom.mesh.vertices.as_ptr();
            rep.cache_geometry(geom, mol.n_atoms, false, false);
            rep.sel_dirty = false;
            rep.geom_dirty = false;
            rep.coords_dirty = false;
            assert!(rep.cached_geometry(false).is_some());
            assert!(rep.cached_geometry(true).is_none());
            if kind.draws_mesh() {
                assert_eq!(mesh_ptr, rep.cached_geometry(false).unwrap().mesh.vertices.as_ptr());
            }
            mol.reps = vec![rep];
            for angle in [0.0, 0.7] {
                let view = RtView { view: glam::Mat4::from_rotation_y(angle),
                    proj: glam::Mat4::IDENTITY, viewport_h: 480.0 };
                let cached = RtScene::gather(&scene, view, false);
                scene.molecules[0].reps[0].coords_dirty = true;
                assert!(scene.molecules[0].reps[0].cached_geometry(false).is_none());
                let fresh = RtScene::gather(&scene, view, false);
                scene.molecules[0].reps[0].coords_dirty = false;
                fn bytes<T: bytemuck::Pod>(v: &[T]) -> &[u8] { bytemuck::cast_slice(v) }
                assert_eq!(bytes(&cached.spheres), bytes(&fresh.spheres), "{kind:?}");
                assert_eq!(bytes(&cached.cylinders), bytes(&fresh.cylinders), "{kind:?}");
                assert_eq!(bytes(&cached.mesh_verts), bytes(&fresh.mesh_verts), "{kind:?}");
                assert_eq!(bytes(&cached.triangles), bytes(&fresh.triangles), "{kind:?}");
                assert_eq!(bytes(&cached.nodes), bytes(&fresh.nodes), "{kind:?}");
                assert_eq!(cached.prim_indices, fresh.prim_indices);
            }
            let rep = &mut scene.molecules[0].reps[0];
            rep.sel_dirty = true;
            assert!(rep.cached_geometry(false).is_none());
            rep.sel_dirty = false;
            rep.geom_dirty = true;
            assert!(rep.cached_geometry(false).is_none());
            rep.geom_dirty = false;
            rep.cache_geometry(Default::default(), 0, false, true);
            assert!(rep.cached_geometry(false).is_none(), "gray draw-mode geometry is not reusable");
        }
    }

    #[test]
    fn envelope_groups_isolate_transparent_representations() {
        use crate::{geometry::RepKind, material::Material, scene::Representation};
        let raw = crate::data::load(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb"))).unwrap();
        let mut scene = Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        let mol = &mut scene.molecules[0];
        mol.reps.clear();
        for (kind, material) in [(RepKind::BallAndStick, Material::Transparent), (RepKind::BallAndStick, Material::Glass), (RepKind::BallAndStick, Material::Opaque), (RepKind::Vdw, Material::Transparent)] {
            let mut rep = Representation::new(kind);
            rep.material = material;
            rep.sel = Some(mol.data.select_all());
            mol.reps.push(rep);
        }
        let view = RtView { view: glam::Mat4::IDENTITY, proj: glam::Mat4::IDENTITY, viewport_h: 480.0 };
        let data = RtScene::gather(&scene, view, false);
        let sphere_groups: std::collections::BTreeSet<_> = data.spheres.iter().map(|s| s.m[2]).collect();
        let bond_groups: std::collections::BTreeSet<_> = data.cylinders.iter().map(|c| c.m[3] >> 1).collect();
        assert_eq!(sphere_groups, [0, 1, 2, 3].into_iter().collect());
        assert_eq!(bond_groups, [0, 1, 2].into_iter().collect());
        assert!(data.cylinders.iter().all(|c| c.m[3] & 1 == 0));
    }

    fn sph(x: f32, y: f32, z: f32, r: f32) -> GpuSphere {
        GpuSphere { c: [x, y, z, r], m: [0, 0, 0, 0] }
    }

    fn hit_aabb(min: Vec3, max: Vec3, o: Vec3, inv: Vec3, t_min: f32, t_max: f32) -> bool {
        let t0 = (min - o) * inv;
        let t1 = (max - o) * inv;
        let tsmall = t0.min(t1);
        let tbig = t0.max(t1);
        let enter = tsmall.x.max(tsmall.y).max(tsmall.z).max(t_min);
        let exit = tbig.x.min(tbig.y).min(tbig.z).min(t_max);
        enter <= exit
    }

    fn ray_sphere(s: &GpuSphere, o: Vec3, d: Vec3) -> Option<f32> {
        let center = Vec3::new(s.c[0], s.c[1], s.c[2]);
        let oc = o - center;
        let b = oc.dot(d);
        let c = oc.dot(oc) - s.c[3] * s.c[3];
        let disc = b * b - c;
        if disc < 0.0 {
            return None;
        }
        let t = -b - disc.sqrt();
        (t > 1e-4).then_some(t)
    }

    /// Build a sphere-only RtScene (mirrors `gather`'s BVH path) for the traversal tests.
    fn sphere_scene(spheres: Vec<GpuSphere>) -> RtScene {
        let aabbs: Vec<Aabb> = spheres.iter().map(sphere_aabb).collect();
        let _timing = crate::performance::span("rt-bvh");
        let (nodes, order) = build_bvh(&aabbs);
        // all spheres → tag(SPHERE, i); order is a permutation, so prim_indices = order.
        let prim_indices = order.iter().map(|&i| tag(TAG_SPHERE, i as usize)).collect();
        RtScene { spheres, nodes, prim_indices, ..Default::default() }
    }

    fn bvh_closest(scene: &RtScene, o: Vec3, d: Vec3) -> Option<(u32, f32)> {
        if scene.nodes.is_empty() {
            return None;
        }
        let inv = Vec3::new(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
        let mut best: Option<(u32, f32)> = None;
        let mut t_max = f32::INFINITY;
        let mut stack = vec![0u32];
        while let Some(ni) = stack.pop() {
            let node = scene.nodes[ni as usize];
            if !hit_aabb(node.min(), node.max(), o, inv, 1e-4, t_max) {
                continue;
            }
            let count = node.count();
            if count == 0 {
                stack.push(node.link());
                stack.push(node.link() + 1);
            } else {
                let first = node.link() as usize;
                for k in 0..count as usize {
                    let idx = scene.prim_indices[first + k] & TAG_MASK;
                    if let Some(t) = ray_sphere(&scene.spheres[idx as usize], o, d) {
                        if t < t_max {
                            t_max = t;
                            best = Some((idx, t));
                        }
                    }
                }
            }
        }
        best
    }

    fn brute_closest(scene: &RtScene, o: Vec3, d: Vec3) -> Option<(u32, f32)> {
        let mut best: Option<(u32, f32)> = None;
        for (i, s) in scene.spheres.iter().enumerate() {
            if let Some(t) = ray_sphere(s, o, d) {
                if best.is_none_or(|(_, bt)| t < bt) {
                    best = Some((i as u32, t));
                }
            }
        }
        best
    }

    #[test]
    fn bvh_covers_all_primitives() {
        let spheres: Vec<GpuSphere> =
            (0..50).map(|i| sph(i as f32 * 0.7, (i % 7) as f32, (i % 3) as f32, 0.3)).collect();
        let aabbs: Vec<Aabb> = spheres.iter().map(sphere_aabb).collect();
        let _timing = crate::performance::span("rt-bvh");
        let (nodes, order) = build_bvh(&aabbs);
        assert!(!nodes.is_empty());
        assert_eq!(order.len(), spheres.len());
        let mut seen = vec![false; spheres.len()];
        for n in &nodes {
            if n.count() > 0 {
                let first = n.link() as usize;
                for k in 0..n.count() as usize {
                    let pi = order[first + k] as usize;
                    assert!(!seen[pi], "primitive {pi} in two leaves");
                    seen[pi] = true;
                }
            }
        }
        assert!(seen.iter().all(|&s| s), "every primitive is in a leaf");
    }

    #[test]
    #[ignore = "requires native GPU; compares BVH traversal against the original WGSL"]
    fn near_first_traversal_matches_original_mixed_hits() {
        let rs = super::super::appearance_tests::gpu();
        let raw = crate::data::load(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb"
        ))).unwrap();
        let center = (raw.bbox_min + raw.bbox_max) * 0.5;
        let radius = (raw.bbox_max - raw.bbox_min).length();
        let mut scene = Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        let mol = &mut scene.molecules[0];
        let mut rep = crate::scene::Representation::new(crate::geometry::RepKind::BallAndStick);
        rep.params = crate::geometry::RepParams::BallAndStick {
            sphere_scale: 0.3, bond_radius: 0.02, bond_smoothing: 0.7, bond_color_blend: 0.0,
        };
        rep.sel = Some(mol.data.select_all());
        rep.material = crate::material::Material::Transparent;
        mol.reps = vec![rep];
        let view = RtView { view: glam::Mat4::IDENTITY, proj: glam::Mat4::IDENTITY, viewport_h: 480.0 };
        let mut data = RtScene::gather(&scene, view, false);
        for sphere in &mut data.spheres { sphere.m[2] = 1; }
        // Mixed transparent envelope + opaque sphere + triangle leaf types.
        data.spheres.push(GpuSphere { c: [center.x, center.y, center.z, 0.25], m: [0xffffffff, 0, 0, 0] });
        data.mesh_verts = [center + Vec3::X, center + Vec3::Y, center - Vec3::X].into_iter()
            .map(|p| GpuMeshVertex { p: [p.x, p.y, p.z, f32::from_bits(0xffffffff)], n: [0.0,0.0,1.0,0.0] }).collect();
        data.triangles = vec![GpuTriangle { i: [0,1,2,0] }];
        let mut bounds = Vec::new();
        let mut tags = Vec::new();
        for (i, sp) in data.spheres.iter().enumerate() { bounds.push(sphere_aabb(sp)); tags.push(tag(TAG_SPHERE, i)); }
        for (i, cy) in data.cylinders.iter().enumerate() { bounds.push(cylinder_aabb(cy)); tags.push(tag(TAG_CYLINDER, i)); }
        bounds.push(triangle_aabb(&data.mesh_verts, 0, 1, 2)); tags.push(tag(TAG_TRIANGLE, 0));
        let (nodes, order) = build_bvh(&bounds);
        data.nodes = nodes;
        data.prim_indices = order.into_iter().map(|i| tags[i as usize]).collect();
        let mut rays: Vec<[[f32; 4]; 2]> = seeded_bounds(1024).iter().map(|a| {
            let direction = (a.centroid() - Vec3::splat(50.0)).normalize();
            let origin = center + direction * radius;
            [origin.extend(0.0).to_array(), (-direction).extend(0.0).to_array()]
        }).collect();
        for direction in [Vec3::X, Vec3::Y, Vec3::Z, -Vec3::X, -Vec3::Y, -Vec3::Z] {
            rays.push([(center + direction * radius).extend(0.0).to_array(), (-direction).extend(0.0).to_array()]);
            rays.push([center.extend(0.0).to_array(), direction.extend(0.0).to_array()]);
        }
        let source = format!("{}\n{}\n{}", super::super::lit_shader_source(include_str!("shaders/raytrace.wgsl")),
            include_str!("shaders/raytrace_traversal_reference.wgsl")
                .replace("fn closest_hit_filtered(", "fn closest_hit_filtered_reference(")
                .replace("fn any_hit(", "fn any_hit_reference("), r#"
struct TestRay { origin: vec4<f32>, direction: vec4<f32> }
@group(1) @binding(0) var<storage, read> test_rays: array<TestRay>;
@group(1) @binding(1) var<storage, read_write> test_results: array<vec4<u32>>;
@compute @workgroup_size(64)
fn test_traversal(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= arrayLength(&test_rays)) { return; }
    let ray = test_rays[id.x];
    var flags = 0u;
    for (var opaque = 0u; opaque < 2u; opaque++) {
        let actual = closest_hit_filtered(ray.origin.xyz, ray.direction.xyz, opaque == 1u);
        let expected = closest_hit_filtered_reference(ray.origin.xyz, ray.direction.xyz, opaque == 1u);
        if (actual.prim != expected.prim || actual.t != expected.t || ((actual.prim >> 30u) == 2u && any(actual.uv != expected.uv))) { flags |= 1u << opaque; }
    }
    for (var exits = 0u; exits < 2u; exits++) {
        let actual = any_hit(ray.origin.xyz, ray.direction.xyz, 100.0, exits == 1u, 0xffffffffu);
        let expected = any_hit_reference(ray.origin.xyz, ray.direction.xyz, 100.0, exits == 1u, 0xffffffffu);
        if (actual != expected) { flags |= 4u << exits; }
    }
    test_results[id.x] = vec4<u32>(flags, 0u, 0u, 0u);
}"#);
        let device = &rs.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None, layout: None, module: &module, entry_point: Some("test_traversal"), compilation_options: Default::default(), cache: None,
        });
        let buffers: Vec<_> = [bytemuck::cast_slice(&data.spheres), bytemuck::cast_slice(&data.cylinders),
            bytemuck::cast_slice(&data.mesh_verts), bytemuck::cast_slice(&data.triangles),
            bytemuck::cast_slice(&data.nodes), bytemuck::cast_slice(&data.prim_indices)]
            .into_iter().map(|bytes| device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None, contents: bytes, usage: wgpu::BufferUsages::STORAGE,
            })).collect();
        let entries: Vec<_> = buffers.iter().enumerate().map(|(i,b)| wgpu::BindGroupEntry { binding: i as u32 + 1, resource: b.as_entire_binding() }).collect();
        let geometry = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &pipeline.get_bind_group_layout(0), entries: &entries });
        let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&rays), usage: wgpu::BufferUsages::STORAGE });
        let size = rays.len() as u64 * 16;
        let output = device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        let readback = device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let test = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &pipeline.get_bind_group_layout(1), entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: input.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: output.as_entire_binding() },
        ] });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline); pass.set_bind_group(0, &geometry, &[]); pass.set_bind_group(1, &test, &[]);
            pass.dispatch_workgroups((rays.len() as u32).div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, size);
        rs.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |r| { tx.send(r).unwrap(); });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap(); rx.recv().unwrap().unwrap();
        let mapped = readback.slice(..).get_mapped_range();
        for (i, flags) in bytemuck::cast_slice::<_, [u32; 4]>(&mapped).iter().enumerate() {
            assert_eq!(flags[0], 0, "ray {i}: closest/opaque/any-hit mismatch");
        }
    }

    #[test]
    fn prepared_scene_keys_track_output_revisions_and_view_dependencies() {
        let raw = crate::data::load(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb"))).unwrap();
        let mut scene = Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        let view = RtView { view: glam::Mat4::IDENTITY, proj: glam::Mat4::IDENTITY, viewport_h: 480.0 };
        let moved = RtView { view: glam::Mat4::from_rotation_y(0.7), viewport_h: 960.0, ..view };
        let mol = &mut scene.molecules[0];
        let mut rep = crate::scene::Representation::new(crate::geometry::RepKind::Vdw);
        rep.sel = Some(mol.data.select_all());
        rep.sel_dirty = false;
        rep.cache_geometry(Default::default(), mol.n_atoms, false, false);
        mol.reps = vec![rep]; mol.show_box = false;
        let base = PreparedKey::new(&scene, view, false).unwrap();
        assert!(PreparedKey::new(&scene, moved, false).as_ref() == Some(&base));
        assert!(PreparedKey::new(&scene, moved, true).is_none());
        scene.molecules[0].reps[0].coords_dirty = true;
        assert!(PreparedKey::new(&scene, view, false).is_none());
        scene.molecules[0].reps[0].coords_dirty = false;
        scene.molecules[0].reps[0].geom_dirty = true;
        assert!(PreparedKey::new(&scene, view, false).is_none());
        scene.molecules[0].reps[0].geom_dirty = false;
        scene.molecules[0].reps[0].sel_dirty = true;
        assert!(PreparedKey::new(&scene, view, false).is_none());
        scene.molecules[0].reps[0].sel_dirty = false;
        scene.molecules[0].reps[0].periodic.pos[0] = 1;
        assert!(PreparedKey::new(&scene, view, false).as_ref() != Some(&base));
        scene.molecules[0].reps[0].periodic = Default::default();
        scene.molecules[0].show_box = true;
        assert!(PreparedKey::new(&scene, moved, false) != PreparedKey::new(&scene, view, false));
        scene.molecules[0].show_box = false;
        scene.molecules[0].reps[0].cache_geometry(crate::geometry::GeometryData {
            lines: vec![crate::render::LineVertex { pos: [0.0;3], color:0, width:1.0, offset_px:0.0 }],
            ..Default::default()
        }, 0, false, false);
        assert!(PreparedKey::new(&scene, view, false).as_ref() != Some(&base));
        assert!(PreparedKey::new(&scene, moved, false) != PreparedKey::new(&scene, view, false));
        scene.molecules[0].reps[0].visible = false;
        assert!(PreparedKey::new(&scene, view, false).as_ref() != Some(&base));
        scene.molecules[0].visible = false;
        assert!(PreparedKey::new(&scene, view, false).as_ref() != Some(&base));
    }

    #[test]
    fn refitted_bvh_matches_full_build_through_motion_and_rebuild_interval() {
        let raw = crate::data::load(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb"))).unwrap();
        let mut scene = Scene::default(); scene.add(raw, &crate::settings::RepDefaults::default());
        let mol = &mut scene.molecules[0];
        let mut rep = crate::scene::Representation::new(crate::geometry::RepKind::Vdw);
        rep.sel = Some(mol.data.select_all()); mol.reps = vec![rep]; mol.show_box = false;
        let view = RtView { view: glam::Mat4::IDENTITY, proj: glam::Mat4::IDENTITY, viewport_h:480.0 };
        let mut cache: Option<BvhCache> = None;
        let mut saw_refit = false;
        let mut saw_rebuild = false;
        for frame in 0..20 {
            let mol = &mut scene.molecules[0];
            let bound = mol.data.bind_with_state(mol.reps[0].sel.as_ref().unwrap(), mol.render_state());
            let mut geom = crate::geometry::build(&bound, mol.n_atoms, &mol.bonds,
                &mol.reps[0].params, mol.reps[0].color_spec(), mol.reps[0].material, None, false);
            for (i,sphere) in geom.spheres.iter_mut().enumerate() {
                sphere.center[0] += (frame as f32 * 0.1 + i as f32).sin() * 0.01;
            }
            mol.reps[0].cache_geometry(geom, mol.n_atoms, false, false);
            mol.reps[0].sel_dirty = false; mol.reps[0].geom_dirty = false; mol.reps[0].coords_dirty = false;
            let mut actual = RtScene::gather_with_bvh_cache(&scene, view, false, &mut cache);
            let expected = RtScene::gather(&scene, view, false);
            assert_eq!(bytemuck::cast_slice::<_,u8>(&actual.spheres), bytemuck::cast_slice::<_,u8>(&expected.spheres));
            for x in 0..7 {
                let origin = Vec3::new(x as f32, 2.5, -10.0);
                let a = bvh_closest(&actual, origin, Vec3::Z);
                let b = brute_closest(&expected, origin, Vec3::Z);
                assert_eq!(a.map(|h| h.1), b.map(|h| h.1));
            }
            let c = cache.as_mut().unwrap();
            saw_refit |= c.refits > 0;
            saw_rebuild |= frame > 0 && c.refits == 0;
            c.nodes = std::mem::take(&mut actual.nodes);
            c.order = std::mem::take(&mut actual.prim_indices);
        }
        assert!(saw_refit && saw_rebuild);
        scene.molecules[0].visible = false;
        assert!(RtScene::gather_with_bvh_cache(&scene, view, false, &mut cache).is_empty());
        assert!(cache.is_none());
    }

    #[test]
    #[ignore = "requires native GPU; verifies scene and storage-buffer reuse"]
    fn camera_only_preparation_reuses_buffers_and_empty_scenes_release_storage() {
        let rs = super::super::appearance_tests::gpu();
        let raw = crate::data::load(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb"))).unwrap();
        let mut scene = Scene::default(); scene.add(raw, &crate::settings::RepDefaults::default());
        let mol = &mut scene.molecules[0];
        let mut rep = crate::scene::Representation::new(crate::geometry::RepKind::Vdw);
        rep.sel = Some(mol.data.select_all());
        let bound = mol.data.bind_with_state(rep.sel.as_ref().unwrap(), mol.render_state());
        let geom = crate::geometry::build(&bound, mol.n_atoms, &mol.bonds, &rep.params,
            rep.color_spec(), rep.material, None, false);
        rep.cache_geometry(geom, mol.n_atoms, false, false);
        rep.sel_dirty = false; mol.reps = vec![rep]; mol.show_box = false;
        let view = RtView { view: glam::Mat4::IDENTITY, proj: glam::Mat4::IDENTITY, viewport_h:480.0 };
        let mut rt = Raytracer::new(&rs, wgpu::TextureFormat::Rgba8Unorm).unwrap();
        rt.prepare(&rs, &scene, view, false);
        let spheres = rt.spheres.as_ref().unwrap().clone();
        let nodes = rt.nodes.as_ref().unwrap().clone();
        rt.prepare(&rs, &scene, RtView { view: glam::Mat4::from_rotation_y(0.7), ..view }, false);
        assert_eq!(rt.spheres.as_ref().unwrap(), &spheres);
        assert_eq!(rt.nodes.as_ref().unwrap(), &nodes);
        assert_eq!(rt.bvh_cache.as_ref().unwrap().refits, 0, "camera-only change must not refit");
        let mut data = RtScene::gather(&scene, view, false);
        data.spheres.truncate(8);
        let short = sphere_scene(data.spheres);
        rt.upload(&rs, &short);
        assert_eq!(rt.spheres.as_ref().unwrap(), &spheres);
        assert_eq!(rt.nodes.as_ref().unwrap(), &nodes);
        rt.upload(&rs, &RtScene::default());
        assert!(rt.spheres.is_none() && rt.nodes.is_none() && rt.trace_bindings.is_none() && rt.bvh_cache.is_none());
    }

    fn seeded_bounds(n: usize) -> Vec<Aabb> {
        let mut seed = 19u32;
        let mut next = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / 16777216.0
        };
        (0..n).map(|_| {
            let center = Vec3::new(next(), next(), next()) * 100.0;
            let radius = Vec3::splat(next() * 0.3 + 0.01);
            Aabb { min: center - radius, max: center + radius }
        }).collect()
    }

    #[test]
    fn prefix_sah_preserves_reference_tree_and_primitive_order() {
        for n in [0, 1, 4, 5, 31, 512, 4096] {
            let bounds = seeded_bounds(n);
            let (nodes, order) = build_bvh(&bounds);
            let (reference, reference_order) = build_bvh_reference(&bounds);
            assert_eq!(bytemuck::cast_slice::<_, u8>(&nodes), bytemuck::cast_slice::<_, u8>(&reference));
            assert_eq!(order, reference_order);
        }
    }

    #[test]
    fn bvh_depth_and_bounds_fit_shader_stack_for_adversarial_inputs() {
        let distributions = [seeded_bounds(16384),
            vec![Aabb::point(Vec3::ZERO); 512],
            (0..128).map(|i| Aabb::point(Vec3::new(1e17 * 0.5_f32.powi(i), 0.0, 0.0))).collect()];
        for bounds in distributions {
            let (nodes, order) = build_bvh(&bounds);
            let mut visited = vec![false; bounds.len()];
            let mut stack = vec![(0usize, 0usize)];
            while let Some((id, depth)) = stack.pop() {
                assert!(depth <= MAX_BVH_DEPTH);
                let node = nodes[id];
                if node.count() == 0 {
                    for child in [node.link() as usize, node.link() as usize + 1] {
                        assert!(nodes[child].min().cmpge(node.min()).all());
                        assert!(nodes[child].max().cmple(node.max()).all());
                        stack.push((child, depth + 1));
                    }
                } else {
                    for &primitive in &order[node.link() as usize..(node.link() + node.count()) as usize] {
                        let index = primitive as usize;
                        assert!(!visited[index]);
                        visited[index] = true;
                        assert!(bounds[index].min.cmpge(node.min()).all());
                        assert!(bounds[index].max.cmple(node.max()).all());
                    }
                }
            }
            assert!(visited.into_iter().all(|v| v));
        }
    }

    #[test]
    #[ignore = "manual CPU BVH benchmark"]
    fn benchmark_bvh_prefix_splits() {
        use std::{hint::black_box, time::Instant};
        for n in [1024, 16384, 131072] {
            let bounds = seeded_bounds(n);
            let mut full = Vec::new();
            let mut optimized = Vec::new();
            for _ in 0..6 {
                let start = Instant::now();
                black_box(build_bvh_reference(&bounds));
                full.push(start.elapsed());
                let start = Instant::now();
                black_box(build_bvh(&bounds));
                optimized.push(start.elapsed());
            }
            full.remove(0); optimized.remove(0);
            full.sort(); optimized.sort();
            eprintln!("BVH {n}: baseline={:?} prefix={:?} speedup={:.2}x",
                full[2], optimized[2], full[2].as_secs_f64()/optimized[2].as_secs_f64());
        }
    }

    #[test]
    fn bvh_matches_brute_force() {
        let mut spheres = Vec::new();
        for x in 0..6 {
            for y in 0..6 {
                for z in 0..6 {
                    spheres.push(sph(x as f32, y as f32, z as f32, 0.35));
                }
            }
        }
        let scene = sphere_scene(spheres);
        let rays = [
            (Vec3::new(-5.0, 2.0, 2.0), Vec3::new(1.0, 0.0, 0.0)),
            (Vec3::new(2.5, 2.5, -5.0), Vec3::new(0.0, 0.0, 1.0)),
            (Vec3::new(-3.0, -3.0, -3.0), Vec3::new(1.0, 1.0, 1.0).normalize()),
            (Vec3::new(10.0, 2.0, 2.0), Vec3::new(-1.0, 0.0, 0.0)),
            (Vec3::new(2.0, 10.0, 2.0), Vec3::new(0.0, -1.0, 0.05).normalize()),
        ];
        for (o, d) in rays {
            let a = bvh_closest(&scene, o, d);
            let b = brute_closest(&scene, o, d);
            match (a, b) {
                (Some((_, ta)), Some((_, tb))) => {
                    assert!((ta - tb).abs() < 1e-3, "t mismatch: bvh {ta} vs brute {tb}");
                }
                (None, None) => {}
                _ => panic!("hit/miss disagreement: bvh {a:?} vs brute {b:?}"),
            }
        }
    }

    #[test]
    fn single_sphere_is_a_leaf_root() {
        let (nodes, order) = build_bvh(&[sphere_aabb(&sph(0.0, 0.0, 0.0, 1.0))]);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].count(), 1);
        assert_eq!(order, vec![0]);
    }

    #[test]
    fn cylinder_aabb_is_never_degenerate() {
        // An axis-aligned bond: the union-of-end-spheres box must have nonzero thickness.
        let c = GpuCylinder { profile: [0.0; 4], lane: [0.0; 4], c0: [0.0, 0.0, 0.0, 0.1], c1: [1.0, 0.0, 0.0, 0.0], m: [0, 0, 0, 0] };
        let a = cylinder_aabb(&c);
        assert!(a.max.y - a.min.y >= 0.19 && a.max.z - a.min.z >= 0.19);
        assert!(a.min.x <= -0.1 && a.max.x >= 1.1);
    }
}
