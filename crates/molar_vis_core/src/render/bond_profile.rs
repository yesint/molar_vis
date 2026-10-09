//! Cubic Hermite bond flares, tangent to atom spheres and flat at the waist.
/// [left join position, left join radius, right join position, right join radius].
/// Zero disables the profile when a ball is no wider than the requested neck.
pub(crate) fn tangent_joins(length: f32, r0: f32, r1: f32, neck: f32, smoothing: f32) -> [f32; 4] {
    if smoothing <= 0.0
        || !length.is_finite()
        || length <= 1e-6
        || neck <= 0.0
        || r0 <= neck
        || r1 <= neck
    {
        return [0.0; 4];
    }
    let join = |radius: f32| {
        let tip = (radius * radius - neck * neck).sqrt();
        // Shorten the flare towards the ordinary sphere/cylinder intersection.
        // At one this is the previous join-to-midpoint spline; approaching zero
        // confines the tangent transition to the atom end of a straight stick.
        let waist =
            tip.min(length * 0.5) + smoothing.clamp(0.0, 1.0) * (length * 0.5 - tip).max(0.0);
        let limit = tip.min(waist * (1.0 - 1e-7));
        let width = |x: f32| (radius * radius - x * x).max(1e-16).sqrt();
        let outside_sphere = |x: f32| {
            let y = width(x);
            let span = waist - x;
            let curvature = (-6.0 * (y - neck) + 4.0 * x / y * span) / (span * span);
            curvature >= -radius * radius / (y * y * y)
        };
        // Keep the exposed spline outside the sphere after the tangent join.
        // Otherwise the sphere would hide it until a later, non-tangent crossing.
        let mut lo = 0.0;
        let mut hi = limit;
        if !outside_sphere(0.0) {
            for _ in 0..24 {
                let mid = (lo + hi) * 0.5;
                if outside_sphere(mid) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
        }
        let minimum = if outside_sphere(0.0) { 0.0 } else { hi };
        let monotone = |x: f32| {
            let y = width(x);
            x / y <= 3.0 * (y - neck) / (waist - x)
        };
        let mut x = (radius * 0.5).max(minimum).min(limit);
        if !monotone(x) {
            lo = minimum;
            hi = x;
            for _ in 0..24 {
                let mid = (lo + hi) * 0.5;
                if monotone(mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            x = lo;
        }
        if !outside_sphere(x) || !monotone(x) {
            return None;
        }
        Some((x, width(x)))
    };
    let (Some((x0, y0)), Some((x1, y1))) = (join(r0), join(r1)) else {
        return [0.0; 4];
    };
    [x0, y0, length - x1, y1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bond_smoothing_is_optional_and_session_compatible() {
        use crate::geometry::{RepKind, RepParams};
        let legacy: RepParams =
            serde_json::from_str(r#"{"BallAndStick":{"sphere_scale":0.25,"bond_radius":0.015}}"#)
                .unwrap();
        assert_eq!(
            serde_json::to_value(legacy).unwrap(),
            serde_json::to_value(RepParams::for_kind(RepKind::BallAndStick)).unwrap()
        );
        for (enabled, value) in [(true, 1.0), (false, 0.0)] {
            let legacy: RepParams = serde_json::from_str(&format!(r#"{{"BallAndStick":{{"sphere_scale":0.25,"bond_radius":0.015,"smooth_joins":{enabled}}}}}"#)).unwrap();
            assert!(
                matches!(legacy, RepParams::BallAndStick { bond_smoothing, .. } if bond_smoothing == value)
            );
        }
        let enabled = RepParams::BallAndStick {
            sphere_scale: 0.25,
            bond_radius: 0.015,
            bond_smoothing: 0.35,
        };
        let restored: RepParams =
            serde_json::from_str(&serde_json::to_string(&enabled).unwrap()).unwrap();
        assert!(matches!(
            restored,
            RepParams::BallAndStick {
                bond_smoothing: 0.35,
                ..
            }
        ));
    }

    #[test]
    fn spline_joins_stay_outside_atoms_and_never_narrow_below_the_waist() {
        for (length, r0, r1, neck) in [
            (0.15, 0.0425, 0.038, 0.015),
            (0.13, 0.06, 0.055, 0.02),
            (0.3, 0.05, 0.07, 0.02),
            (0.15, 0.051, 0.051, 0.04),
        ] {
            for smoothing in [1.0, 0.75, 0.25, 0.1, 0.01, 0.001] {
                let p = tangent_joins(length, r0, r1, neck, smoothing);
                assert!(
                    p[1] > 0.0,
                    "missing profile: {length} {r0} {r1} at {smoothing}"
                );
                for (x, y, radius) in [(p[0], p[1], r0), (length - p[2], p[3], r1)] {
                    assert!((x * x + y * y - radius * radius).abs() < 1e-7);
                    let tip = (radius * radius - neck * neck).sqrt();
                    let waist = tip + smoothing * (length * 0.5 - tip);
                    let span = waist - x;
                    let slope = -x / y;
                    for i in 0..=100 {
                        let u = i as f32 / 100.0;
                        let value = (2.0 * u * u * u - 3.0 * u * u + 1.0) * y
                            + (u * u * u - 2.0 * u * u + u) * span * slope
                            + (-2.0 * u * u * u + 3.0 * u * u) * neck;
                        let axial = x + u * span;
                        assert!(value >= neck - 1e-6);
                        assert!(
                            value * value + axial * axial >= radius * radius - 1e-7,
                            "spline is hidden inside sphere at {axial}"
                        );
                    }
                }
            }
        }
        assert_eq!(tangent_joins(0.15, 0.0425, 0.038, 0.015, 0.0), [0.0; 4]);
        assert_eq!(tangent_joins(0.15, 0.03, 0.04, 0.04, 1.0), [0.0; 4]);
    }

    #[test]
    #[ignore = "requires native GPU; verifies spline curvature, tangent normals and ray intersections"]
    fn spline_profile_gpu() {
        let rs = crate::render::appearance_tests::gpu();
        for smoothing in [1.0, 0.25, 0.01] {
            let p = tangent_joins(0.15, 0.0425, 0.038, 0.015, smoothing);
            let source = format!(
                r#"
            {}
            @group(0) @binding(0) var<storage, read_write> result: array<vec4<f32>>;
            @compute @workgroup_size(1) fn main() {{
                let p = vec4<f32>({}, {}, {}, {});
                let smoothing: f32 = {smoothing};
                let base = vec3<f32>(0.0); let axis = vec3<f32>(1.0,0.0,0.0);
                let waist = bond_profile_radius(0.075, 0.15, 0.015, p, smoothing);
                let half = bond_profile_radius((p.x+0.075)*0.5, 0.15, 0.015, p, smoothing);
                result[0] = vec4<f32>(waist, half);
                result[1] = vec4<f32>(bond_profile_normal(vec3<f32>(p.x,p.y,0.0),base,axis,0.15,0.015,p,smoothing,vec3<f32>(0.0)), 0.0);
                result[2] = vec4<f32>(bond_profile_normal(vec3<f32>(p.z,p.w,0.0),base,axis,0.15,0.015,p,smoothing,vec3<f32>(0.0)), 0.0);
                result[3] = bond_profile_ray(base,axis,0.15,0.015,p,smoothing,vec3<f32>(0.0),vec3<f32>(0.075,0.1,0.0),vec3<f32>(0.0,-1.0,0.0),false);
                result[4] = bond_profile_ray(base,axis,0.15,0.015,p,smoothing,vec3<f32>(0.0),vec3<f32>(-0.1,0.0,0.0),axis,false);
                result[5] = bond_profile_ray(base,axis,0.15,0.015,p,smoothing,vec3<f32>(0.0),vec3<f32>(0.075,0.0,0.0),vec3<f32>(0.0,1.0,0.0),true);
                result[6] = bond_profile_ray(base,axis,0.15,0.006,p,smoothing,vec3<f32>(0.0,0.018,0.0),vec3<f32>(0.075,0.1,0.0),vec3<f32>(0.0,-1.0,0.0),false);
                result[7] = vec4<f32>(bond_profile_radius(0.06,0.15,0.015,p,smoothing),0.0,0.0);
                result[8] = bond_profile_ray(base,axis,0.15,0.006,p,smoothing,vec3<f32>(0.0,0.018,0.0),vec3<f32>(0.06,0.1,0.0),vec3<f32>(0.0,-1.0,0.0),false);
                for (var i = 0u; i < 512u; i += 1u) {{
                    let angle = f32(i) * 2.39996323;
                    let inside = i % 3u == 0u;
                    let offset = i % 4u == 0u;
                    let shift = select(vec3<f32>(0.0), vec3<f32>(0.0,0.018,0.0), offset);
                    let neck = select(0.015, 0.006, offset);
                    let ro = vec3<f32>(f32(i % 37u) * 0.006 - 0.03, cos(angle), sin(angle)) * vec3<f32>(1.0, select(0.1,0.002,inside),select(0.1,0.002,inside));
                    let rd = normalize(vec3<f32>(sin(angle*0.3)*0.2, -cos(angle), -sin(angle)));
                    let actual = bond_profile_ray_diagnostic(base,axis,0.15,neck,p,smoothing,shift,ro,rd,inside);
                    result[9u+i*3u] = actual.hit;
                    result[10u+i*3u] = bond_profile_ray_reference(base,axis,0.15,neck,p,smoothing,shift,ro,rd,inside);
                    result[11u+i*3u] = vec4<f32>(f32(actual.iterations),0.0,0.0,0.0);
                }}
            }}"#,
                format!(
                    "{}\n{}",
                    include_str!("shaders/bond_profile.wgsl"),
                    include_str!("shaders/bond_profile_ray_reference.wgsl")
                ),
                p[0],
                p[1],
                p[2],
                p[3]
            );
            let shader = rs
                .device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: None,
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
                size: 24720,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let readback = rs.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 24720,
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
            encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 24720);
            rs.queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                tx.send(r).unwrap();
            });
            rs.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let bytes = readback.slice(..).get_mapped_range();
            let v: &[[f32; 4]] = bytemuck::cast_slice(&bytes);
            let mut iterations = Vec::new();
            for i in 0..512 {
                let actual = v[9 + i * 3];
                let reference = v[10 + i * 3];
                assert_eq!(
                    actual[0] < 0.0,
                    reference[0] < 0.0,
                    "hit mismatch ray {i} smoothing {smoothing}"
                );
                for component in 0..4 {
                    assert!(
                        (actual[component] - reference[component]).abs() < 2e-5,
                        "ray {i} smoothing {smoothing}: {actual:?} != {reference:?}"
                    );
                }
                iterations.push(v[11 + i * 3][0] as u32);
            }
            iterations.sort_unstable();
            eprintln!(
                "bond smoothing {smoothing}: mean iterations {:.1}, p95 {}, max {}",
                iterations.iter().sum::<u32>() as f32 / 512.0,
                iterations[486],
                iterations[511]
            );
            assert!((v[0][0] - 0.015).abs() < 1e-6 && v[0][1].abs() < 1e-6);
            assert!(
                (v[0][2] - (p[1] + 0.015) * 0.5).abs() > 1e-4,
                "profile must curve, not widen linearly"
            );
            for (actual, expected) in [
                (v[1], glam::Vec3::new(p[0], p[1], 0.0).normalize()),
                (v[2], glam::Vec3::new(p[2] - 0.15, p[3], 0.0).normalize()),
            ] {
                assert!(
                    (glam::Vec3::new(actual[0], actual[1], actual[2]) - expected).length() < 1e-5
                );
            }
            assert!((v[3][0] - 0.085).abs() < 1e-5, "side ray {:?}", v[3]);
            assert!(
                (v[4][0] - (0.1 + p[0])).abs() < 1e-5,
                "end-on ray {:?}",
                v[4]
            );
            assert!((v[5][0] - 0.015).abs() < 1e-5, "exit ray {:?}", v[5]);
            assert!(
                (v[6][0] - 0.076).abs() < 1e-5,
                "parallel strand must keep its offset: {:?}",
                v[6]
            );
            if smoothing < 0.5 {
                assert!(
                    (v[8][0] - 0.076).abs() < 1e-5,
                    "strand must not bend back to the central axis near the atom: {:?}",
                    v[8]
                );
                assert!(
                    (v[7][0] - 0.015).abs() < 1e-6 && v[7][1].abs() < 1e-6,
                    "sharp join must reach straight radius earlier"
                );
            } else {
                assert!(v[7][0] > 0.016, "one must preserve the broad flare");
            }
        }
    }
}
