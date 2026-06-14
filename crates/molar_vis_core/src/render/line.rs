//! Screen-space fat lines for the Lines representation. Each bond contributes two
//! half-segments split at the midpoint and colored by their endpoint atom.
//!
//! WebGPU/wgpu can only rasterize 1‑px `LineList` primitives, so a settable line
//! width is done the portable way: each segment (a *pair* of [`LineVertex`]es) is
//! drawn as an **instanced quad** that is expanded perpendicular to the segment in
//! screen space by `width` pixels in the vertex shader. The vertex buffer is the
//! same flat pair-list the builder already produces — it is simply reinterpreted
//! as per-instance data with a stride of two vertices (the shader reads both
//! endpoints), so the width stays constant in pixels at any zoom, like VMD.

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct LineVertex {
    /// World-space position (nm).
    pub pos: [f32; 3],
    /// RGBA8 packed.
    pub color: u32,
    /// Line width in pixels (screen-space). Equal for both endpoints of a segment.
    pub width: f32,
}

impl LineVertex {
    /// One segment per instance: stride = two vertices; the shader reads endpoint 0
    /// (offset 0) and endpoint 1 (offset `size_of::<LineVertex>()`) to expand the quad.
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: 2 * std::mem::size_of::<LineVertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &[
            // Endpoint 0.
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x3,
            },
            wgpu::VertexAttribute {
                offset: 12,
                shader_location: 1,
                format: wgpu::VertexFormat::Uint32,
            },
            wgpu::VertexAttribute {
                offset: 16,
                shader_location: 2,
                format: wgpu::VertexFormat::Float32,
            },
            // Endpoint 1 (next vertex in the pair).
            wgpu::VertexAttribute {
                offset: 20,
                shader_location: 3,
                format: wgpu::VertexFormat::Float32x3,
            },
            wgpu::VertexAttribute {
                offset: 32,
                shader_location: 4,
                format: wgpu::VertexFormat::Uint32,
            },
        ],
    };
}

pub fn build_pipeline(
    device: &wgpu::Device,
    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    camera_bgl: &wgpu::BindGroupLayout,
    transparent: bool,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("line-shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/line.wgsl").into()),
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("line-pipeline-layout"),
        bind_group_layouts: &[Some(camera_bgl)],
        immediate_size: 0,
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("line-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[LineVertex::LAYOUT],
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState {
            // Each segment is one instance drawn as a 4-vertex quad (two triangles).
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: Some(!transparent),
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
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}
