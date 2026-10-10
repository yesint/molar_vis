//! Per-rep union boundary: CPU builds a camera-independent BVH; fragment shaders
//! test analytic surface hits against it. No mesh extraction or camera rebuilds.
use super::GeometryData;
use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct Primitive {
    a: [f32; 4],    // xyz center/base, w radius
    b: [f32; 4],    // xyz far endpoint, w 0=sphere / 1=capsule
    lane: [f32; 4], // signed slot and gap; same view-dependent shift as cylinder.wgsl
    profile: [f32; 4], // cubic spline join positions/radii (zero for capsules)
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct Node {
    lo: [f32; 3],
    primitive: u32, // MAX = internal node
    hi: [f32; 3],
    escape: u32, // first node after this subtree (stackless traversal)
}
struct Tree {
    primitives: Vec<Primitive>,
    nodes: Vec<Node>,
    spheres: u32,
}
impl Tree {
    fn from_geometry(geom: &GeometryData) -> Self {
        let mut primitives: Vec<_> = geom
            .spheres
            .iter()
            .map(|s| Primitive {
                a: [s.center[0], s.center[1], s.center[2], s.radius],
                b: [s.center[0], s.center[1], s.center[2], 0.0],
                lane: [0.0; 4],
                profile: [0.0; 4],
            })
            .collect();
        primitives.extend(geom.cylinders.iter().map(|c| Primitive {
            a: [c.p0[0], c.p0[1], c.p0[2], c.radius],
            b: [c.p1[0], c.p1[1], c.p1[2], 1.0],
            lane: [c.offset[0], c.offset[1], 0.0, c.smoothing],
            profile: c.profile,
        }));
        Self {
            primitives,
            nodes: Vec::new(),
            spheres: geom.spheres.len() as u32,
        }
    }
    fn build(geom: &GeometryData) -> Self {
        let mut tree = Self::from_geometry(geom);
        let mut ids: Vec<_> = (0..tree.primitives.len() as u32).collect();
        if !ids.is_empty() {
            tree.subtree(&mut ids);
        }
        tree
    }
    fn refit(&mut self) {
        // The leaf ordering stays valid as coordinates move. Refit bottom-up in
        // linear time instead of sorting/building a fresh tree for each frame.
        for i in (0..self.nodes.len()).rev() {
            let (lo, hi) = if self.nodes[i].primitive != u32::MAX {
                self.bounds(self.nodes[i].primitive)
            } else {
                let left = self.nodes[i + 1];
                let right = self.nodes[left.escape as usize];
                (
                    Vec3::from_array(left.lo).min(Vec3::from_array(right.lo)),
                    Vec3::from_array(left.hi).max(Vec3::from_array(right.hi)),
                )
            };
            self.nodes[i].lo = lo.to_array();
            self.nodes[i].hi = hi.to_array();
        }
    }
    fn bounds(&self, id: u32) -> (Vec3, Vec3) {
        let p = self.primitives[id as usize];
        let a = Vec3::from_slice(&p.a[..3]);
        let b = Vec3::from_slice(&p.b[..3]);
        // Conservative for every camera orientation, including multi-order lanes.
        let r0 = p.profile[0].hypot(p.profile[1]);
        let r1 = (a.distance(b) - p.profile[2]).hypot(p.profile[3]);
        let flare = if p.profile[1] > 0.0 { r0.max(r1) + p.lane[3] * r0.min(r1) / 3.0 } else { 0.0 };
        let extent = Vec3::splat(p.a[3].max(flare) + (p.lane[0] * p.lane[1]).abs() + 1e-5);
        (a.min(b) - extent, a.max(b) + extent)
    }
    fn subtree(&mut self, ids: &mut [u32]) {
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for &id in ids.iter() {
            let (a, b) = self.bounds(id);
            lo = lo.min(a);
            hi = hi.max(b);
        }
        let index = self.nodes.len();
        self.nodes.push(Node {
            lo: lo.to_array(),
            hi: hi.to_array(),
            primitive: u32::MAX,
            escape: 0,
        });
        if ids.len() == 1 {
            self.nodes[index].primitive = ids[0];
        } else {
            let size = hi - lo;
            let axis = if size.x >= size.y && size.x >= size.z {
                0
            } else if size.y >= size.z {
                1
            } else {
                2
            };
            let mid = ids.len() / 2;
            ids.select_nth_unstable_by(mid, |a, b| {
                let center = |id| {
                    let (lo, hi) = self.bounds(id);
                    lo[axis] + hi[axis]
                };
                center(*a).total_cmp(&center(*b))
            });
            let (left, right) = ids.split_at_mut(mid);
            self.subtree(left);
            self.subtree(right);
        }
        self.nodes[index].escape = self.nodes.len() as u32;
    }
}

pub(super) struct Gpu {
    pub binding: wgpu::BindGroup,
    nodes: wgpu::Buffer,
    primitives: wgpu::Buffer,
    config: wgpu::Buffer,
    tree: Tree,
}
pub(super) fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("envelope-layout"),
        entries: &[
            entry(0, wgpu::BufferBindingType::Storage { read_only: true }),
            entry(1, wgpu::BufferBindingType::Storage { read_only: true }),
            entry(2, wgpu::BufferBindingType::Uniform),
        ],
    })
}
pub(super) fn needed(geom: &GeometryData) -> bool {
    geom.cylinders.iter().any(|c| c.color >> 24 < 255)
}
/// Keep oversized reps on the existing transparency path instead of exceeding a
/// device's storage binding limit (particularly the smaller WebGPU limits).
pub(super) fn fits(device: &wgpu::Device, geom: &GeometryData) -> bool {
    let count = (geom.spheres.len() + geom.cylinders.len()) as u64;
    let limit = device.limits().max_storage_buffer_binding_size as u64;
    count * std::mem::size_of::<Primitive>() as u64 <= limit
        && count.saturating_mul(2).saturating_sub(1) * std::mem::size_of::<Node>() as u64 <= limit
}
impl Gpu {
    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, geom: &GeometryData) -> Self {
        let tree = Tree::build(geom);
        let buffer = |label, bytes: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytes,
                usage: usage | wgpu::BufferUsages::COPY_DST,
            })
        };
        let empty_node = [Node::default()];
        let empty_primitive = [Primitive::default()];
        let nodes = buffer(
            "envelope-nodes",
            bytemuck::cast_slice(if tree.nodes.is_empty() {
                &empty_node
            } else {
                &tree.nodes
            }),
            wgpu::BufferUsages::STORAGE,
        );
        let primitives = buffer(
            "envelope-primitives",
            bytemuck::cast_slice(if tree.primitives.is_empty() {
                &empty_primitive
            } else {
                &tree.primitives
            }),
            wgpu::BufferUsages::STORAGE,
        );
        let config = buffer(
            "envelope-config",
            bytemuck::cast_slice(&[tree.nodes.len() as u32, tree.spheres, 0, 0]),
            wgpu::BufferUsages::UNIFORM,
        );
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("envelope"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: nodes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: primitives.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: config.as_entire_binding(),
                },
            ],
        });
        Self {
            binding,
            nodes,
            primitives,
            config,
            tree,
        }
    }
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        geom: &GeometryData,
    ) {
        let next = Tree::from_geometry(geom);
        if self.tree.primitives.len() != next.primitives.len() {
            *self = Self::new(device, layout, geom);
        } else {
            self.tree.primitives = next.primitives;
            self.tree.spheres = next.spheres;
            self.tree.refit();
            queue.write_buffer(&self.nodes, 0, bytemuck::cast_slice(&self.tree.nodes));
            queue.write_buffer(
                &self.primitives,
                0,
                bytemuck::cast_slice(&self.tree.primitives),
            );
            queue.write_buffer(
                &self.config,
                0,
                bytemuck::cast_slice(&[self.tree.nodes.len() as u32, self.tree.spheres, 0, 0]),
            );
        }
    }
}
pub(super) fn shader(source: &str, enabled: bool) -> String {
    source.replace(
        "// ENVELOPE",
        if enabled {
            include_str!("shaders/envelope.wgsl")
        } else {
            "fn envelope_hidden(p: vec3<f32>, id: u32, capsule: bool) -> bool { return false; }"
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{CylinderInstance, SphereInstance};

    fn geometry() -> GeometryData {
        GeometryData {
            spheres: vec![SphereInstance {
                center: [0.0; 3],
                radius: 0.2,
                color: 0x4d804020,
                mat: 0,
                pick: [0; 2],
            }],
            cylinders: vec![
                CylinderInstance {
                    p0: [0.0; 3],
                    p1: [1.0, 0.0, 0.0],
                    radius: 0.1,
                    color: 0x4d804020,
                    color1: 0x4d804020,
                    mat: 0,
                    offset: [0.0; 2],
                    profile: [0.0; 4],
                    smoothing: 0.0,
                    color_blend: 0.0,
                },
                CylinderInstance {
                    p0: [0.0; 3],
                    p1: [0.0, 1.0, 0.0],
                    radius: 0.1,
                    color: 0x4d804020,
                    color1: 0x4d804020,
                    mat: 0,
                    offset: [0.0; 2],
                    profile: [0.0; 4],
                    smoothing: 0.0,
                    color_blend: 0.0,
                },
            ],
            ..Default::default()
        }
    }
    #[test]
    fn tree_bounds_and_escape_links_cover_every_primitive() {
        let mut geom = geometry();
        for i in 0..1024 {
            let mut c = geom.cylinders[0];
            c.p0[0] = i as f32 * 0.3;
            c.p1[0] = c.p0[0] + 0.1;
            c.offset = [1.0, 0.04];
            geom.cylinders.push(c);
        }
        let tree = Tree::build(&geom);
        assert_eq!(tree.nodes.len(), 2 * tree.primitives.len() - 1);
        assert_eq!(tree.nodes[0].escape as usize, tree.nodes.len());
        let mut seen = vec![false; tree.primitives.len()];
        for (i, node) in tree.nodes.iter().enumerate() {
            assert!(node.escape as usize > i && node.escape as usize <= tree.nodes.len());
            if node.primitive != u32::MAX {
                assert!(!seen[node.primitive as usize]);
                seen[node.primitive as usize] = true;
                let (lo, hi) = tree.bounds(node.primitive);
                assert_eq!(lo.to_array(), node.lo);
                assert_eq!(hi.to_array(), node.hi);
            } else {
                for child in &tree.nodes[i + 1..node.escape as usize] {
                    assert!(Vec3::from_array(child.lo)
                        .cmpge(Vec3::from_array(node.lo))
                        .all());
                    assert!(Vec3::from_array(child.hi)
                        .cmple(Vec3::from_array(node.hi))
                        .all());
                }
            }
        }
        assert!(seen.into_iter().all(|b| b));
    }

    #[test]
    fn refit_tracks_trajectory_coordinates_without_rebuilding() {
        let mut geom = geometry();
        let mut tree = Tree::build(&geom);
        let root = tree.nodes[0];
        let shift = Vec3::new(0.3, -0.4, 0.5);
        for sphere in &mut geom.spheres {
            sphere.center = (Vec3::from_array(sphere.center) + shift).to_array();
        }
        for cylinder in &mut geom.cylinders {
            cylinder.p0 = (Vec3::from_array(cylinder.p0) + shift).to_array();
            cylinder.p1 = (Vec3::from_array(cylinder.p1) + shift).to_array();
        }
        tree.primitives = Tree::from_geometry(&geom).primitives;
        tree.refit();
        assert!(
            (Vec3::from_array(tree.nodes[0].lo) - Vec3::from_array(root.lo) - shift).length()
                < 1e-6
        );
        assert!(
            (Vec3::from_array(tree.nodes[0].hi) - Vec3::from_array(root.hi) - shift).length()
                < 1e-6
        );
        for node in &tree.nodes {
            if node.primitive != u32::MAX {
                let (lo, hi) = tree.bounds(node.primitive);
                assert_eq!(node.lo, lo.to_array());
                assert_eq!(node.hi, hi.to_array());
            }
        }
    }

    #[test]
    #[ignore = "requires native GPU; checks actual envelope shader containment"]
    fn gpu_envelope_removes_internal_faces_and_shared_caps() {
        let rs = crate::render::appearance_tests::gpu();
        let geom = geometry();
        for rotated in [false, true] {
            let view = if rotated {
                glam::Mat4::from_rotation_y(0.7)
                    * glam::Mat4::from_translation(Vec3::new(2.0, -1.0, 0.3))
            } else {
                glam::Mat4::IDENTITY
            };
            run_cases(
                &rs,
                &geom,
                view,
                &[
                    ([0.0, 0.1, 0.0], 0, true, true),
                    ([0.5, 0.0, 0.1], 0, true, false),
                    ([0.2, 0.0, 0.0], 0, false, true),
                    ([0.0, 0.0, 0.2], 0, false, false),
                ],
            );
            let mut shifted = geometry();
            shifted.cylinders.truncate(1);
            shifted.cylinders[0].radius = 0.05;
            shifted.cylinders[0].offset = [1.0, 0.08];
            let axis = view.transform_vector3(Vec3::X).normalize();
            let side = axis.cross(Vec3::Z).normalize();
            let point = Vec3::X * 0.2 + view.inverse().transform_vector3(side * 0.08);
            run_cases(&rs, &shifted, view, &[(point.to_array(), 0, false, true)]);
            let mut caps = geom.cylinders.clone();
            caps[0].radius = 0.2;
            caps[1].radius = 0.2;
            let joined = GeometryData {
                cylinders: caps,
                ..Default::default()
            };
            run_cases(
                &rs,
                &joined,
                view,
                &[
                    ([0.0, 0.0, 0.2], 0, true, false),
                    ([0.0, 0.0, 0.2], 1, true, true),
                    ([0.1, 0.0, 0.0], 0, true, true),
                ],
            );
        }
    }
    fn run_cases(
        rs: &eframe::egui_wgpu::RenderState,
        geom: &GeometryData,
        view: glam::Mat4,
        cases: &[([f32; 3], u32, bool, bool)],
    ) {
        let device = &rs.device;
        let source = format!("struct Camera {{ view: mat4x4<f32> }};\n\
            @group(0) @binding(0) var<uniform> camera: Camera;\n\
            @group(0) @binding(1) var<storage, read> cases: array<vec4<f32>>;\n\
            @group(0) @binding(2) var<storage, read_write> result: array<u32>;\n{}\n\
            @compute @workgroup_size(1) fn main(@builtin(global_invocation_id) i: vec3<u32>) {{\n\
                let c = cases[i.x]; let id = bitcast<u32>(c.w);\n\
                result[i.x] = select(0u, 1u, envelope_hidden((camera.view * vec4<f32>(c.xyz,1.0)).xyz, id >> 1u, (id & 1u) != 0u));\n\
            }}", format!("{}\n{}", include_str!("shaders/envelope.wgsl"), include_str!("shaders/bond_profile.wgsl")));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("envelope-test"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let gpu = Gpu::new(device, &pipeline.get_bind_group_layout(1), geom);
        let camera = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&view.to_cols_array()),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let inputs: Vec<_> = cases
            .iter()
            .map(|(p, id, capsule, _)| {
                [
                    p[0],
                    p[1],
                    p[2],
                    f32::from_bits((id << 1) | u32::from(*capsule)),
                ]
            })
            .collect();
        let inputs = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&inputs),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let size = (cases.len() * 4) as u64;
        let result = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: inputs.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: result.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &binding, &[]);
            pass.set_bind_group(1, &gpu.binding, &[]);
            pass.dispatch_workgroups(cases.len() as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&result, 0, &readback, 0, size);
        rs.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            tx.send(r).unwrap();
        });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let bytes = readback.slice(..).get_mapped_range();
        let actual: &[u32] = bytemuck::cast_slice(&bytes);
        for (i, case) in cases.iter().enumerate() {
            assert_eq!(actual[i] != 0, case.3, "case {i}: {case:?}");
        }
    }
}
