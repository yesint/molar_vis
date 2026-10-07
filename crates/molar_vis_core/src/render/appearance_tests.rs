//! Appearance regressions compare actual raster and ray-traced pixels.
use super::*;
use crate::camera::{BgKind, Camera, Projection};

#[test]
fn shared_shaders_validate_without_a_gpu() {
    for (name, source) in [
        ("sphere", include_str!("shaders/sphere.wgsl")),
        ("cylinder", include_str!("shaders/cylinder.wgsl")),
        ("mesh", include_str!("shaders/mesh.wgsl")),
        ("line", include_str!("shaders/line.wgsl")),
        ("ssao", include_str!("shaders/ssao.wgsl")),
        ("raytrace", include_str!("shaders/raytrace.wgsl")),
    ] {
        let source = lit_shader_source(source);
        let module = wgpu::naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&source)));
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn trace_effects_keep_world_units_and_the_raster_light_frame() {
    for radius in [0.1, 5.0, 50.0] {
        let mut camera = Camera::frame_bbox(Vec3::splat(-radius), Vec3::splat(radius), 0.8);
        camera.orientation = glam::Quat::from_rotation_y(0.7);
        let u = SceneRenderer::rt_uniform(&camera, 320, 240, 64, 0, 0.0, 2048);
        assert_eq!(
            u.ao,
            camera.ao_uniform(),
            "AO must not change with scene size"
        );
        let shadow_matrix = Mat4::from_cols_array_2d(&u.shadow_matrix);
        let direction = Vec3::from_slice(&u.light_dir[..3]);
        assert!((direction.length() - 1.0).abs() < 1e-6);
        assert_eq!(
            u.shadow[1], 0.0002,
            "ray bias must not grow with scene size"
        );
        let step = shadow_matrix.transform_vector3(Vec3::from_slice(&u.shadow_u[..3]));
        assert!((step.x - 2.0 / 2048.0).abs() < 1e-6);
        assert!(step.y.abs() < 1e-6 && step.z.abs() < 1e-6);
    }
}

fn gpu() -> RenderState {
    use std::sync::Arc;
    pollster::block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .unwrap();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .unwrap();
        let target_format = wgpu::TextureFormat::Rgba8Unorm;
        let renderer = Arc::new(egui::mutex::RwLock::new(egui_wgpu::Renderer::new(
            &device,
            target_format,
            egui_wgpu::RendererOptions::default(),
        )));
        RenderState {
            available_adapters: vec![adapter.clone()],
            adapter,
            device,
            queue,
            target_format,
            renderer,
        }
    })
}

fn raster_and_trace(
    renderer: &mut SceneRenderer,
    rs: &RenderState,
    scene: &Scene,
    camera: &Camera,
) -> (image::RgbaImage, image::RgbaImage) {
    let (w, h) = (320, 240);
    let cap = renderer.capture_begin(
        rs,
        w,
        h,
        camera.view(),
        camera.proj(w as f32 / h as f32),
        camera.is_perspective(),
        camera.cue_uniform(),
        camera.ao_uniform(),
        camera.shadow_uniform(),
        camera.background,
        camera.eye_depth_range(),
        scene,
    );
    rs.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let raster = cap.read();
    renderer.prepare_raytrace(rs, scene, camera, [w, h], true);
    let cap = renderer
        .capture_begin_raytrace(rs, w, h, camera, 128)
        .unwrap();
    rs.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    (raster, cap.read())
}

fn populate_rep(
    renderer: &SceneRenderer,
    rs: &RenderState,
    scene: &mut Scene,
    style: crate::geometry::RepKind,
    material: crate::material::Material,
) {
    use crate::{geometry, scene::Representation, secstruct::SsMap};
    let mol = &mut scene.molecules[0];
    let mut rep = Representation::new(style);
    rep.material = material;
    let sel = mol.data.select_all();
    let bound = mol.data.bind_with_state(&sel, mol.render_state());
    let ss =
        geometry::needs_ss(&rep.params, rep.color).then(|| SsMap::compute(&bound, rep.ss_algo));
    let geom = geometry::build(
        &bound,
        mol.n_atoms,
        &mol.bonds,
        &rep.params,
        rep.color_spec(),
        rep.material,
        ss.as_ref(),
        true,
    );
    rep.gpu = renderer.upload(rs, &geom);
    rep.sel = Some(sel);
    mol.reps = vec![rep];
}

