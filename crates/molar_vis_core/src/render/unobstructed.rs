//! Native GPU scorer for unobstructed view. Independent of the ray tracer.
//!
//! A balanced sphere tree is built once per search. Its preorder escape links
//! allow stack-free traversal in WGSL. Node bounds only reject impossible
//! blockers; leaves apply the CPU scorer's projected-centre/depth rule.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use wgpu::util::DeviceExt;

const LEAF_SIZE: usize = 8;
const WORKGROUP_SIZE: u32 = 64;
const BATCH_SIZE: usize = 256;
const CORRECTION_CAPACITY: usize = 4096;
const SCORE_BYTES: u64 = (BATCH_SIZE * 4) as u64;
// The vec2 array starts at byte 8 after its atomic count (WGSL alignment).
const CORRECTION_BYTES: u64 = ((2 + CORRECTION_CAPACITY * 2) * 4) as u64;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct Node {
    lo: [f32; 4],
    hi: [f32; 4],
    // first atom index, atom count (0 for interior), escape node index, unused.
    links: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Direction {
    u: [f32; 4],
    v: [f32; 4],
    d: [f32; 4],
}

impl From<Vec3> for Direction {
    fn from(d: Vec3) -> Self {
        let (u, v) = crate::unobstructed::tangent_basis(d);
        Self {
            u: u.extend(0.0).to_array(),
            v: v.extend(0.0).to_array(),
            d: d.extend(0.0).to_array(),
        }
    }
}

fn build_tree(atoms: &[[f32; 4]]) -> (Vec<Node>, Vec<u32>) {
    fn split(atoms: &[[f32; 4]], ids: &mut [u32], first: usize, nodes: &mut Vec<Node>) {
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        let mut center_lo = lo;
        let mut center_hi = hi;
        for &id in ids.iter() {
            let a = atoms[id as usize];
            let p = Vec3::new(a[0], a[1], a[2]);
            lo = lo.min(p - Vec3::splat(a[3]));
            hi = hi.max(p + Vec3::splat(a[3]));
            center_lo = center_lo.min(p);
            center_hi = center_hi.max(p);
        }
        let node = nodes.len();
        nodes.push(Node {
            lo: lo.extend(0.0).to_array(),
            hi: hi.extend(0.0).to_array(),
            links: [first as u32, ids.len() as u32, 0, 0],
        });
        if ids.len() > LEAF_SIZE {
            let extent = center_hi - center_lo;
            let axis = if extent.x >= extent.y && extent.x >= extent.z {
                0
            } else if extent.y >= extent.z {
                1
            } else {
                2
            };
            let middle = ids.len() / 2;
            ids.select_nth_unstable_by(middle, |&a, &b| {
                atoms[a as usize][axis]
                    .total_cmp(&atoms[b as usize][axis])
                    .then(a.cmp(&b))
            });
            let (left, right) = ids.split_at_mut(middle);
            split(atoms, left, first, nodes);
            split(atoms, right, first + middle, nodes);
            nodes[node].links[1] = 0;
        }
        nodes[node].links[2] = nodes.len() as u32;
    }
    let mut order: Vec<_> = (0..atoms.len() as u32).collect();
    let mut nodes = Vec::new();
    if !order.is_empty() {
        split(atoms, &mut order, 0, &mut nodes);
    }
    (nodes, order)
}

/// Resolve boundary queries with CPU arithmetic using the same conservative tree.
/// This avoids a full atom scan for each correction on large molecules.
fn boundary_visible(atoms: &[[f32; 4]], nodes: &[Node], order: &[u32], id: usize, d: Vec3) -> bool {
    let p = Vec3::from_slice(&atoms[id][..3]);
    let (u, v) = crate::unobstructed::tangent_basis(d);
    let (ui, vi, di) = (p.dot(u), p.dot(v), p.dot(d));
    let mut i = 0;
    while i < nodes.len() {
        let node = &nodes[i];
        let lo = Vec3::from_slice(&node.lo[..3]);
        let hi = Vec3::from_slice(&node.hi[..3]);
        let center = (lo + hi) * 0.5;
        let extent = (hi - lo) * 0.5;
        let (cu, cv, cd) = (center.dot(u), center.dot(v), center.dot(d));
        let (eu, ev, ed) = (
            extent.dot(u.abs()),
            extent.dot(v.abs()),
            extent.dot(d.abs()),
        );
        let pad = 1e-5 * (1.0 + ui.abs() + vi.abs() + di.abs() + eu + ev + ed);
        if ui < cu - eu - pad
            || ui > cu + eu + pad
            || vi < cv - ev - pad
            || vi > cv + ev + pad
            || cd + ed + pad <= di + 1e-4
        {
            i = node.links[2] as usize;
            continue;
        }
        if node.links[1] == 0 {
            i += 1;
            continue;
        }
        for &j in &order[node.links[0] as usize..(node.links[0] + node.links[1]) as usize] {
            let atom = atoms[j as usize];
            let q = Vec3::from_slice(&atom[..3]);
            if q.dot(d) > di + 1e-4 {
                let du = ui - q.dot(u);
                let dv = vi - q.dot(v);
                if du * du + dv * dv < atom[3] * atom[3] {
                    return false;
                }
            }
        }
        i = node.links[2] as usize;
    }
    true
}

pub(super) struct UnobstructedGpu {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

impl UnobstructedGpu {
    pub(super) fn new(device: &wgpu::Device) -> Option<Self> {
        let limits = device.limits();
        if limits.max_storage_buffers_per_shader_stage < 6
            || limits.max_compute_workgroup_size_x < WORKGROUP_SIZE
            || limits.max_compute_invocations_per_workgroup < WORKGROUP_SIZE
            || limits.max_compute_workgroup_storage_size < WORKGROUP_SIZE * 4
            || u64::from(limits.max_storage_buffer_binding_size) < CORRECTION_BYTES
            || limits.max_buffer_size < SCORE_BYTES + CORRECTION_BYTES
        {
            return None;
        }
        let mut entries: Vec<_> = (0..5)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage {
                        read_only: binding != 4,
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 5,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 6,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("unobstructed-layout"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("unobstructed-pipeline-layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("unobstructed-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/unobstructed.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("unobstructed-compute"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("score"),
            compilation_options: Default::default(),
            cache: None,
        });
        Some(Self { layout, pipeline })
    }

    pub(super) fn best_direction(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &[(Vec3, f32)],
        occluders: &[(Vec3, f32)],
        resolution: usize,
    ) -> Result<Vec3, String> {
        if target.is_empty() {
            return Ok(Vec3::Z);
        }
        let scene = ScoringScene::new(device, target, occluders)?;
        crate::unobstructed::search_directions(resolution, |dirs| {
            self.scores(device, queue, &scene, dirs)
        })
    }

    fn scores(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &ScoringScene,
        dirs: &[Vec3],
    ) -> Result<Vec<u32>, String> {
        let batch_size =
            BATCH_SIZE.min(device.limits().max_compute_workgroups_per_dimension as usize);
        let mut result = Vec::with_capacity(dirs.len());
        for batch in dirs.chunks(batch_size) {
            let directions: Vec<Direction> = batch.iter().copied().map(Into::into).collect();
            queue.write_buffer(&scene.directions, 0, bytemuck::cast_slice(&directions));
            let buffers = [
                &scene.atoms,
                &scene.nodes,
                &scene.order,
                &scene.directions,
                &scene.scores,
                &scene.params,
                &scene.corrections,
            ];
            let entries: Vec<_> = buffers
                .iter()
                .enumerate()
                .map(|(binding, buffer)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: buffer.as_entire_binding(),
                })
                .collect();
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("unobstructed-bind-group"),
                layout: &self.layout,
                entries: &entries,
            });
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("unobstructed-score-batch"),
            });
            encoder.clear_buffer(&scene.scores, 0, None);
            encoder.clear_buffer(&scene.corrections, 0, None);
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("unobstructed-score"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.dispatch_workgroups(
                    scene.target_count.div_ceil(WORKGROUP_SIZE),
                    batch.len() as u32,
                    1,
                );
            }
            let size = (batch.len() * 4) as u64;
            encoder.copy_buffer_to_buffer(&scene.scores, 0, &scene.readback, 0, size);
            encoder.copy_buffer_to_buffer(
                &scene.corrections,
                0,
                &scene.readback,
                SCORE_BYTES,
                CORRECTION_BYTES,
            );
            let submission = queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            scene
                .readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    let _ = tx.send(r);
                });
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: None,
                })
                .map_err(|e| format!("GPU score poll: {e}"))?;
            rx.recv()
                .map_err(|e| format!("GPU score readback: {e}"))?
                .map_err(|e| format!("GPU score map: {e}"))?;
            {
                let data: Vec<u32> = {
                    let mapped = scene.readback.slice(..).get_mapped_range();
                    bytemuck::cast_slice(&mapped).to_vec()
                };
                scene.readback.unmap();
                let count = data[BATCH_SIZE] as usize;
                if count > CORRECTION_CAPACITY {
                    // Rare degenerate scenes may exceed the bounded correction list.
                    // The caller uses the complete CPU search rather than lose queries.
                    return Err("too many queries near floating-point boundaries".into());
                }
                let mut scores = data[..batch.len()].to_vec();
                for record in data[BATCH_SIZE + 2..BATCH_SIZE + 2 + count * 2].chunks_exact(2) {
                    let direction = record[0] as usize;
                    let atom = record[1] as usize;
                    if !boundary_visible(
                        &scene.cpu_atoms,
                        &scene.cpu_nodes,
                        &scene.cpu_order,
                        atom,
                        batch[direction],
                    ) {
                        scores[direction] -= 1;
                    }
                }
                log::debug!("unobstructed GPU batch: {count} boundary queries checked on CPU");
                result.extend(scores);
            }
        }
        Ok(result)
    }
}

