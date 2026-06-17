//! Reflective ground plane. The scene is rendered a second time mirrored across a
//! view-space horizontal plane below the molecule (into a reflection color target),
//! then this pass draws the floor quad — a large plane at `y = floor_y` in view
//! space — sampling that reflection by screen position, tinted by `reflectivity`
//! and faded toward the horizon. The floor is depth-tested + depth-writing, so the
//! molecule occludes it and it receives the deferred AO / cast-shadow pass.

use bytemuck::{Pod, Zeroable};
use glam::Mat4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct FloorUniform {
    pub proj: [[f32; 4]; 4],
    /// `[floor_y, half_extent, z_near, z_far]` — the quad corners in view space.
    pub params: [f32; 4],
    /// `[reflectivity, fade_start, fade_end, _]` (fade in view-space distance).
    pub params2: [f32; 4],
    /// `[render_w, render_h, _, _]`.
    pub dims: [f32; 4],
    /// Floor base color (mixed with the reflection by `reflectivity`).
    pub base: [f32; 4],
}

impl FloorUniform {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        proj: Mat4,
        floor_y: f32,
        half_extent: f32,
        z_near: f32,
        z_far: f32,
        reflectivity: f32,
        fade_start: f32,
        fade_end: f32,
        render_size: [u32; 2],
        base: [f32; 4],
    ) -> Self {
        Self {
            proj: proj.to_cols_array_2d(),
            params: [floor_y, half_extent, z_near, z_far],
            params2: [reflectivity, fade_start, fade_end, 0.0],
            dims: [render_size[0] as f32, render_size[1] as f32, 0.0, 0.0],
            base,
        }
    }
}

/// Bind group: the floor uniform + the reflection color texture + a linear sampler.
pub fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("floor-bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

pub fn build_pipeline(
    device: &wgpu::Device,
    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("floor-shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/floor.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("floor-layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("floor-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
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
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}