#[test]
#[ignore = "requires native GPU; compares actual raster and ray-traced molecular images"]
fn molecular_appearance_matches_for_materials_effects_and_projections() {
    use crate::geometry::RepKind;
    let rs = gpu();
    let mut renderer = SceneRenderer::new(&rs, &crate::settings::RenderingSettings::default());
    let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb"));
    let raw = crate::data::load(path).unwrap();
    let mut camera = Camera::frame_bbox(raw.bbox_min, raw.bbox_max, 0.8);
    let mut scene = Scene::default();
    scene.add(raw, &crate::settings::RepDefaults::default());
    for style in [RepKind::Vdw, RepKind::Licorice, RepKind::Cartoon] {
        populate_rep(
            &renderer,
            &rs,
            &mut scene,
            style,
            crate::material::Material::Opaque,
        );
        for projection in [Projection::Orthographic, Projection::Perspective] {
            camera.projection = projection;
            for effect in ["base", "ao", "shadow", "combined", "gradient"] {
                camera.ao.enabled = matches!(effect, "ao" | "combined");
                camera.ao.strength = crate::camera::Ao::default().strength;
                camera.shadow.enabled = matches!(effect, "shadow" | "combined");
                camera.shadow.strength = 1.0;
                camera.background.kind = if effect == "gradient" {
                    BgKind::Gradient
                } else {
                    BgKind::Solid
                };
                let (raster, trace) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
                if let Ok(dir) = std::env::var("MOLAR_VIS_TEST_IMAGES") {
                    let dir = std::path::Path::new(&dir);
                    std::fs::create_dir_all(dir).unwrap();
                    raster
                        .save(dir.join(format!("{style:?}_{projection:?}_{effect}_raster.png")))
                        .unwrap();
                    trace
                        .save(dir.join(format!("{style:?}_{projection:?}_{effect}_trace.png")))
                        .unwrap();
                }
                let mut error = 0.0;
                let mut foreground_error = 0.0;
                let mut foreground_count = 0;
                for (r, t) in raster.pixels().zip(trace.pixels()) {
                    let delta: f64 = (0..3).map(|i| (r[i] as f64 - t[i] as f64).abs()).sum();
                    error += delta;
                    if effect != "gradient" && r.0[..3].iter().any(|&v| v > 30) {
                        foreground_error += delta;
                        foreground_count += 3;
                    }
                }
                let mae = error / (raster.width() * raster.height() * 3) as f64;
                let fg = foreground_error / foreground_count.max(1) as f64;
                eprintln!("{style:?}/{projection:?}/{effect}: MAE {mae:.2}, foreground {fg:.2}");
                assert!(
                    // Analytic soft shadows legitimately differ from the map filter at contacts.
                    mae < 3.0
                        && fg
                            < if matches!(effect, "shadow" | "combined") {
                                12.0
                            } else {
                                10.0
                            },
                    "appearance changed: {style:?}/{projection:?}/{effect}"
                );
                if effect == "gradient" {
                    for p in [(0, 0), (0, 239), (319, 0), (319, 239)] {
                        let (r, t) = (raster.get_pixel(p.0, p.1), trace.get_pixel(p.0, p.1));
                        assert!((0..3).all(|i| r[i].abs_diff(t[i]) <= 1), "gradient changed");
                    }
                }
            }
        }
    }
    // GI must not tone-map the backdrop. Moving the slab completely past the
    // molecule also checks that primary rays honor the camera's far clip plane.
    camera.background.kind = BgKind::Gradient;
    camera.gi = 1.0;
    camera.clip_offset = Vec3::Z * (-camera.scene_radius * 10.0);
    camera.ao.enabled = false;
    camera.shadow.enabled = false;
    let (raster, trace) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
    assert!(raster
        .pixels()
        .zip(trace.pixels())
        .all(|(r, t)| (0..3).all(|i| r[i].abs_diff(t[i]) <= 1)));
}

