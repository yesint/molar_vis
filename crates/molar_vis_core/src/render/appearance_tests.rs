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
        let offset = shadow_matrix.transform_vector3(direction * u.shadow[1]);
        assert!((offset.z + camera.shadow_uniform()[1]).abs() < 1e-6);
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
                    mae < 3.0 && fg < 10.0,
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
