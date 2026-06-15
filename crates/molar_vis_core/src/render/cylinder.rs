//! Cylinder impostor: one instance per half-bond, drawn as a camera-facing
//! billboard whose fragment shader ray-casts a finite (capless) cylinder and
//! writes analytic depth. Caps are covered by the sphere rep at the joints.

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct CylinderInstance {
    /// Start point, world space (nm).
    pub p0: [f32; 3],
    pub radius: f32,
    /// End point, world space (nm).
    pub p1: [f32; 3],
    /// RGBA8 packed; alpha carries the material opacity.
    pub color: u32,
    /// Packed material lighting (ambient|diffuse<<8|specular<<16|shininess<<24).
    pub mat: u32,
}

impl CylinderInstance {
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<CylinderInstance>() as wgpu::BufferAddress,
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
                format: wgpu::VertexFormat::Float32x3,
            },
            wgpu::VertexAttribute {
                offset: 28,
                shader_location: 3,
                format: wgpu::VertexFormat::Uint32,
            },
            // mat: u32 @location(4)
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
    depth_format: wgpu::TextureFormat,
    camera_bgl: &wgpu::BindGroupLayout,
    targets: &[Option<wgpu::ColorTargetState>],
    depth_write: bool,
    depth_compare: wgpu::CompareFunction,
    fs_entry: &str,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("cylinder-shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/cylinder.wgsl").into()),
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("cylinder-pipeline-layout"),
        bind_group_layouts: &[Some(camera_bgl)],
        immediate_size: 0,
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("cylinder-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[CylinderInstance::LAYOUT],
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