#[test]
#[ignore = "requires native GPU; compares outlines, transparency and color-space compositing"]
fn materials_match_in_linear_and_srgb_targets() {
    use crate::{geometry::RepKind, material::Material};
    let mut rs = gpu();
    let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb"));
    let raw = crate::data::load(path).unwrap();
    let mut camera = Camera::frame_bbox(raw.bbox_min, raw.bbox_max, 0.8);
    camera.ao.enabled = true;
    camera.shadow.enabled = true;
    let mut scene = Scene::default();
    scene.add(raw, &crate::settings::RepDefaults::default());
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ] {
        rs.target_format = format;
        let mut renderer = SceneRenderer::new(&rs, &crate::settings::RenderingSettings::default());
        for style in [RepKind::Vdw, RepKind::Licorice, RepKind::Cartoon] {
            for material in [
                Material::Glossy,
                Material::Metal,
                Material::AoEdgy,
                Material::Transparent,
                Material::Glass,
                Material::Ghost,
            ] {
                populate_rep(&renderer, &rs, &mut scene, style, material);
                let (raster, trace) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
                let error: f64 = raster
                    .pixels()
                    .zip(trace.pixels())
                    .map(|(r, t)| (0..3).map(|i| r[i].abs_diff(t[i]) as f64).sum::<f64>())
                    .sum();
                let mae = error / (raster.width() * raster.height() * 3) as f64;
                eprintln!("{format:?}/{style:?}/{material:?}: MAE {mae:.2}");
                assert!(
                    mae < 3.0,
                    "material changed: {format:?}/{style:?}/{material:?}"
                );
            }
        }
        for style in [RepKind::Lines, RepKind::BallAndStick, RepKind::Surface] {
            populate_rep(&renderer, &rs, &mut scene, style, Material::Opaque);
            let (raster, trace) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
            let error: f64 = raster
                .pixels()
                .zip(trace.pixels())
                .map(|(r, t)| (0..3).map(|i| r[i].abs_diff(t[i]) as f64).sum::<f64>())
                .sum();
            let mae = error / (raster.width() * raster.height() * 3) as f64;
            eprintln!("{format:?}/{style:?}: MAE {mae:.2}");
            assert!(
                mae < 3.0,
                "geometry appearance changed: {format:?}/{style:?}"
            );
        }
    }
}

#[test]
#[ignore = "requires native GPU; verifies that traced AO sees off-screen occluders"]
fn traced_ao_detects_blockers_absent_from_the_depth_buffer() {
    use crate::{geometry::RepKind, material::Material};
    let rs = gpu();
    let mut renderer = SceneRenderer::new(&rs, &crate::settings::RenderingSettings::default());
    let atom = molar::prelude::Atom::new().with_name("C").guess();
    let raw = crate::data::RawMolecule::single_atom("ao target", atom.clone(), Vec3::ZERO).unwrap();
    let mut scene = Scene::default();
    scene.add(raw, &crate::settings::RepDefaults::default());
    let mut camera = Camera::frame_bbox(Vec3::splat(-0.1), Vec3::splat(0.1), 0.8);
    camera.scene_radius = 1.0; // clip bounds include blockers, without widening the view
    camera.depth_cue.enabled = false;
    camera.ao.enabled = true;
    camera.ao.radius = 0.8;
    camera.ao.strength = 1.0;
    populate_rep(&renderer, &rs, &mut scene, RepKind::Vdw, Material::AoChalky);
    let (raster_open, trace_open) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
    for i in 0..8 {
        let angle = i as f32 * std::f32::consts::TAU / 8.0;
        let pos = Vec3::new(0.45 * angle.cos(), 0.45 * angle.sin(), 0.30);
        scene.molecules[0].add_atom(&atom, pos).unwrap();
    }
    populate_rep(&renderer, &rs, &mut scene, RepKind::Vdw, Material::AoChalky);
    let (raster_blocked, trace_blocked) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
    let mean = |image: &image::RgbaImage| {
        let mut sum = 0.0;
        for y in 110..130 {
            for x in 150..170 {
                sum += image.get_pixel(x, y).0[..3]
                    .iter()
                    .map(|&v| v as f64)
                    .sum::<f64>();
            }
        }
        sum / (20.0 * 20.0 * 3.0)
    };
    let raster_change = mean(&raster_open) - mean(&raster_blocked);
    let trace_change = mean(&trace_open) - mean(&trace_blocked);
    eprintln!("Off-screen blockers: raster darkening {raster_change:.2}, traced AO darkening {trace_change:.2}");
    assert!(
        raster_change.abs() < 1.0,
        "blockers must remain absent from the raster depth buffer"
    );
    assert!(trace_change > 5.0, "traced AO must detect the 3D occluders");
    camera.ao.radius = 0.1; // the same blockers now lie outside the user's AO range
    let (_, short_range) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
    assert!(
        (mean(&short_range) - mean(&trace_open)).abs() < 1.0,
        "AO radius must limit the rays in nm"
    );
}