struct ScoringScene {
    cpu_atoms: Vec<[f32; 4]>,
    cpu_nodes: Vec<Node>,
    cpu_order: Vec<u32>,
    corrections: wgpu::Buffer,
    atoms: wgpu::Buffer,
    nodes: wgpu::Buffer,
    order: wgpu::Buffer,
    directions: wgpu::Buffer,
    scores: wgpu::Buffer,
    params: wgpu::Buffer,
    readback: wgpu::Buffer,
    target_count: u32,
}

impl ScoringScene {
    fn new(
        device: &wgpu::Device,
        target: &[(Vec3, f32)],
        occluders: &[(Vec3, f32)],
    ) -> Result<Self, String> {
        let count = target
            .len()
            .checked_add(occluders.len())
            .ok_or("too many atoms")?;
        let limits = device.limits();
        // The tree has fewer than 2*N nodes; reject oversized searches before allocating.
        let binding_limit =
            u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size);
        if count == 0
            || count > u32::MAX as usize
            || count as u64 * 2 * size_of::<Node>() as u64 > binding_limit
            || target.len().div_ceil(WORKGROUP_SIZE as usize)
                > limits.max_compute_workgroups_per_dimension as usize
        {
            return Err("unobstructed scene exceeds GPU buffer or dispatch limits".into());
        }
        let atoms: Vec<[f32; 4]> = target
            .iter()
            .chain(occluders)
            .map(|&(p, r)| p.extend(r).to_array())
            .collect();
        if atoms
            .iter()
            .any(|a| a.iter().any(|v| !v.is_finite()) || a[3] < 0.0)
        {
            return Err("unobstructed scene has invalid positions or radii".into());
        }
        let (nodes, order) = build_tree(&atoms);
        let upload = |label, contents| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::STORAGE,
            })
        };
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        Ok(Self {
            corrections: buffer(
                "unobstructed-corrections",
                CORRECTION_BYTES,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            ),
            atoms: upload("unobstructed-atoms", bytemuck::cast_slice(&atoms)),
            nodes: upload("unobstructed-nodes", bytemuck::cast_slice(&nodes)),
            order: upload("unobstructed-order", bytemuck::cast_slice(&order)),
            directions: buffer(
                "unobstructed-directions",
                (BATCH_SIZE * size_of::<Direction>()) as u64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            scores: buffer(
                "unobstructed-scores",
                (BATCH_SIZE * 4) as u64,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            ),
            params: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("unobstructed-params"),
                contents: bytemuck::cast_slice(&[target.len() as u32, nodes.len() as u32, 0, 0]),
                usage: wgpu::BufferUsages::UNIFORM,
            }),
            readback: buffer(
                "unobstructed-readback",
                SCORE_BYTES + CORRECTION_BYTES,
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            ),
            cpu_atoms: atoms,
            cpu_nodes: nodes,
            cpu_order: order,
            target_count: target.len() as u32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cloud(n: usize) -> Vec<(Vec3, f32)> {
        let mut seed = 17u64;
        let mut rand = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 40) as f32 / (1u32 << 24) as f32
        };
        (0..n)
            .map(|_| {
                (
                    Vec3::new(rand(), rand(), rand()) * 4.0 - Vec3::ONE,
                    0.05 + rand() * 0.3,
                )
            })
            .collect()
    }

    #[test]
    fn tree_leaves_cover_atoms_once_and_bounds_enclose_every_sphere() {
        for mut atoms in [
            Vec::new(),
            cloud(1),
            cloud(65),
            cloud(4096),
            vec![(Vec3::ZERO, 0.2); 4096],
        ] {
            // Include disparate radii, negative and far-away coordinates.
            if !atoms.is_empty() {
                atoms.push((Vec3::new(-500.0, 1000.0, 2000.0), 3.0));
            }
            let atoms: Vec<_> = atoms.iter().map(|&(p, r)| p.extend(r).to_array()).collect();
            let (nodes, order) = build_tree(&atoms);
            let mut seen = vec![0; atoms.len()];
            for (i, node) in nodes.iter().enumerate() {
                assert!(node.links[2] as usize > i && node.links[2] as usize <= nodes.len());
                let lo = Vec3::from_slice(&node.lo[..3]);
                let hi = Vec3::from_slice(&node.hi[..3]);
                if node.links[1] != 0 {
                    for &id in
                        &order[node.links[0] as usize..(node.links[0] + node.links[1]) as usize]
                    {
                        let atom = atoms[id as usize];
                        let center = Vec3::from_slice(&atom[..3]);
                        assert!(lo.cmple(center - Vec3::splat(atom[3])).all());
                        assert!(hi.cmpge(center + Vec3::splat(atom[3])).all());
                        seen[id as usize] += 1;
                    }
                } else {
                    let left = &nodes[i + 1];
                    let right = &nodes[left.links[2] as usize];
                    for child in [left, right] {
                        assert!(lo.cmple(Vec3::from_slice(&child.lo[..3])).all());
                        assert!(hi.cmpge(Vec3::from_slice(&child.hi[..3])).all());
                    }
                    assert_eq!(right.links[2], node.links[2]);
                }
            }
            assert!(seen.iter().all(|&count| count == 1));
        }
    }

    #[test]
    fn compute_shader_validates_without_a_gpu() {
        let module =
            wgpu::naga::front::wgsl::parse_str(include_str!("shaders/unobstructed.wgsl")).unwrap();
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn cpu_boundary_tree_matches_the_grid_scorer() {
        for offset in [Vec3::ZERO, Vec3::new(-500.0, 1000.0, 2000.0)] {
            let atoms: Vec<_> = cloud(257)
                .iter()
                .map(|&(p, r)| (p + offset).extend(r).to_array())
                .collect();
            let (nodes, order) = build_tree(&atoms);
            let target: Vec<_> = atoms
                .iter()
                .map(|a| (Vec3::from_slice(&a[..3]), a[3]))
                .collect();
            for d in crate::unobstructed::fibonacci_sphere(32)
                .into_iter()
                .chain([Vec3::X, Vec3::Y, Vec3::Z])
            {
                let count = (0..atoms.len())
                    .filter(|&id| boundary_visible(&atoms, &nodes, &order, id, d))
                    .count() as u32;
                assert_eq!(count, crate::unobstructed::visible_count(&target, &[], d));
            }
        }
    }

    fn gpu() -> (wgpu::Device, wgpu::Queue) {
        pollster::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    ..Default::default()
                })
                .await
                .expect("a compute-capable adapter is required for this explicit GPU test");
            eprintln!("unobstructed GPU test adapter: {:?}", adapter.get_info());
            adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .unwrap()
        })
    }

    #[test]
    #[ignore = "requires a native compute-capable GPU; run explicitly"]
    fn gpu_scores_match_cpu_including_partial_workgroups_and_direction_batches() {
        let (device, queue) = gpu();
        let scorer = UnobstructedGpu::new(&device).unwrap();
        let directions = [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
            Vec3::new(0.3, -0.7, 0.65).normalize(),
        ];
        for (t, o) in [(1, 0), (2, 1), (65, 129), (1050, 200), (32, 1200)] {
            let atoms = cloud(t + o);
            let (target, occluders) = atoms.split_at(t);
            let scene = ScoringScene::new(&device, target, occluders).unwrap();
            let scores = scorer.scores(&device, &queue, &scene, &directions).unwrap();
            let expected: Vec<_> = directions
                .iter()
                .map(|&d| crate::unobstructed::visible_count(target, occluders, d))
                .collect();
            assert_eq!(scores, expected, "t={t}, o={o}");
            assert_eq!(
                scorer
                    .best_direction(&device, &queue, target, occluders, 32)
                    .unwrap(),
                crate::unobstructed::best_unobstructed_direction(target, occluders, 32)
            );
            if t == 65 {
                let dirs: Vec<_> = directions.into_iter().cycle().take(300).collect();
                let scores = scorer.scores(&device, &queue, &scene, &dirs).unwrap();
                for (i, &s) in scores.iter().enumerate() {
                    assert_eq!(s, expected[i % expected.len()]);
                }
            }
        }
        // The exact depth epsilon and projected-radius boundary matter.
        let target = [
            (Vec3::ZERO, 0.2),
            (Vec3::ZERO, 0.1),
            (Vec3::new(0.2, 0.0, 0.0), 0.1),
        ];
        for depth in [0.0, 1e-4, 1.01e-4, 0.1] {
            let occluders = [(Vec3::new(0.0, 0.0, depth), 0.2)];
            let scene = ScoringScene::new(&device, &target, &occluders).unwrap();
            assert_eq!(
                scorer.scores(&device, &queue, &scene, &[Vec3::Z]).unwrap(),
                vec![crate::unobstructed::visible_count(
                    &target,
                    &occluders,
                    Vec3::Z
                )]
            );
        }
        // A degenerate boundary-heavy scene must request CPU fallback, then leave
        // the readback buffer reusable rather than silently dropping corrections.
        let target = vec![(Vec3::ZERO, 0.2); CORRECTION_CAPACITY + 1];
        let occluders = [(Vec3::new(0.2, 0.0, 0.1), 0.2)];
        let scene = ScoringScene::new(&device, &target, &occluders).unwrap();
        assert!(scorer
            .scores(&device, &queue, &scene, &[Vec3::Z])
            .unwrap_err()
            .contains("boundaries"));
        assert_eq!(
            scorer.scores(&device, &queue, &scene, &[Vec3::X]).unwrap(),
            vec![0]
        );
        assert_eq!(
            scorer
                .best_direction(&device, &queue, &[], &[], 256)
                .unwrap(),
            Vec3::Z
        );
        assert!(ScoringScene::new(&device, &[(Vec3::splat(f32::NAN), 0.1)], &[]).is_err());
    }
    #[test]
    #[ignore = "requires a native GPU; validates scores and benchmarks bundled molecules"]
    fn molecular_gpu_scores_and_best_direction_match_cpu() {
        use molar::prelude::*;
        let (device, queue) = gpu();
        let scorer = UnobstructedGpu::new(&device).unwrap();
        for name in ["2lao.pdb", "cg.pdb"] {
            let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests"))
                .join(name);
            let raw = crate::data::load(&path).unwrap();
            let bound = raw.system.select_all_bound();
            let atoms: Vec<_> = bound
                .iter_particle()
                .map(|p| (Vec3::new(p.pos.x, p.pos.y, p.pos.z), p.atom.vdw()))
                .collect();
            let dirs = crate::unobstructed::fibonacci_sphere(64);
            let scene = ScoringScene::new(&device, &atoms, &[]).unwrap();
            let scores = scorer.scores(&device, &queue, &scene, &dirs).unwrap();
            for (&d, &score) in dirs.iter().zip(&scores) {
                assert_eq!(
                    score,
                    crate::unobstructed::visible_count(&atoms, &[], d),
                    "{name}, direction {d:?}"
                );
            }
            let start = std::time::Instant::now();
            let cpu = crate::unobstructed::best_unobstructed_direction(&atoms, &[], 256);
            let cpu_ms = start.elapsed().as_secs_f64() * 1000.0;
            let start = std::time::Instant::now();
            let gpu = scorer
                .best_direction(&device, &queue, &atoms, &[], 256)
                .unwrap();
            let gpu_ms = start.elapsed().as_secs_f64() * 1000.0;
            eprintln!(
                "{name}: {} atoms, CPU {cpu_ms:.1} ms, GPU {gpu_ms:.1} ms",
                atoms.len()
            );
            assert_eq!(cpu, gpu, "{name}: search directions must match");
        }
    }
}
