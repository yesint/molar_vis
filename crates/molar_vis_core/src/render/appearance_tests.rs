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
fn envelope_shaders_validate_without_a_gpu() {
    for source in [
        include_str!("shaders/sphere.wgsl"),
        include_str!("shaders/cylinder.wgsl"),
    ] {
        let source = lit_shader_source(&envelope::shader(source, true));
        let module = wgpu::naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
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

pub(super) fn gpu() -> RenderState {
    use std::sync::Arc;
    pollster::block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .unwrap();
        eprintln!("Rendering on {:?}", adapter.get_info());
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

/// Close-up, native-resolution reproducer for inner ribbon AO and shadow artifacts.
/// MOLAR_VIS_HELIX_PDB selects a real structure; output is kept for visual inspection.
#[test]
#[ignore = "requires GPU; writes helix close-ups for visual inspection"]
fn helix_closeup_render() {
    use crate::{
        geometry,
        scene::Representation,
        secstruct::{SsClass, SsMap},
    };
    let rs = gpu();
    let mut renderer = SceneRenderer::new(&rs, &crate::settings::RenderingSettings::default());
    let path = std::env::var("MOLAR_VIS_HELIX_PDB")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb").into());
    let raw = crate::data::load(std::path::Path::new(&path)).unwrap();
    let mut scene = Scene::default();
    scene.add(raw, &crate::settings::RepDefaults::default());
    let mol = &mut scene.molecules[0];
    let mut rep = Representation::new(geometry::RepKind::Cartoon);
    let sel = mol.data.select_all();
    let bound = mol.data.bind_with_state(&sel, mol.render_state());
    let ss = SsMap::compute(&bound, rep.ss_algo);
    let mut residues: Vec<_> = ss.entries().map(|(i, _)| i).collect();
    residues.sort_unstable();
    let mut runs = Vec::new();
    let mut run = Vec::new();
    for i in residues {
        if ss.class(i) == SsClass::Helix && run.last().is_none_or(|last| i == last + 1) {
            run.push(i);
        } else {
            if !run.is_empty() {
                runs.push(std::mem::take(&mut run));
            }
            if ss.class(i) == SsClass::Helix {
                run.push(i);
            }
        }
    }
    runs.push(run);
    let run = runs.into_iter().max_by_key(Vec::len).unwrap();
    eprintln!("Helix residues: {run:?}");
    let mut geom = geometry::build(
        &bound,
        mol.n_atoms,
        &mol.bonds,
        &rep.params,
        rep.color_spec(),
        rep.material,
        Some(&ss),
        true,
    );
    geom.mesh.indices = geom
        .mesh
        .indices
        .chunks_exact(3)
        .filter(|tri| {
            tri.iter()
                .all(|&i| run.contains(&(geom.mesh.vert_res[i as usize] as usize)))
        })
        .flatten()
        .copied()
        .collect();
    let points: Vec<_> = geom
        .mesh
        .indices
        .iter()
        .map(|&i| Vec3::from_array(geom.mesh.vertices[i as usize].pos))
        .collect();
    let center = points.iter().copied().sum::<Vec3>() / points.len() as f32;
    let mut axis = Vec3::new(0.3, 0.8, 0.5);
    for _ in 0..32 {
        axis = points
            .iter()
            .map(|p| (*p - center) * (*p - center).dot(axis))
            .sum::<Vec3>()
            .normalize();
    }
    let orbit: f32 = std::env::var("MOLAR_VIS_HELIX_ORBIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.5);
    let rotation = glam::Quat::from_rotation_z(-0.25)
        * glam::Quat::from_rotation_y(orbit)
        * glam::Quat::from_rotation_arc(axis, Vec3::Y);
    for v in &mut geom.mesh.vertices {
        v.pos = (rotation * (Vec3::from_array(v.pos) - center)).to_array();
        v.normal = (rotation * Vec3::from_array(v.normal)).to_array();
        v.color = 0xffb044a0;
    }
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for &i in &geom.mesh.indices {
        let p = Vec3::from_array(geom.mesh.vertices[i as usize].pos);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    rep.gpu = renderer.upload(&rs, &geom);
    rep.sel = Some(sel);
    mol.reps = vec![rep];
    let mut camera = Camera::frame_bbox(lo, hi, 0.9);
    camera.distance *= 0.42;
    if std::env::var_os("MOLAR_VIS_HELIX_PERSPECTIVE").is_some() {
        camera.projection = Projection::Perspective;
    }
    camera.depth_cue.enabled = false;
    camera.background = crate::camera::Background::for_theme(false);
    camera.ao.strength = 0.8;
    camera.shadow.strength = 0.8;
    let dir = std::env::var("MOLAR_VIS_TEST_IMAGES").unwrap_or_else(|_| {
        std::env::temp_dir()
            .join("molar-helix-closeup")
            .to_string_lossy()
            .into_owned()
    });
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h) = (640, 800);
    for effect in ["base", "ao", "shadow", "combined"] {
        camera.ao.enabled = matches!(effect, "ao" | "combined");
        camera.shadow.enabled = matches!(effect, "shadow" | "combined");
        let cap = renderer.capture_begin(
            &rs,
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
            &scene,
        );
        rs.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        cap.read()
            .save(format!("{dir}/{effect}_raster.png"))
            .unwrap();
        renderer
            .raytracer
            .as_mut()
            .unwrap()
            .upload(&rs, &raytrace::RtScene::from_test_mesh(&geom.mesh, geometry::RepKind::Cartoon));
        let cap = renderer
            .capture_begin_raytrace(&rs, w, h, &camera, 64)
            .unwrap();
        rs.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        cap.read()
            .save(format!("{dir}/{effect}_trace.png"))
            .unwrap();
    }
    let mut metric_dirs = vec![dir.clone()];
    if let Ok(before) = std::env::var("MOLAR_VIS_COMPARE_IMAGES") {
        metric_dirs.push(before);
    }
    for metric_dir in metric_dirs {
        for mode in ["raster", "trace"] {
            let base = image::open(format!("{metric_dir}/base_{mode}.png"))
                .unwrap()
                .to_rgba8();
            for effect in ["ao", "shadow"] {
                let effected = image::open(format!("{metric_dir}/{effect}_{mode}.png"))
                    .unwrap()
                    .to_rgba8();
                let mut roughness = 0.0;
                let mut count = 0;
                for y in 5..h - 5 {
                    for x in 5..w - 5 {
                        let coords = [(x, y), (x - 4, y), (x + 4, y), (x, y - 4), (x, y + 4)];
                        let values: Vec<_> = coords
                            .iter()
                            .map(|&(xx, yy)| base.get_pixel(xx, yy)[0] as f32)
                            .collect();
                        if values
                            .iter()
                            .any(|&v| v < 40.0 || v > 230.0 || (v - values[0]).abs() > 8.0)
                        {
                            continue;
                        }
                        let f: Vec<_> = coords
                            .iter()
                            .zip(&values)
                            .map(|(&(xx, yy), &b)| effected.get_pixel(xx, yy)[0] as f32 / b)
                            .collect();
                        roughness += ((f[1] - 2.0 * f[0] + f[2]).abs()
                            + (f[3] - 2.0 * f[0] + f[4]).abs())
                            as f64;
                        count += 1;
                    }
                }
                let curvature = roughness / count as f64;
                eprintln!(
                    "{metric_dir}/{effect}/{mode}: interior factor curvature {curvature} ({count} pixels)"
                );
                if metric_dir == dir {
                    assert!(
                        count > 10_000,
                        "close-up must contain a substantial smooth surface"
                    );
                    let limit = match (mode, effect) {
                        ("raster", "ao") => 0.0075,
                        ("trace", "ao") => 0.016,
                        _ => 0.04,
                    };
                    assert!(
                        curvature < limit,
                        "{effect}: patchy interior occlusion ({curvature})"
                    );
                    assert!(
                        effected
                            .pixels()
                            .zip(base.pixels())
                            .any(|(e, b)| b[0] < 230 && (e[0] as f32) < b[0] as f32 * 0.9),
                        "effect must remain visible"
                    );
                }
            }
        }
    }
    if let Ok(before) = std::env::var("MOLAR_VIS_COMPARE_IMAGES") {
        for name in ["ao_raster", "combined_raster", "combined_trace"] {
            let old = image::open(format!("{before}/{name}.png"))
                .unwrap()
                .to_rgba8();
            let new = image::open(format!("{dir}/{name}.png")).unwrap().to_rgba8();
            let mut pair = image::RgbaImage::new(w * 2, h);
            image::imageops::overlay(&mut pair, &old, 0, 0);
            image::imageops::overlay(&mut pair, &new, w as i64, 0);
            pair.save(format!("{dir}/{name}_comparison.png")).unwrap();
        }
    }
}

fn raster_and_trace(
    renderer: &mut SceneRenderer,
    rs: &RenderState,
    scene: &Scene,
    camera: &Camera,
) -> (image::RgbaImage, image::RgbaImage) {
    raster_and_trace_at(renderer, rs, scene, camera, 320, 240)
}

fn raster_and_trace_at(
    renderer: &mut SceneRenderer,
    rs: &RenderState,
    scene: &Scene,
    camera: &Camera,
    w: u32,
    h: u32,
) -> (image::RgbaImage, image::RgbaImage) {
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
    populate_rep_at(renderer, rs, scene, 0, style, material);
}

fn populate_rep_at(
    renderer: &SceneRenderer,
    rs: &RenderState,
    scene: &mut Scene,
    molecule: usize,
    style: crate::geometry::RepKind,
    material: crate::material::Material,
) {
    use crate::{geometry, scene::Representation, secstruct::SsMap};
    let mol = &mut scene.molecules[molecule];
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
    let (legacy_raster, hard) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
    camera.shadow.softness = crate::camera::Shadow::MIN_SOFTNESS;
    let (minimum_raster, minimum_trace) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
    assert_eq!(legacy_raster, minimum_raster, "legacy zero must use the raster minimum");
    assert_eq!(hard, minimum_trace, "legacy zero must use the RT minimum");
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

#[test]
#[ignore = "requires native GPU; checks smooth surface self-occlusion and saves close-ups"]
fn smooth_surface_secondary_rays_do_not_create_triangle_patches() {
    use crate::{geometry::RepKind, material::Material};
    let rs = gpu();
    let mut renderer = SceneRenderer::new(&rs, &crate::settings::RenderingSettings::default());
    let atom = molar::prelude::Atom::new().with_name("C").guess();
    let raw = crate::data::RawMolecule::single_atom("surface target", atom, Vec3::ZERO).unwrap();
    let mut scene = Scene::default();
    scene.add(raw, &crate::settings::RepDefaults::default());
    populate_rep(
        &renderer,
        &rs,
        &mut scene,
        RepKind::Surface,
        Material::AoChalky,
    );
    let mut camera = Camera::frame_bbox(Vec3::splat(-0.2), Vec3::splat(0.2), 0.8);
    camera.depth_cue.enabled = false;
    camera.ao.enabled = false;
    let (_, open) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
    camera.ao.enabled = true;
    camera.ao.strength = 1.0;
    let (_, ao) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
    let mut darkening = 0.0;
    let mut count = 0;
    for (a, b) in open.pixels().zip(ao.pixels()) {
        if a[0] > 50 {
            darkening += a[0].saturating_sub(b[0]) as f64;
            count += 1;
        }
    }
    let mean = darkening / count as f64;
    eprintln!("Convex surface self-AO darkening: {mean:.2}");
    assert!(
        mean < 3.0,
        "smooth convex surface must not acquire false self-occlusion patches"
    );
    camera.ao.enabled = false;
    camera.shadow.enabled = true;
    camera.shadow.strength = 1.0;
    camera.shadow.softness = 0.0;
    let (_, hard) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
    let mol = &scene.molecules[0];
    let rep = &mol.reps[0];
    let bound = mol
        .data
        .bind_with_state(rep.sel.as_ref().unwrap(), mol.render_state());
    let geom = crate::geometry::build(
        &bound,
        mol.n_atoms,
        &mol.bonds,
        &rep.params,
        rep.color_spec(),
        rep.material,
        None,
        true,
    );
    let inverse = (camera.proj(320.0 / 240.0) * camera.view()).inverse();
    let light = camera
        .view()
        .inverse()
        .transform_vector3(SHADOW_LIGHT_DIR_VIEW)
        .normalize();
    let mut checked = 0;
    for y in (40..200).step_by(3) {
        for x in (60..260).step_by(3) {
            let ndc = Vec3::new(
                (x as f32 + 0.5) / 320.0 * 2.0 - 1.0,
                1.0 - (y as f32 + 0.5) / 240.0 * 2.0,
                0.0,
            );
            let origin = inverse.project_point3(ndc);
            let direction = (inverse.project_point3(ndc + Vec3::Z) - origin).normalize();
            let mut nearest = f32::INFINITY;
            let mut normal = Vec3::ZERO;
            for tri in geom.mesh.indices.chunks_exact(3) {
                let [a, b, c] = [
                    geom.mesh.vertices[tri[0] as usize],
                    geom.mesh.vertices[tri[1] as usize],
                    geom.mesh.vertices[tri[2] as usize],
                ];
                let pa = Vec3::from_array(a.pos);
                let e1 = Vec3::from_array(b.pos) - pa;
                let e2 = Vec3::from_array(c.pos) - pa;
                let cross = direction.cross(e2);
                let det = e1.dot(cross);
                if det.abs() < 1e-9 {
                    continue;
                }
                let offset = origin - pa;
                let u = offset.dot(cross) / det;
                let q = offset.cross(e1);
                let v = direction.dot(q) / det;
                let t = e2.dot(q) / det;
                if u < 0.0 || v < 0.0 || u + v > 1.0 || t <= 0.0 || t >= nearest {
                    continue;
                }
                nearest = t;
                normal = ((1.0 - u - v) * Vec3::from_array(a.normal)
                    + u * Vec3::from_array(b.normal)
                    + v * Vec3::from_array(c.normal))
                .normalize();
            }
            let facing = normal.dot(light);
            // Exclude silhouettes and the finite-light penumbra: even legacy zero
            // softness now uses the minimum filtered light (angular radius 0.45*s).
            if nearest.is_finite()
                && normal.dot(-direction) > 0.4
                && facing.abs() > camera.shadow_uniform()[3] * 0.45 + 0.04
                && open.get_pixel(x, y)[0] > 50
            {
                let visibility = hard.get_pixel(x, y)[0] as f32 / open.get_pixel(x, y)[0] as f32;
                assert!(if facing > 0.0 { visibility > 0.98 } else { visibility < 0.02 },
                "smooth shadow terminator disagrees with the normal at ({x},{y}): {facing}, {visibility}");
                checked += 1;
            }
        }
    }
    assert!(checked > 100);
    camera.ao.enabled = true;
    camera.shadow.strength = 0.8;
    camera.shadow.softness = 0.4;
    let (_, shadow) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
    // Rejecting artificial exit hits must still preserve entry hits on a real blocker.
    let blocker_atom = molar::prelude::Atom::new().with_name("C").guess();
    let blocker = crate::data::RawMolecule::single_atom(
        "surface blocker",
        blocker_atom,
        Vec3::new(0.35, 0.0, 0.3),
    )
    .unwrap();
    scene.add(blocker, &crate::settings::RepDefaults::default());
    populate_rep_at(
        &renderer,
        &rs,
        &mut scene,
        1,
        RepKind::Surface,
        Material::AoChalky,
    );
    camera.scene_radius = 1.0;
    camera.shadow.enabled = false;
    camera.ao.radius = 0.8;
    let (_, blocked) = raster_and_trace(&mut renderer, &rs, &scene, &camera);
    let mut blocker_darkening = 0.0;
    for y in 110..130 {
        for x in 150..170 {
            blocker_darkening += ao.get_pixel(x, y)[0] as f64 - blocked.get_pixel(x, y)[0] as f64;
        }
    }
    blocker_darkening /= 400.0;
    eprintln!("Closed surface blocker darkening: {blocker_darkening:.2}");
    assert!(
        blocker_darkening > 3.0,
        "real closed-surface blockers must still occlude"
    );
    if let Ok(dir) = std::env::var("MOLAR_VIS_TEST_IMAGES") {
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        open.save(dir.join("surface_convex_open.png")).unwrap();
        ao.save(dir.join("surface_convex_ao.png")).unwrap();
        shadow.save(dir.join("surface_convex_shadow.png")).unwrap();
        let path =
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb"));
        let raw = crate::data::load(path).unwrap();
        camera = Camera::frame_bbox(raw.bbox_min, raw.bbox_max, 0.8);
        camera.target.z = raw.bbox_max.z - 0.5;
        camera.distance *= 0.35;
        camera.depth_cue.enabled = false;
        camera.ao.enabled = true;
        camera.shadow.enabled = true;
        camera.shadow.strength = 0.5;
        camera.background.color = [1.0, 1.0, 1.0, 1.0];
        scene = Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        populate_rep(
            &renderer,
            &rs,
            &mut scene,
            RepKind::Surface,
            Material::AoChalky,
        );
        camera.shadow.enabled = false;
        let (_, unshadowed) = raster_and_trace_at(&mut renderer, &rs, &scene, &camera, 640, 480);
        unshadowed.save(dir.join("surface_closeup_ao.png")).unwrap();
        camera.shadow.enabled = true;
        let (raster, trace) = raster_and_trace_at(&mut renderer, &rs, &scene, &camera, 640, 480);
        raster.save(dir.join("surface_closeup_raster.png")).unwrap();
        trace.save(dir.join("surface_closeup_trace.png")).unwrap();
        if std::env::var_os("MOLAR_VIS_SHADOW_SWEEP").is_some() {
            camera.ao.enabled = false;
            camera.shadow.strength = 0.6;
            for softness in [0.0, 0.05, 0.1, 0.15, 0.2] {
                camera.shadow.softness = softness;
                let (raster, trace) = raster_and_trace_at(&mut renderer, &rs, &scene, &camera, 640, 480);
                raster.save(dir.join(format!("surface_softness_{softness:.2}_raster.png"))).unwrap();
                trace.save(dir.join(format!("surface_softness_{softness:.2}_trace.png"))).unwrap();
            }
        }
    }
}

#[test]
#[ignore = "requires native GPU; compares transparent envelope pixels"]
fn transparent_envelope_renders_duplicate_primitives_once() {
    let rs = gpu();
    let mut renderer = SceneRenderer::new(&rs, &crate::settings::RenderingSettings::default());
    assert!(renderer.envelope_bgl.is_some());
    let raw = crate::data::load(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/2lao.pdb"
    )))
    .unwrap();
    let mut scene = Scene::default();
    scene.add(raw, &crate::settings::RepDefaults::default());
    let material = crate::material::Material::Transparent;
    let color = ((material.opacity_u8() as u32) << 24) | 0x00b06030;
    let capsule = CylinderInstance {
        p0: [-0.4, 0.0, 0.0],
        p1: [0.4, 0.0, 0.0],
        radius: 0.1,
        color,
        color1: color,
        mat: material.pack_lighting(),
        offset: [0.0; 2],
    };
    let mut geom = GeometryData {
        cylinders: vec![capsule],
        ..Default::default()
    };
    let mut rep = crate::scene::Representation::new(crate::geometry::RepKind::Licorice);
    rep.material = material;
    rep.gpu = renderer.upload(&rs, &geom);
    scene.molecules[0].reps = vec![rep];
    let mut camera = Camera::frame_bbox(Vec3::new(-0.6, -0.3, -0.2), Vec3::new(0.6, 0.3, 0.2), 0.8);
    camera.projection = Projection::Orthographic;
    camera.depth_cue.enabled = false;
    camera.ao.enabled = false;
    camera.shadow.enabled = false;
    camera.background = crate::camera::Background::for_theme(false);
    let capture = |renderer: &mut SceneRenderer, scene: &Scene| {
        let cap = renderer.capture_begin(
            &rs,
            320,
            240,
            camera.view(),
            camera.proj(320.0 / 240.0),
            camera.is_perspective(),
            camera.cue_uniform(),
            camera.ao_uniform(),
            camera.shadow_uniform(),
            camera.background,
            camera.eye_depth_range(),
            scene,
        );
        rs.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        cap.read()
    };
    let single = capture(&mut renderer, &scene);
    geom.cylinders.push(capsule);
    scene.molecules[0].reps[0].gpu = renderer.upload(&rs, &geom);
    let union = capture(&mut renderer, &scene);
    scene.molecules[0].reps[0].gpu.envelope = None;
    let overlapping = capture(&mut renderer, &scene);
    let error = |a: &image::RgbaImage, b: &image::RgbaImage| -> u64 {
        a.as_raw()
            .iter()
            .zip(b.as_raw())
            .map(|(x, y)| x.abs_diff(*y) as u64)
            .sum()
    };
    assert!(
        error(&single, &overlapping) > 10000,
        "baseline must reproduce overlap darkening"
    );
    assert!(
        error(&single, &union) < 100,
        "duplicate surfaces must contribute exactly once"
    );
    let dir =
        std::env::var("MOLAR_VIS_TEST_IMAGES").unwrap_or_else(|_| "/tmp/molar-envelope".into());
    std::fs::create_dir_all(&dir).unwrap();
    single.save(format!("{dir}/single.png")).unwrap();
    union.save(format!("{dir}/envelope.png")).unwrap();
    overlapping.save(format!("{dir}/overlap.png")).unwrap();
}

#[test]
#[ignore = "requires native GPU; saves molecule envelope comparisons and measures frames"]
fn transparent_envelope_molecule_preview() {
    let rs = gpu();
    let mut settings = crate::settings::RenderingSettings::default();
    settings.ssaa = 1;
    let mut renderer = SceneRenderer::new(&rs, &settings);
    let raw = crate::data::load(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/2lao.pdb"
    )))
    .unwrap();
    let mut scene = Scene::default();
    scene.add(raw, &crate::settings::RepDefaults::default());
    let dir =
        std::env::var("MOLAR_VIS_TEST_IMAGES").unwrap_or_else(|_| "/tmp/molar-envelope".into());
    std::fs::create_dir_all(&dir).unwrap();
    for kind in [
        crate::geometry::RepKind::BallAndStick,
        crate::geometry::RepKind::Licorice,
    ] {
        let mol = &mut scene.molecules[0];
        let mut rep = crate::scene::Representation::new(kind);
        rep.material = crate::material::Material::Transparent;
        let selection =
            std::env::var("MOLAR_VIS_ENVELOPE_SELECTION").unwrap_or_else(|_| "resid 1:3".into());
        let (expr, sel) = mol.data.evaluate(&selection).unwrap();
        let bound = mol.data.bind_with_state(&sel, mol.render_state());
        let geom = crate::geometry::build(
            &bound,
            mol.n_atoms,
            &mol.bonds,
            &rep.params,
            rep.color_spec(),
            rep.material,
            None,
            false,
        );
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for s in &geom.spheres {
            let p = Vec3::from_array(s.center);
            lo = lo.min(p - Vec3::splat(s.radius));
            hi = hi.max(p + Vec3::splat(s.radius));
        }
        for c in &geom.cylinders {
            for p in [c.p0, c.p1] {
                let p = Vec3::from_array(p);
                lo = lo.min(p - Vec3::splat(c.radius));
                hi = hi.max(p + Vec3::splat(c.radius));
            }
        }
        drop(bound);
        rep.gpu = renderer.upload(&rs, &geom);
        rep.expr = Some(expr);
        rep.sel = Some(sel);
        mol.reps = vec![rep];
        let mut camera = Camera::frame_bbox(lo, hi, 0.8);
        camera.orientation = glam::Quat::from_rotation_y(0.5) * glam::Quat::from_rotation_x(0.3);
        camera.projection = Projection::Orthographic;
        camera.depth_cue.enabled = false;
        camera.ao.enabled = false;
        camera.shadow.enabled = false;
        camera.background = crate::camera::Background::for_theme(false);
        let (w, h) = (640, 480);
        let capture = |renderer: &mut SceneRenderer, scene: &Scene| {
            let cap = renderer.capture_begin(
                &rs,
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
            cap.read()
        };
        let envelope = scene.molecules[0].reps[0].gpu.envelope.take();
        let before = capture(&mut renderer, &scene);
        let mut times = Vec::new();
        for _ in 0..9 {
            let t = std::time::Instant::now();
            capture(&mut renderer, &scene);
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        let baseline = times[4];
        scene.molecules[0].reps[0].gpu.envelope = envelope;
        let after = capture(&mut renderer, &scene);
        times.clear();
        for _ in 0..9 {
            let t = std::time::Instant::now();
            capture(&mut renderer, &scene);
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        eprintln!("{kind:?}: baseline {baseline:.2} ms, envelope {:.2} ms (capture + GPU wait + readback)", times[4]);
        let mut pair = image::RgbaImage::new(w * 2, h);
        image::imageops::overlay(&mut pair, &before, 0, 0);
        image::imageops::overlay(&mut pair, &after, w as i64, 0);
        pair.save(format!("{dir}/{kind:?}_comparison.png")).unwrap();
        assert_ne!(
            before.as_raw(),
            after.as_raw(),
            "envelope must remove intersection layers"
        );
        renderer.prepare_raytrace(&rs, &scene, &camera, [w, h], false);
        let rt = renderer
            .capture_begin_raytrace(&rs, w, h, &camera, 16)
            .unwrap();
        rs.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rt.read()
            .save(format!("{dir}/{kind:?}_raytrace.png"))
            .unwrap();
    }
}
