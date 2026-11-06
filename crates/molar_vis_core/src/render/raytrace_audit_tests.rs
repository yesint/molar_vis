//! Differential tests retain the original repeated-resolve policy as a reference.
use super::*;
use crate::{camera::Camera, render::{SceneRenderer, SphereInstance}};

#[test]
#[ignore = "physical GPU performance and progressive image parity"]
fn progressive_resolve_parity_and_performance() {
    let rs = crate::render::test_gpu();
    assert_ne!(rs.adapter.get_info().device_type, wgpu::DeviceType::Cpu);
    let renderer = SceneRenderer::new(&rs, &crate::settings::RenderingSettings::default());
    let spheres: Vec<_> = (0..9).map(|i| SphereInstance {
        center: [(i % 3) as f32 - 1.0, (i / 3) as f32 - 1.0, (i % 2) as f32 * 0.3],
        radius: 0.55, color: 0xffa060d0,
        mat: crate::material::Material::Opaque.pack_lighting(), pick: [0, i],
    }).collect();
    let scene = RtScene::from_test_mesh_and_spheres(&Default::default(),
        crate::geometry::RepKind::Vdw, &spheres);
    let mut traces = [Raytracer::new(&rs, renderer.color_format).unwrap(),
        Raytracer::new(&rs, renderer.color_format).unwrap()];
    for rt in &mut traces { rt.upload(&rs, &scene); }
    let mut camera = Camera::frame_bbox(Vec3::splat(-1.6), Vec3::splat(1.6), 0.8);
    let texture = |size: [u32; 2]| rs.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("resolve-audit"),
        size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: renderer.color_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let read = |tex: &wgpu::Texture, size| {
        let cap = renderer.begin_readback(&rs, tex, size, size);
        rs.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        cap.read()
    };
    // Restart and resize the same tracers, exercise both projections and GI,
    // and change destination twice during an incomplete second sweep.
    for (case, (size, perspective, gi)) in [([256,192],false,0.0),
        ([192,128],true,0.0), ([256,192],true,1.0)].into_iter().enumerate() {
        camera.projection = if perspective { crate::camera::Projection::Perspective }
            else { crate::camera::Projection::Orthographic };
        camera.ao.enabled = true;
        camera.shadow.enabled = true;
        let uniform = SceneRenderer::rt_uniform(&camera, size[0], size[1], 2, 0, gi, 2048);
        let targets = [[texture(size), texture(size)], [texture(size), texture(size)]];
        let views = targets.each_ref().map(|pair| pair.each_ref().map(|t| t.create_view(&Default::default())));
        for rt in &mut traces {
            rt.trace_begin(&rs, size, uniform, 2);
            let cur = rt.cursor.as_mut().unwrap();
            cur.tile_size = 64;
            cur.chunk_cap = 1;
        }
        let tiles = size[0].div_ceil(64) * size[1].div_ceil(64);
        let mut step = 0;
        loop {
            let destination = usize::from(step == tiles);
            let prior_samples = traces[1].samples();
            let prior_resolves = traces[1].resolve_count.get();
            // Clear only the resolve memo to reproduce the original pass policy.
            traces[0].last_resolved = None;
            let reference_done = traces[0].trace_step(&rs, &views[0][destination], 1);
            let done = traces[1].trace_step(&rs, &views[1][destination], 1);
            assert_eq!(reference_done, done);
            assert_eq!(traces[0].samples(), traces[1].samples());
            let destination_changed = step == tiles || step == tiles + 1;
            let expected = u32::from(traces[1].samples() > 0
                && (traces[1].samples() != prior_samples || destination_changed));
            assert_eq!(traces[1].resolve_count.get() - prior_resolves, expected);
            if traces[1].samples() > 0 {
                let before = read(&targets[0][destination], size);
                let after = read(&targets[1][destination], size);
                assert_eq!(before, after, "case {case}, step {step}");
                if done {
                    if let Ok(dir) = std::env::var("MOLAR_VIS_TEST_IMAGES") {
                        std::fs::create_dir_all(&dir).unwrap();
                        before.save(format!("{dir}/rt-{case}-before.png")).unwrap();
                        after.save(format!("{dir}/rt-{case}-after.png")).unwrap();
                    }
                }
            }
            step += 1;
            if done { break; }
        }
        assert_eq!(traces[1].samples(), 2);
        for rt in &mut traces {
            rt.trace_begin(&rs, size, uniform, 2);
            rt.trace_step(&rs, &views[0][0], 1);
            rt.trace_cancel();
            let count = rt.resolve_count.get();
            assert!(rt.trace_step(&rs, &views[0][0], 1));
            assert_eq!(rt.resolve_count.get(), count);
            // A single call can complete all tiles and must resolve exactly once.
            rt.trace_begin(&rs, size, uniform, 1);
            assert!(rt.trace_step(&rs, &views[0][0], u32::MAX));
            assert_eq!(rt.resolve_count.get(), count + 1);
        }
    }
    camera.ao.enabled = false;
    camera.shadow.enabled = false;
    camera.projection = crate::camera::Projection::Orthographic;
    // Actual scheduling, viewport-like four-tile calls and blocking-export batches.
    for (size, batch) in [([1536,1024],4), ([1536,1024],32), ([256,192],32)] {
        let targets = [texture(size), texture(size)];
        let views = targets.each_ref().map(|t| t.create_view(&Default::default()));
        let uniform = SceneRenderer::rt_uniform(&camera, size[0], size[1], 64, 0, 0.0, 2048);
        let mut times = [Vec::new(), Vec::new()];
        for iteration in 0..14 {
            for mode in if iteration % 2 == 0 { [0,1] } else { [1,0] } {
                let rt = &mut traces[mode];
                let count = rt.resolve_count.get();
                let start = std::time::Instant::now();
                rt.trace_begin(&rs, size, uniform, 64);
                loop {
                    if mode == 0 { rt.last_resolved = None; }
                    let done = rt.trace_step(&rs, &views[mode], batch);
                    rs.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                    if done { break; }
                }
                if iteration >= 3 { times[mode].push(start.elapsed().as_secs_f64()*1000.0); }
                assert_eq!(rt.samples(), 64);
                if iteration == 13 { eprintln!("RESOLVES batch={batch} mode={mode}: {}", rt.resolve_count.get()-count); }
            }
            assert_eq!(read(&targets[0], size), read(&targets[1], size));
        }
        for (mode, samples) in times.iter_mut().enumerate() {
            samples.sort_by(f64::total_cmp);
            eprintln!("RESOLVE_BENCH {}", serde_json::json!({"size":size,"batch":batch,"mode":mode,
                "median_ms":samples[5],"p95_ms":samples[10],"samples_ms":samples}));
        }
    }
}
