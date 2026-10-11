//! Portable screen-space point discs, expanded into instanced quads.
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct PointInstance {
    pub pos: [f32; 3],
    /// Diameter in pixels, independent of camera zoom.
    pub size: f32,
    pub color: u32,
}
impl PointInstance {
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &[
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x3,
            },
            wgpu::VertexAttribute {
                offset: 12,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32,
            },
            wgpu::VertexAttribute {
                offset: 16,
                shader_location: 2,
                format: wgpu::VertexFormat::Uint32,
            },
        ],
    };
}

pub fn build_pipeline(
    device: &wgpu::Device,
    depth_format: wgpu::TextureFormat,
    camera_bgl: &wgpu::BindGroupLayout,
    targets: &[Option<wgpu::ColorTargetState>],
    depth_write: bool,
    depth_compare: wgpu::CompareFunction,
    fs_entry: &str,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("point-shader"),
        source: wgpu::ShaderSource::Wgsl(
            super::lit_shader_source(include_str!("shaders/point.wgsl")).into(),
        ),
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("point-pipepoint-layout"),
        bind_group_layouts: &[Some(camera_bgl)],
        immediate_size: 0,
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("point-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[PointInstance::LAYOUT],
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState {
            // Each point is one instance drawn as a 4-vertex quad (two triangles).
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(depth_compare),
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