#[test]
#[ignore = "requires native GPU; checks shadow acne, scene-size bias and penumbrae"]
fn traced_shadows_keep_lit_surfaces_clean_and_soften_the_terminator() {
    use crate::{geometry::RepKind, material::Material};
    let rs = gpu();
    let mut renderer = SceneRenderer::new(&rs, &crate::settings::RenderingSettings::default());
    let atom = molar::prelude::Atom::new().with_name("C").guess();
    let raw = crate::data::RawMolecule::single_atom("shadow target", atom, Vec3::ZERO).unwrap();
    let mut scene = Scene::default();
    scene.add(raw, &crate::settings::RepDefaults::default());
    populate_rep(&renderer, &rs, &mut scene, RepKind::Vdw, Material::AoChalky);
    let mut camera = Camera::frame_bbox(Vec3::splat(-0.2), Vec3::splat(0.2), 0.8);
    camera.depth_cue.enabled = false;
    camera.ao.enabled = false;
    let (_, open) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
    camera.shadow.enabled = true;
    camera.shadow.strength = 1.0;
    camera.shadow.softness = 0.0;
    let (_, hard) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
    // Central, light-facing sphere surface must not acquire shadow acne.
    for y in 105..125 {
        for x in 150..170 {
            assert!((0..3).all(|c| open.get_pixel(x, y)[c].abs_diff(hard.get_pixel(x, y)[c]) <= 1));
        }
    }
    camera.scene_radius = 20.0;
    let (_, large_bounds) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
    assert!(
        hard.pixels()
            .zip(large_bounds.pixels())
            .all(|(a, b)| (0..3).all(|c| a[c].abs_diff(b[c]) <= 1)),
        "shadow bias must not erase contacts or shift the terminator with scene bounds"
    );
    camera.shadow.softness = 1.0;
    let (_, soft) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
    let partial = |im: &image::RgbaImage| {
        open.pixels()
            .zip(im.pixels())
            .filter(|(a, b)| {
                let baseline = a[0] as f32;
                baseline > 50.0 && b[0] as f32 > baseline * 0.1 && (b[0] as f32) < baseline * 0.9
            })
            .count()
    };
    eprintln!(
        "Partially shadowed pixels: hard {}, soft {}",
        partial(&hard),
        partial(&soft)
    );
    assert!(
        partial(&soft) > partial(&hard) + 100,
        "finite light must soften the shadow terminator"
    );
}

#[test]
#[ignore = "requires native GPU; checks capsule exits after rejecting the near hemisphere"]
fn capsule_shadow_rays_find_the_valid_exit() {
    let rs = gpu();
    let source = include_str!("shaders/raytrace.wgsl");
    let start = source.find("fn ray_cylinder(").unwrap();
    let end = source[start..].find("// Möller").unwrap() + start;
    let source = format!(
        "struct Cyl {{ c0: vec4<f32>, c1: vec4<f32>, m: vec4<u32> }};\n{}\n\
        @group(0) @binding(0) var<storage, read_write> result: array<f32>;\n\
        @compute @workgroup_size(1) fn main() {{\n\
            let c = Cyl(vec4<f32>(0.0,0.0,0.0,0.2),vec4<f32>(0.0,0.0,1.0,0.0),vec4<u32>(0u));\n\
            result[0] = ray_cylinder(c, vec3<f32>(0.1,0.0,0.5), vec3<f32>(0.0,0.0,1.0), true);\n\
            result[1] = ray_cylinder(c, vec3<f32>(0.1,0.0,-0.1), vec3<f32>(0.0,0.0,1.0), true);\n\
        }}",
        &source[start..end]
    );
    let shader = rs
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("capsule-shadow-regression"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
    let pipeline = rs
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
    let output = rs.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 8,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = rs.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 8,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let group = rs.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: output.as_entire_binding(),
        }],
    });
    let mut encoder = rs
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 8);
    rs.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    rs.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let bytes = readback.slice(..).get_mapped_range();
    let values: &[f32] = bytemuck::cast_slice(&bytes);
    let cap_height = (0.2_f32.powi(2) - 0.1_f32.powi(2)).sqrt();
    assert!((values[0] - (0.5 + cap_height)).abs() < 1e-5);
    assert!((values[1] - (1.1 + cap_height)).abs() < 1e-5);
}
