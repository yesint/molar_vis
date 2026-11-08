//! Camera preparation parity and bounded rendering checks for the efficiency audit.
use super::*;
use crate::{camera::{Camera, Projection}, geometry::RepKind, scene::Representation};

fn fixture(molecules: usize, reps: usize) -> Scene {
    let mut scene = Scene::default();
    for mi in 0..molecules {
        let raw = crate::data::load(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb"))).unwrap();
        scene.add(raw, &crate::settings::RepDefaults::default());
        let mol = scene.molecules.last_mut().unwrap();
        let mut state = mol.data.state().clone();
        state.pbox = Some(molar::prelude::PeriodicBox::from_vectors_angles(
            2.0 + mi as f32 * 0.2, 3.0, 4.0, 90.0, 90.0, 80.0).unwrap());
        mol.data.set_state(state).unwrap();
        mol.visible = mi % 2 == 0;
        mol.reps = (0..reps).map(|ri| {
            let mut rep = Representation::new(RepKind::Vdw);
            rep.visible = ri % 3 == 0;
            rep.periodic.neg = [1; 3]; rep.periodic.pos = [1; 3];
            rep.periodic.self_img = ri % 2 == 0;
            rep.periodic.show_box = true;
            rep
        }).collect();
    }
    scene
}

fn prepare<const FILTER: bool>(scene: &Scene, camera: &Camera) -> (Vec<CameraUniform>, Vec<Vec<Vec<u32>>>) {
    let make = |view| CameraUniform::new(view, camera.proj(4.0/3.0), camera.is_perspective(),
        [160.0,120.0], camera.cue_uniform(), camera.background.fog_color(), camera.eye_depth_range(), 0.7, [0.0,1.0,1.0,1.0]);
    let mut cameras = vec![make(camera.view()), make(camera.view())];
    cameras[1].params[3] = 1.0;
    let images = prepare_periodic_cameras::<FILTER>(scene, camera.view(), &mut cameras, make);
    (cameras, images)
}

fn prepare_reused(scene: &Scene, camera: &Camera, cameras: &mut Vec<CameraUniform>, bytes: &mut Vec<u8>) -> Vec<Vec<Vec<u32>>> {
    let make = |view| CameraUniform::new(view, camera.proj(4.0/3.0), camera.is_perspective(),
        [160.0,120.0], camera.cue_uniform(), camera.background.fog_color(), camera.eye_depth_range(), 0.7, [0.0,1.0,1.0,1.0]);
    cameras.clear(); cameras.extend([make(camera.view()), make(camera.view())]);
    cameras[1].params[3] = 1.0;
    let images = prepare_periodic_cameras::<true>(scene, camera.view(), cameras, make);
    pack_camera_bytes(cameras, bytes);
    images
}

#[test]
fn reused_camera_storage_preserves_bytes_after_growth_and_shrink() {
    let mut scene = fixture(3, 4);
    let mut camera = Camera::frame_bbox(Vec3::splat(-6.0), Vec3::splat(6.0), 0.8);
    let mut cameras = Vec::new(); let mut bytes = Vec::new();
    for case in 0..6 {
        scene.molecules[1].visible = case % 2 == 0;
        scene.molecules[0].reps[1].visible = case % 3 == 0;
        camera.projection = if case % 2 == 0 {Projection::Perspective} else {Projection::Orthographic};
        let (fresh, images) = prepare::<true>(&scene, &camera);
        let mut fresh_bytes = Vec::new(); pack_camera_bytes(&fresh, &mut fresh_bytes);
        assert_eq!(images, prepare_reused(&scene, &camera, &mut cameras, &mut bytes));
        assert_eq!(fresh_bytes, bytes);
        assert_eq!(bytemuck::cast_slice::<_,u8>(&fresh), bytemuck::cast_slice::<_,u8>(&cameras));
        let pointers = (cameras.as_ptr(), bytes.as_ptr());
        prepare_reused(&scene, &camera, &mut cameras, &mut bytes);
        assert_eq!(pointers, (cameras.as_ptr(), bytes.as_ptr()));
        for entry in bytes.chunks_exact(CAMERA_STRIDE as usize) {
            assert!(entry[std::mem::size_of::<CameraUniform>()..].iter().all(|&byte| byte == 0));
        }
    }
}

fn assert_visible_cameras(scene: &Scene, camera: &Camera) {
    let (old, before) = prepare::<false>(scene, camera);
    let (new, after) = prepare::<true>(scene, camera);
    assert_eq!(bytemuck::cast_slice::<_,u8>(&old[..2]), bytemuck::cast_slice::<_,u8>(&new[..2]));
    assert_eq!(before.len(), after.len());
    for (mi, mol) in scene.molecules.iter().enumerate() {
        assert_eq!(before[mi].len(), after[mi].len());
        for (ri, rep) in mol.reps.iter().enumerate() {
            if mol.visible && rep.visible {
                assert_eq!(before[mi][ri].len(), after[mi][ri].len());
                for (&a, &b) in before[mi][ri].iter().zip(&after[mi][ri]) {
                    assert_eq!(bytemuck::bytes_of(&old[a as usize]), bytemuck::bytes_of(&new[b as usize]));
                }
            } else { assert!(after[mi][ri].is_empty()); }
        }
    }
}

#[test]
fn hidden_camera_filter_preserves_visible_uniforms_and_indices() {
    let mut scene = fixture(3, 4);
    let mut camera = Camera::frame_bbox(Vec3::splat(-6.0), Vec3::splat(6.0), 0.8);
    for projection in [Projection::Orthographic, Projection::Perspective] {
        camera.projection = projection;
        assert_visible_cameras(&scene, &camera);
        scene.molecules[1].visible = true;
        scene.molecules[0].reps[1].visible = true;
        assert_visible_cameras(&scene, &camera);
        let mol = &mut scene.molecules[2];
        let mut state = mol.data.state().clone(); state.pbox = None;
        mol.data.set_state(state).unwrap();
        assert_visible_cameras(&scene, &camera);
        for mol in &mut scene.molecules { mol.visible = false; }
        assert_visible_cameras(&scene, &camera);
        for mol in &mut scene.molecules { mol.visible = true; }
    }
}

#[test]
#[ignore = "CPU periodic-camera preparation benchmark"]
fn visible_camera_preparation_paired_benchmark() {
    let mut scene = fixture(8, 24);
    let camera = Camera::frame_bbox(Vec3::splat(-6.0), Vec3::splat(6.0), 0.8);
    for case in ["hidden", "visible", "simple"] {
        let hidden = case == "hidden";
        if case == "simple" {
            scene.molecules.truncate(1); scene.molecules[0].reps.truncate(1);
            let mol = &mut scene.molecules[0];
            let mut state = mol.data.state().clone(); state.pbox = None; mol.data.set_state(state).unwrap();
        }
        if !hidden { for mol in &mut scene.molecules { mol.visible = true; for rep in &mut mol.reps { rep.visible = true; } } }
        assert_visible_cameras(&scene, &camera);
        let mut times = [Vec::new(), Vec::new()]; let mut paired = Vec::new();
        let mut cameras = Vec::new(); let mut bytes = Vec::new();
        for iteration in 0..23 {
            let mut elapsed = [0.0;2];
            for mode in if iteration % 2 == 0 {[0,1]} else {[1,0]} {
                let start = std::time::Instant::now();
                for _ in 0..32 {
                    if mode == 0 {
                        let (fresh, images) = prepare::<false>(&scene, &camera);
                        let mut packed = vec![0u8; fresh.len() * CAMERA_STRIDE as usize]; pack_camera_bytes(&fresh, &mut packed);
                        std::hint::black_box((fresh, images, packed));
                    } else {
                        let images = prepare_reused(&scene, &camera, &mut cameras, &mut bytes);
                        std::hint::black_box((&cameras, images, &bytes));
                    }
                }
                elapsed[mode] = start.elapsed().as_secs_f64()*1000.0/32.0;
                if iteration >= 2 { times[mode].push(elapsed[mode]); }
            }
            if iteration >= 2 { paired.push(100.0*(elapsed[1]/elapsed[0]-1.0)); }
        }
        paired.sort_by(f64::total_cmp);
        for (mode, samples) in times.iter_mut().enumerate() {
            samples.sort_by(f64::total_cmp);
            let count = if mode == 0 {prepare::<false>(&scene,&camera).0.len()} else {prepare::<true>(&scene,&camera).0.len()};
            eprintln!("CAMERA_PREP {}", serde_json::json!({"case":case,"hidden":hidden,"mode":mode,"median_ms":samples[10],
                "p95_ms":samples[19],"paired_percent":paired[10],"camera_entries":count,"upload_bytes":count as u64*CAMERA_STRIDE}));
        }
    }
}

#[test]
#[ignore = "short physical-GPU parity check for hidden periodic cameras"]
fn visible_camera_preparation_matches_images() {
    let rs = test_gpu();
    let mut renderer = SceneRenderer::new(&rs, &crate::settings::RenderingSettings::default());
    let mut scene = fixture(2, 2);
    for (mi, mol) in scene.molecules.iter_mut().enumerate() {
        mol.show_box = true;
        let lines = crate::geometry::box_wireframe(mol.data.state().pbox.as_ref().unwrap());
        mol.box_gpu = renderer.upload(&rs, &GeometryData {lines, ..Default::default()});
        for (ri, rep) in mol.reps.iter_mut().enumerate() {
            rep.periodic.neg = [1,0,0]; rep.periodic.pos = [1,0,0];
            rep.material = if ri == 0 {crate::material::Material::Opaque} else {crate::material::Material::Glass};
            let mut geom = GeometryData {spheres:vec![SphereInstance {
                center:[mi as f32 * 0.7,ri as f32 * 0.8,0.0],radius:0.5,
                color:if mi == 0 {0xff8080ff} else {0xff80ff80},mat:0,pick:[0;2],
            }], ..Default::default()};
            crate::geometry::stamp_material(&mut geom, rep.material);
            rep.gpu = renderer.upload(&rs, &geom);
        }
    }
    let mut camera = Camera::frame_bbox(Vec3::new(-3.0,-1.0,-1.0), Vec3::new(5.0,4.0,4.0), 0.8);
    camera.ao.enabled = true; camera.shadow.enabled = true;
    for projection in [Projection::Orthographic, Projection::Perspective] {
        camera.projection = projection;
        for case in 0..4 {
            scene.molecules[1].visible = case >= 2;
            scene.molecules[0].reps[1].visible = case >= 1;
            scene.molecules[0].reps[0].visible = case != 3;
            let mut images = Vec::new();
            for reference in [true, false] {
                renderer.audit_include_hidden_cameras = reference;
                renderer.shadow_key = None;
                let cap = renderer.capture_begin(&rs,160,120,camera.view(),camera.proj(4.0/3.0),
                    camera.is_perspective(),camera.cue_uniform(),camera.ao_uniform(),camera.shadow_uniform(),
                    camera.background,camera.eye_depth_range(),&scene);
                rs.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                let image = cap.read();
                if let Ok(dir) = std::env::var("MOLAR_VIS_TEST_IMAGES") {
                    std::fs::create_dir_all(&dir).unwrap();
                    image.save(format!("{dir}/camera-{projection:?}-{case}-{reference}.png")).unwrap();
                }
                images.push(image);
            }
            assert_eq!(images[0], images[1],"{projection:?} case {case}");
        }
    }
}
