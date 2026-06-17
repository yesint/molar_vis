//! Screen-space ambient occlusion: the GPU uniform + bind-group layout + pipeline
//! for the fullscreen SSAO pass (see `shaders/ssao.wgsl`). The pass reads the
//! scene depth and multiply-blends an occlusion factor onto the color target.

use bytemuck::{Pod, Zeroable};
use glam::Mat4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct SsaoUniform {
    pub proj: [[f32; 4]; 4],
    pub inv_proj: [[f32; 4]; 4],
    /// View space → light clip space (`light_proj · light_view · inv_view`), for
    /// the deferred shadow test.
    pub shadow_matrix: [[f32; 4]; 4],
    /// `[radius, bias, strength, perspective(1/0)]`.
    pub params: [f32; 4],
    /// `[render_w, render_h, shadow_texel, _]` (SSAA target size in pixels; the
    /// `1/shadow_res` PCF texel step for the cast-shadow lookup).
    pub misc: [f32; 4],
    /// `[strength, bias, enabled, _]` for the cast-shadow test.
    pub shadow_params: [f32; 4],
}

impl SsaoUniform {
    /// `ao` is `[radius, bias, strength, enabled]`, `shadow` is `[strength, bias,
    /// enabled, _]` (enables handled by the caller / shader).
    pub fn new(
        proj: Mat4,
        perspective: bool,
        ao: [f32; 4],
        shadow_matrix: Mat4,
        shadow: [f32; 4],
        render_size: [u32; 2],
        shadow_texel: f32,
    ) -> Self {
        Self {
            proj: proj.to_cols_array_2d(),
            inv_proj: proj.inverse().to_cols_array_2d(),
            shadow_matrix: shadow_matrix.to_cols_array_2d(),
            params: [ao[0], ao[1], ao[2], if perspective { 1.0 } else { 0.0 }],
            misc: [render_size[0] as f32, render_size[1] as f32, shadow_texel, 0.0],
            shadow_params: shadow,
        }
    }
}

/// Bind group 0: the scene depth texture + the SSAO uniform + the shadow map
/// (depth) and its comparison sampler.
pub fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ssao-bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // Shadow map (depth) + comparison sampler.
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
        ],
    })
}

/// Fullscreen SSAO pipeline: outputs the AO factor with a **multiply** blend
/// (`result = dst × src`) so it darkens the existing color in crevices.
pub fn build_pipeline(
    device: &wgpu::Device,
    color_format: wgpu::TextureFormat,
    bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ssao-shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/ssao.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ssao-layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
    });
    let blend = wgpu::BlendState {
        // rgb = 0·src + dst·src = dst × ao
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::Src,
            operation: wgpu::BlendOperation::Add,
        },
        // keep the destination alpha unchanged
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ssao-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: Some(blend),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}
