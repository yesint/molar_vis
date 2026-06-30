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

// Primitive type tags, packed into the top 2 bits of each `prim_indices` entry.
const TAG_SHIFT: u32 = 30;
const TAG_MASK: u32 = (1 << TAG_SHIFT) - 1;
const TAG_SPHERE: u32 = 0;
const TAG_CYLINDER: u32 = 1;
const TAG_TRIANGLE: u32 = 2;
fn tag(typ: u32, idx: usize) -> u32 {
    (typ << TAG_SHIFT) | (idx as u32 & TAG_MASK)
}

/// A sphere primitive: `c = (center.xyz, radius)`, `m = (color, mat, _, _)`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct GpuSphere {
    pub c: [f32; 4],
    pub m: [u32; 4],
}

/// A (capless) cylinder primitive: `c0 = (p0.xyz, radius)`, `c1 = (p1.xyz, _)`,
/// `m = (color, mat, _, _)`. Sphere caps at joints come from separate sphere prims.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct GpuCylinder {
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

/// A mesh triangle = three indices into the shared vertex array (`.w` unused).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct GpuTriangle {
    pub i: [u32; 4],
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
    /// Whether there's anything to trace.
    pub fn is_empty(&self) -> bool {
        self.prim_indices.is_empty()
    }

    /// Gather all visible primitives from the scene (re-running `geometry::build` per
    /// visible representation, exactly as `rebuild_dirty` does — same displayed frame /
    /// smoothing) and build the BVH. `dashed_pbc` matches the live render's setting.
    pub fn gather(scene: &Scene, dashed_pbc: bool) -> Self {
        let mut s = Self::default();
        let mut aabbs: Vec<Aabb> = Vec::new();
        let mut tags: Vec<u32> = Vec::new();
        s.collect(scene, dashed_pbc, &mut aabbs, &mut tags);
        if aabbs.is_empty() {
            return s;
        }
        let (nodes, order) = build_bvh(&aabbs);
        s.nodes = nodes;
        s.prim_indices = order.into_iter().map(|i| tags[i as usize]).collect();
        s
    }

    /// Re-run `geometry::build` for every visible rep and append its spheres, cylinders,
    /// and mesh triangles, accumulating each primitive's AABB + type-tag.
    fn collect(&mut self, scene: &Scene, dashed_pbc: bool, aabbs: &mut Vec<Aabb>, tags: &mut Vec<u32>) {
        use crate::geometry;
        use crate::secstruct::SsMap;

        for mol in &scene.molecules {
            if !mol.visible {
                continue;
            }
            let render_state = match mol.trajectory.frames.get(mol.trajectory.current) {
                Some(frame) => frame,
                None => mol.data.state(),
            };
            for rep in &mol.reps {
                if !rep.visible {
                    continue;
                }
                let Some(sel) = rep.sel.as_ref() else { continue };
                let smoothed = (rep.smooth_window > 1)
                    .then(|| mol.trajectory.smoothed_state(rep.smooth_window))
                    .flatten();
                let state = smoothed.as_ref().unwrap_or(render_state);

                let bound = mol.data.bind_with_state(sel, state);
                let ss = geometry::needs_ss(&rep.params, rep.color)
                    .then(|| SsMap::compute(&bound, rep.ss_algo));
                let geom = geometry::build(
                    &bound,
                    mol.n_atoms,
                    &mol.bonds,
                    &rep.params,
                    rep.color,
                    rep.material,
                    ss.as_ref(),
                    dashed_pbc,
                );

                for sp in &geom.spheres {
                    let gs = GpuSphere {
                        c: [sp.center[0], sp.center[1], sp.center[2], sp.radius],
                        m: [sp.color, sp.mat, 0, 0],
                    };
                    aabbs.push(sphere_aabb(&gs));
                    tags.push(tag(TAG_SPHERE, self.spheres.len()));
                    self.spheres.push(gs);
                }
                for cy in &geom.cylinders {
                    let gc = GpuCylinder {
                        c0: [cy.p0[0], cy.p0[1], cy.p0[2], cy.radius],
                        c1: [cy.p1[0], cy.p1[1], cy.p1[2], 0.0],
                        m: [cy.color, cy.mat, 0, 0],
                    };
                    aabbs.push(cylinder_aabb(&gc));
                    tags.push(tag(TAG_CYLINDER, self.cylinders.len()));
                    self.cylinders.push(gc);
                }
                // Mesh: append vertices (offset triangle indices into the shared array).
                let base = self.mesh_verts.len() as u32;
                for v in &geom.mesh.vertices {
                    self.mesh_verts.push(GpuMeshVertex {
                        p: [v.pos[0], v.pos[1], v.pos[2], f32::from_bits(v.color)],
                        n: [v.normal[0], v.normal[1], v.normal[2], f32::from_bits(v.mat)],
                    });
                }
                for t in geom.mesh.indices.chunks_exact(3) {
                    let (i0, i1, i2) = (base + t[0], base + t[1], base + t[2]);
                    aabbs.push(triangle_aabb(&self.mesh_verts, i0, i1, i2));
                    tags.push(tag(TAG_TRIANGLE, self.triangles.len()));
                    self.triangles.push(GpuTriangle { i: [i0, i1, i2, 0] });
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

/// Cylinder AABB = union of the two end-spheres (`p0±r`, `p1±r`) — correct and never
/// degenerate (a segment-only box would be a zero-thickness slab for an axis-aligned bond).
fn cylinder_aabb(c: &GpuCylinder) -> Aabb {
    let p0 = Vec3::new(c.c0[0], c.c0[1], c.c0[2]);
    let p1 = Vec3::new(c.c1[0], c.c1[1], c.c1[2]);
    let r = Vec3::splat(c.c0[3].max(0.0));
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
/// (mat4x4 + 7×vec4 = 176 bytes, 16-byte aligned).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct RtUniform {
    pub inv_view_proj: [[f32; 4]; 4],
    /// xyz = eye world pos; w = perspective flag (1 persp / 0 ortho).
    pub eye: [f32; 4],
    /// xyz = world-space direction toward the key light (shadow ray).
    pub light_dir: [f32; 4],
    /// xyz = world-space direction toward the shading headlight.
    pub head_dir: [f32; 4],
    /// radius (nm), bias, strength, enabled.
    pub ao: [f32; 4],
    /// strength, bias, enabled, _.
    pub shadow: [f32; 4],
    /// background color (linear), w unused.
    pub bg: [f32; 4],
    /// width, height, samples-this-step, frame_seed. Set by `render`.
    pub dims: [u32; 4],
    /// Progressive accumulation: prior_total_samples, reset(0/1), _, _. Set by `render`.
    pub accum: [u32; 4],
}

/// GPU ray-tracing resources (compute tracer + fullscreen resolve). Created only on a
/// device with compute + `Rgba32Float` storage (WebGPU/native); `None` on WebGL2.
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
    has_scene: bool,
    // Linear HDR accumulators (ping-pong: read one, write the other, swap). Each holds the
    // running *average* radiance. Recreated on size change.
    accum: Option<[(wgpu::Texture, wgpu::TextureView); 2]>,
    accum_size: [u32; 2],
    /// Which accumulator is the current (latest) one to read from / resolve.
    read_idx: usize,
    /// Samples accumulated so far (the running-average weight). Reset on camera change.
    total_samples: u32,
}

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
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/raytrace.wgsl").into()),
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
            has_scene: false,
            accum: None,
            accum_size: [0, 0],
            read_idx: 0,
            total_samples: 0,
        })
    }

    /// (Re)upload the scene's primitive + BVH buffers. Call when geometry changes.
    pub fn upload(&mut self, rs: &RenderState, scene: &RtScene) {
        self.has_scene = !scene.is_empty();
        if !self.has_scene {
            return;
        }
        let device = &rs.device;
        // A WGSL `array<T>` storage binding needs a non-empty buffer; pad empty primitive
        // classes with one zeroed element (never referenced — its tag is absent).
        let mk = |bytes: &[u8], stride: usize, label| {
            let pad = [0u8; 64];
            let data = if bytes.is_empty() { &pad[..stride] } else { bytes };
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: data,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            })
        };
        self.spheres = Some(mk(bytemuck::cast_slice(&scene.spheres), 32, "rt-spheres"));
        self.cylinders = Some(mk(bytemuck::cast_slice(&scene.cylinders), 48, "rt-cylinders"));
        self.mesh_verts = Some(mk(bytemuck::cast_slice(&scene.mesh_verts), 32, "rt-mesh-verts"));
        self.triangles = Some(mk(bytemuck::cast_slice(&scene.triangles), 16, "rt-triangles"));
        self.nodes = Some(mk(bytemuck::cast_slice(&scene.nodes), 32, "rt-nodes"));
        self.prim_indices = Some(mk(bytemuck::cast_slice(&scene.prim_indices), 4, "rt-prim-indices"));
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
            self.accum = Some([mk(), mk()]);
            self.accum_size = size;
            self.read_idx = 0;
            self.total_samples = 0;
        }
    }

    /// Trace `spp` more sample-paths into the running-average accumulator at `size`, then
    /// resolve (tonemap) the current average into `target`. `reset` clears the average
    /// (camera/scene change); pass `reset = true, spp = N` for a one-shot converged file
    /// render. No-op if no scene is uploaded.
    pub fn render(
        &mut self,
        rs: &RenderState,
        target: &wgpu::TextureView,
        size: [u32; 2],
        mut uniform: RtUniform,
        reset: bool,
        spp: u32,
    ) {
        if !self.has_scene {
            return;
        }
        self.ensure_accum(&rs.device, size);
        if reset {
            self.total_samples = 0;
            self.read_idx = 0;
        }
        let spp = spp.max(1);
        uniform.dims[2] = spp;
        uniform.dims[3] = self.total_samples; // varies the RNG per accumulation step
        uniform.accum = [self.total_samples, u32::from(reset), 0, 0];
        rs.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniform));

        let spheres = self.spheres.as_ref().unwrap();
        let cylinders = self.cylinders.as_ref().unwrap();
        let mesh_verts = self.mesh_verts.as_ref().unwrap();
        let triangles = self.triangles.as_ref().unwrap();
        let nodes = self.nodes.as_ref().unwrap();
        let prim_indices = self.prim_indices.as_ref().unwrap();
        let accums = self.accum.as_ref().unwrap();
        let read_view = &accums[self.read_idx].1;
        let write_idx = 1 - self.read_idx;
        let write_view = &accums[write_idx].1;

        let trace_bg = rs.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rt-trace-bg"),
            layout: &self.trace_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: spheres.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: cylinders.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: mesh_verts.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: triangles.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: nodes.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: prim_indices.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(write_view) },
                wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::TextureView(read_view) },
            ],
        });
        let resolve_bg = rs.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rt-resolve-bg"),
            layout: &self.resolve_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform_buf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(write_view),
                },
            ],
        });

        let mut encoder = rs
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("rt-encoder") });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("rt-trace-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.trace_pipeline);
            pass.set_bind_group(0, &trace_bg, &[]);
            pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
        }
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
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.resolve_pipeline);
            pass.set_bind_group(0, &resolve_bg, &[]);
            pass.draw(0..3, 0..1);
        }
        rs.queue.submit(std::iter::once(encoder.finish()));
        self.total_samples += spp;
        self.read_idx = write_idx; // the just-written average is next frame's source
    }

    /// Tiled, multi-submit file render: trace `total_samples` paths/pixel by sweeping the
    /// image in `TILE`×`TILE` blocks over many short GPU submits (a bounded sample-chunk per
    /// block, polling between submits), then resolve once into `target`. Keeping each submit
    /// well under the driver's command-timeout is what stops a huge scene from hanging a
    /// single whole-image dispatch and **losing the device** (the reported crash). Used by the
    /// "Save image" path; the in-place viewport uses [`render`](Self::render) (single
    /// full-image submit, gated to small scenes, so a tiny per-frame cost).
    pub fn render_tiled(
        &mut self,
        rs: &RenderState,
        target: &wgpu::TextureView,
        size: [u32; 2],
        base_uniform: RtUniform,
        total_samples: u32,
    ) {
        if !self.has_scene {
            return;
        }
        self.ensure_accum(&rs.device, size);
        self.total_samples = 0;
        self.read_idx = 0;
        let [w, h] = size;
        let total = total_samples.max(1);

        // Block dimension + per-submit sample chunk: bound each submit to ~`RAY_BUDGET`
        // *BVH-ray traversals* so its GPU time stays well under the watchdog/TDR, even on a
        // big scene. Crucially this must account for the rays cast PER sample: AO (`AO_RAYS`,
        // matching the shader) + a shadow ray are incoherent BVH traversals that dominate the
        // cost, so an AO+shadow submit does ~6× the work of a primary-only one — ignoring that
        // is what still lost the device on a large scene with AO/shadows on.
        const TILE: u32 = 256;
        const AO_RAYS: u32 = 4; // must match raytrace.wgsl
        const RAY_BUDGET: u32 = 2_000_000;
        let ao_on = base_uniform.ao[3] > 0.5;
        let shadow_on = base_uniform.shadow[2] > 0.5;
        let rays_per_sample = 1 + if ao_on { AO_RAYS } else { 0 } + u32::from(shadow_on);
        let chunk_cap = (RAY_BUDGET / (TILE * TILE * rays_per_sample)).max(1);

        let mut done = 0u32;
        while done < total {
            let reset = done == 0;
            let prior = self.total_samples;
            let chunk = chunk_cap.min(total - done);
            let read_idx = self.read_idx;
            let write_idx = 1 - read_idx;

            // One trace bind group per chunk (read ← read_idx, write → write_idx); the
            // per-tile origin rides the uniform, rewritten before each submit.
            let trace_bg = {
                let accums = self.accum.as_ref().unwrap();
                rs.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("rt-trace-bg-tiled"),
                    layout: &self.trace_bgl,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: self.uniform_buf.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: self.spheres.as_ref().unwrap().as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 2, resource: self.cylinders.as_ref().unwrap().as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 3, resource: self.mesh_verts.as_ref().unwrap().as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 4, resource: self.triangles.as_ref().unwrap().as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 5, resource: self.nodes.as_ref().unwrap().as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 6, resource: self.prim_indices.as_ref().unwrap().as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(&accums[write_idx].1) },
                        wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::TextureView(&accums[read_idx].1) },
                    ],
                })
            };

            let mut oy = 0u32;
            while oy < h {
                let th = TILE.min(h - oy);
                let mut ox = 0u32;
                while ox < w {
                    let tw = TILE.min(w - ox);
                    let mut u = base_uniform;
                    u.dims = [w, h, chunk, prior];
                    u.accum = [prior, u32::from(reset), ox, oy];
                    rs.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&u));

                    let mut encoder = rs.device.create_command_encoder(
                        &wgpu::CommandEncoderDescriptor { label: Some("rt-tile-encoder") },
                    );
                    {
                        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                            label: Some("rt-tile-pass"),
                            timestamp_writes: None,
                        });
                        pass.set_pipeline(&self.trace_pipeline);
                        pass.set_bind_group(0, &trace_bg, &[]);
                        pass.dispatch_workgroups(tw.div_ceil(8), th.div_ceil(8), 1);
                    }
                    rs.queue.submit(std::iter::once(encoder.finish()));
                    // Block until this submit finishes, so each command buffer's GPU time
                    // stays bounded (no pile-up) — fine on the offline file-render path.
                    let _ = rs.device.poll(wgpu::PollType::wait_indefinitely());
                    ox += TILE;
                }
                oy += TILE;
            }
            self.total_samples += chunk;
            self.read_idx = write_idx;
            done += chunk;
        }

        // Resolve the finished average (read_idx) into the target, once.
        let resolve_bg = {
            let accums = self.accum.as_ref().unwrap();
            rs.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rt-resolve-bg-tiled"),
                layout: &self.resolve_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.uniform_buf.as_entire_binding() },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(&accums[self.read_idx].1),
                    },
                ],
            })
        };
        let mut encoder = rs
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("rt-resolve-tiled") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rt-resolve-pass-tiled"),
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
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.resolve_pipeline);
            pass.set_bind_group(0, &resolve_bg, &[]);
            pass.draw(0..3, 0..1);
        }
        rs.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Samples accumulated into the current average (for the progressive in-place loop to
    /// know when it has converged).
    pub fn samples(&self) -> u32 {
        self.total_samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let c = GpuCylinder { c0: [0.0, 0.0, 0.0, 0.1], c1: [1.0, 0.0, 0.0, 0.0], m: [0, 0, 0, 0] };
        let a = cylinder_aabb(&c);
        assert!(a.max.y - a.min.y >= 0.19 && a.max.z - a.min.z >= 0.19);
        assert!(a.min.x <= -0.1 && a.max.x >= 1.1);
    }
}
