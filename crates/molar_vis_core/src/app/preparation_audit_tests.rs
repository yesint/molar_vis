use super::*;
use crate::{geometry_jobs::Input, material::Material};

fn fixture(kind: RepKind, selection: &str) -> Scene {
    let raw = crate::data::load(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/2lao.pdb"
    )))
    .unwrap();
    let mut scene = Scene::default();
    scene.add(raw, &Settings::default().reps);
    let mut rep = Representation::new(kind);
    if kind == RepKind::Surface {
        rep.params = RepParams::Surface {
            probe: 0.14,
            quality: 0,
            smoothing: 1,
        };
    }
    rep.sel_text = selection.into();
    rep.color = ColorMethod::Element;
    scene.molecules[0].reps = vec![rep];
    scene.refresh_selections();
    scene
}

fn caches(mol: &scene::Molecule) -> (Option<SsMap>, Option<geometry::CartoonCache>) {
    let rep = &mol.reps[0];
    let bound = mol
        .data
        .bind_with_state(rep.sel.as_ref().unwrap(), mol.render_state());
    let ss =
        geometry::needs_ss(&rep.params, rep.color).then(|| SsMap::compute(&bound, rep.ss_algo));
    let cartoon = geometry::CartoonCache::new(
        &bound,
        mol.n_atoms,
        &rep.params,
        rep.color_spec(),
        rep.material,
        ss.as_ref(),
    );
    (ss, cartoon)
}

fn build(
    mol: &scene::Molecule,
    ss: Option<&SsMap>,
    cartoon: Option<&geometry::CartoonCache>,
    reuse: bool,
    background: bool,
    dashed: bool,
) -> GeometryBuild {
    let rep = &mol.reps[0];
    let state = mol.render_state();
    let bound = mol.data.bind_with_state(rep.sel.as_ref().unwrap(), state);
    // Reproduce the former gather/filter/drop path independently of the new branch.
    if !reuse && background {
        let input = if let Some(cartoon) = cartoon {
            Some(Input::Cartoon(cartoon.snapshot(state, dashed)))
        } else if let RepParams::Surface {
            probe,
            quality,
            smoothing,
        } = rep.params
        {
            let colors = crate::color::Colorizer::new(rep.color_spec(), &bound, mol.n_atoms, ss);
            Some(Input::Surface(
                geometry::SurfaceInput::new(&bound, &colors, probe, quality, smoothing),
                rep.material,
            ))
        } else {
            None
        };
        if let Some(input) = input.filter(|i| i.worth_offloading()) {
            return GeometryBuild::Background(input);
        }
    }
    prepare_geometry(
        &bound,
        state,
        mol.n_atoms,
        &rep.params,
        rep.color_spec(),
        rep.material,
        ss,
        cartoon,
        dashed,
        background && reuse,
        || {
            cartoon.map_or_else(
                || {
                    geometry::build_with_reference(
                        &bound,
                        mol.n_atoms,
                        &mol.bonds,
                        &rep.params,
                        rep.color_spec(),
                        rep.material,
                        ss,
                        dashed,
                        Some(mol.data.state()),
                    )
                },
                |cache| cache.build(state, dashed),
            )
        },
    )
}

fn ready(result: GeometryBuild) -> geometry::GeometryData {
    match result {
        GeometryBuild::Ready(geom) => geom,
        GeometryBuild::Background(_) => panic!("unexpected background build"),
    }
}

fn equal(a: &geometry::GeometryData, b: &geometry::GeometryData) {
    assert_eq!(
        bytemuck::cast_slice::<_, u8>(&a.mesh.vertices),
        bytemuck::cast_slice::<_, u8>(&b.mesh.vertices)
    );
    assert_eq!(a.mesh.indices, b.mesh.indices);
    assert_eq!(a.mesh.vert_atom, b.mesh.vert_atom);
    assert_eq!(a.mesh.vert_res, b.mesh.vert_res);
    assert_eq!(
        bytemuck::cast_slice::<_, u8>(&a.spheres),
        bytemuck::cast_slice::<_, u8>(&b.spheres)
    );
    assert_eq!(
        bytemuck::cast_slice::<_, u8>(&a.cylinders),
        bytemuck::cast_slice::<_, u8>(&b.cylinders)
    );
    assert_eq!(
        bytemuck::cast_slice::<_, u8>(&a.lines),
        bytemuck::cast_slice::<_, u8>(&b.lines)
    );
}

#[test]
fn prepared_small_inputs_preserve_complete_geometry_and_offload_boundary() {
    for (kind, selection) in [
        (RepKind::Surface, "index 0"),
        (RepKind::Surface, "index 0:254"),
        (RepKind::Cartoon, "resid 1:20"),
        (RepKind::Vdw, "resid 1:3"),
    ] {
        let mut scene = fixture(kind, selection);
        for material in [Material::Opaque, Material::Glass, Material::MolecularNodes] {
            scene.molecules[0].reps[0].material = material;
            let mol = &scene.molecules[0];
            let (ss, cartoon) = caches(mol);
            for background in [false, true] {
                for dashed in [false, true] {
                    let mut a = ready(build(
                        mol,
                        ss.as_ref(),
                        cartoon.as_ref(),
                        false,
                        background,
                        dashed,
                    ));
                    let mut b = ready(build(
                        mol,
                        ss.as_ref(),
                        cartoon.as_ref(),
                        true,
                        background,
                        dashed,
                    ));
                    equal(&a, &b);
                    gray_out(&mut a);
                    gray_out(&mut b);
                    equal(&a, &b);
                }
            }
        }
    }
    let scene = fixture(RepKind::Surface, "index 0:255");
    for reuse in [false, true] {
        assert!(matches!(
            build(&scene.molecules[0], None, None, reuse, true, false),
            GeometryBuild::Background(_)
        ));
    }
}

