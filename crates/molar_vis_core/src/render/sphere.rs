//! Sphere impostor: one instance per atom, drawn as a camera-facing billboard
//! whose fragment shader ray-casts the sphere and writes analytic depth. Serves
//! VDW now, and (later) licorice caps and ball-and-stick balls.

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct SphereInstance {
    /// World-space center (nm).
    pub center: [f32; 3],
    /// Sphere radius (nm) — VDW radius pre-multiplied by any per-rep scale.
    pub radius: f32,
    /// RGBA8 packed little-endian; alpha carries the material opacity.
    pub color: u32,
    /// Packed material lighting (ambient|diffuse<<8|specular<<16|shininess<<24).
    pub mat: u32,
}

impl SphereInstance {
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<SphereInstance>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &[
            // center: vec3<f32> @location(0)
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x3,
            },
            // radius: f32 @location(1)
            wgpu::VertexAttribute {
                offset: 12,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32,
            },
            // color: u32 @location(2)
            wgpu::VertexAttribute {
                offset: 16,
                shader_location: 2,
                format: wgpu::VertexFormat::Uint32,
            },
            // mat: u32 @location(3)
            wgpu::VertexAttribute {
                offset: 20,
                shader_location: 3,
                format: wgpu::VertexFormat::Uint32,
            },
        ],
    };
}

/// Build the sphere impostor pipeline. `camera_bgl` is bind group 0 (the camera
/// uniform); the billboard quad vertices are generated from `vertex_index`, so
/// the only vertex buffer is the per-instance buffer. `targets`/`depth_write`/
/// `fs_entry` are supplied by the caller so the same geometry serves both the
/// opaque pass (single color target, depth-write on, `fs_main`) and the
/// weighted-blended OIT pass (accum+reveal targets, depth-write off, `fs_oit`).
pub fn build_pipeline(
    device: &wgpu::Device,
    depth_format: wgpu::TextureFormat,
    camera_bgl: &wgpu::BindGroupLayout,
    targets: &[Option<wgpu::ColorTargetState>],
    depth_write: bool,
    fs_entry: &str,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sphere-shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sphere.wgsl").into()),
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sphere-pipeline-layout"),
        bind_group_layouts: &[Some(camera_bgl)],
        immediate_size: 0,
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sphere-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[SphereInstance::LAYOUT],
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some(fs_entry),
            targets,
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}
