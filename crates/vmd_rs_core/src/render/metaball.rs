//! Metaball representation GPU resources: a compute pass bakes a density+color
//! volume from the atoms, and a full-screen pass ray-marches it (see the two
//! WGSL shaders). Requires compute + storage buffers — i.e. a WebGPU/native
//! device (not a WebGL2 downlevel device).

use bytemuck::{Pod, Zeroable};

/// One atom in the metaball density field (storage-buffer element, std430).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct MetaballAtom {
    /// xyz = center (nm), w = kernel radius R (VDW · radius_scale).
    pub center_radius: [f32; 4],
    /// rgb in 0..1; w unused.
    pub color: [f32; 4],
}

/// Grid + kernel parameters, shared by the bake and render passes (uniform).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct MetaballUniform {
    /// xyz = volume min corner (nm), w = voxel edge.
    pub origin: [f32; 4],
    /// xyz = grid dims, w = atom count.
    pub dims: [u32; 4],
    /// x = isovalue, y = K (kernel sharpness), z = cutoff (×R), w = unused.
    pub params: [f32; 4],
}

/// Kernel sharpness `K` in `exp(-K·d²/R²)`. Must agree with the cutoff used when
/// sizing the grid (`geometry::METABALL_CUTOFF`).
pub const METABALL_K: f32 = 2.5;
pub const METABALL_CUTOFF: f32 = 2.5;

/// Compute pipeline that bakes the density volume, plus its bind-group layout
/// (binding 0 = uniform, 1 = atoms [read], 2 = volume [read_write]).
pub fn build_bake_pipeline(
    device: &wgpu::Device,
) -> (wgpu::ComputePipeline, wgpu::BindGroupLayout) {
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("metaball-bake-bgl"),
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
            storage_entry(1, wgpu::ShaderStages::COMPUTE, true),
            storage_entry(2, wgpu::ShaderStages::COMPUTE, false),
        ],
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("metaball-bake-layout"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("metaball-bake-shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/metaball_bake.wgsl").into()),
    });

    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("metaball-bake-pipeline"),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });

    (pipeline, bgl)
}

/// Full-screen ray-march render pipeline, plus its group-1 bind-group layout
/// (binding 0 = uniform, 1 = volume [read]). Group 0 is the shared camera UBO.
pub fn build_render_pipeline(
    device: &wgpu::Device,
    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    camera_bgl: &wgpu::BindGroupLayout,
) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("metaball-render-bgl"),
        entries: &[
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
            storage_entry(1, wgpu::ShaderStages::FRAGMENT, true),
        ],
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("metaball-render-layout"),
        bind_group_layouts: &[Some(camera_bgl), Some(&bgl)],
        immediate_size: 0,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("metaball-shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/metaball.wgsl").into()),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("metaball-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
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

    (pipeline, bgl)
}

fn storage_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    read_only: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