#[test]
#[ignore = "CPU measurement of complete small geometry builds"]
fn prepared_small_inputs_paired_benchmark() {
    for (kind, selection) in [
        (RepKind::Surface, "index 0:7"),
        (RepKind::Surface, "index 0:127"),
        (RepKind::Cartoon, "resid 1:20"),
        (RepKind::Cartoon, "all"),
        (RepKind::Vdw, "index 0:127"),
    ] {
        let scene = fixture(kind, selection);
        let mol = &scene.molecules[0];
        let (ss, cartoon) = caches(mol);
        let mut times = [Vec::new(), Vec::new()];
        for iteration in 0..23 {
            let mut outputs = [None, None];
            for mode in if iteration % 2 == 0 { [0, 1] } else { [1, 0] } {
                let start = std::time::Instant::now();
                // Batch short builds to reduce timer noise without retaining their geometry.
                for _ in 0..8 {
                    outputs[mode] = Some(ready(build(
                        mol,
                        ss.as_ref(),
                        cartoon.as_ref(),
                        mode == 1,
                        true,
                        false,
                    )));
                    std::hint::black_box(outputs[mode].as_ref());
                }
                if iteration >= 2 {
                    times[mode].push(start.elapsed().as_secs_f64() * 125.0);
                }
            }
            equal(outputs[0].as_ref().unwrap(), outputs[1].as_ref().unwrap());
        }
        for (mode, samples) in times.iter_mut().enumerate() {
            samples.sort_by(f64::total_cmp);
            eprintln!(
                "PREPARE_BUILD {}",
                serde_json::json!({"kind":format!("{kind:?}"), "selection":selection,
                "mode":mode, "median_ms":samples[10], "p95_ms":samples[19]})
            );
        }
    }
}

#[test]
#[ignore = "short GPU parity check for reuse of prepared surface and cartoon inputs"]
fn prepared_small_inputs_match_images() {
    let rs = crate::render::test_gpu();
    let settings = Settings::default();
    let mut renderer = SceneRenderer::new(&rs, &settings.rendering);
    for kind in [RepKind::Surface, RepKind::Cartoon] {
        let mut scene = fixture(kind, "resid 1:20");
        // Keep the surface below the 256-atom background threshold.
        if kind == RepKind::Surface {
            scene.molecules[0].reps[0].sel_text = "index 0:127".into();
            scene.molecules[0].reps[0].sel_dirty = true;
            scene.refresh_selections();
        }
        for material in [Material::Opaque, Material::Glass, Material::MolecularNodes] {
            scene.molecules[0].reps[0].material = material;
            let mol = &scene.molecules[0];
            let (ss, cartoon) = caches(mol);
            let a = ready(build(
                mol,
                ss.as_ref(),
                cartoon.as_ref(),
                false,
                true,
                false,
            ));
            let b = ready(build(mol, ss.as_ref(), cartoon.as_ref(), true, true, false));
            equal(&a, &b);
            let lo = a
                .mesh
                .vertices
                .iter()
                .fold(glam::Vec3::splat(f32::INFINITY), |a, v| {
                    a.min(glam::Vec3::from_array(v.pos))
                });
            let hi = a
                .mesh
                .vertices
                .iter()
                .fold(glam::Vec3::splat(f32::NEG_INFINITY), |a, v| {
                    a.max(glam::Vec3::from_array(v.pos))
                });
            let mut camera = crate::camera::Camera::frame_bbox(lo, hi, 0.8);
            camera.ao.enabled = true;
            camera.shadow.enabled = true;
            for projection in [
                crate::camera::Projection::Orthographic,
                crate::camera::Projection::Perspective,
            ] {
                camera.projection = projection;
                let mut pictures = Vec::new();
                for geom in [&a, &b] {
                    scene.molecules[0].reps[0].gpu = renderer.upload(&rs, geom);
                    let cap = renderer.capture_begin(
                        &rs,
                        160,
                        120,
                        camera.view(),
                        camera.proj(4.0 / 3.0),
                        camera.is_perspective(),
                        camera.cue_uniform(),
                        camera.ao_uniform(),
                        camera.shadow_uniform(),
                        camera.background,
                        camera.eye_depth_range(),
                        &scene,
                    );
                    rs.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                    pictures.push(cap.read());
                }
                assert_eq!(pictures[0], pictures[1]);
                if let Ok(dir) = std::env::var("MOLAR_VIS_TEST_IMAGES") {
                    std::fs::create_dir_all(&dir).unwrap();
                    for (mode, picture) in pictures.iter().enumerate() {
                        picture
                            .save(format!(
                                "{dir}/prepare-{kind:?}-{material:?}-{projection:?}-{mode}.png"
                            ))
                            .unwrap();
                    }
                }
            }
        }
    }
}
