//! Aromatic circles fitted to the opening left by a representation's atoms and bonds.
use super::*;
use glam::Vec3;
use std::collections::HashMap;

pub(crate) fn append_aromatic_circles(
    data: &mut GeometryData,
    bound: &impl ParticleIterProvider,
    rings: &[Vec<usize>],
    params: &RepParams,
    material: Material,
) {
    if rings.is_empty() { return; }
    let (bond_radius, sphere_scale, smoothing, line_width) = match *params {
        RepParams::Licorice { bond_radius, .. } => (bond_radius, 0.0, false, None),
        RepParams::BallAndStick { bond_radius, sphere_scale, bond_smoothing, .. } =>
            (bond_radius, sphere_scale, bond_smoothing > 0.0, None),
        RepParams::Lines { width, .. } => (0.0, 0.0, false, Some(width)),
        _ => return,
    };
    let atoms: HashMap<_, _> = bound.iter_particle().map(|p| {
        (p.id, (Vec3::new(p.pos.x, p.pos.y, p.pos.z),
            (p.atom.vdw() * sphere_scale).max(bond_radius)))
    }).collect();
    let color = crate::color::pack_rgba8([180, 180, 180, material.opacity_u8()]);
    for ring in rings {
        // A partial selection must not show a floating circle for an incomplete ring.
        let Some(points) = ring.iter().map(|id| atoms.get(id).copied())
            .collect::<Option<Vec<_>>>() else { continue };
        append_ring(data, &points, bond_radius, smoothing, line_width, color, material);
    }
}

fn append_ring(
    data: &mut GeometryData,
    points: &[(Vec3, f32)],
    bond_radius: f32,
    smoothing: bool,
    line_width: Option<f32>,
    color: u32,
    material: Material,
) {
    if points.len() < 3 || points.iter().any(|(p, r)| !p.is_finite() || !r.is_finite()) {
        return;
    }
    let center = points.iter().map(|(p, _)| *p).sum::<Vec3>() / points.len() as f32;
    let mut normal = Vec3::ZERO;
    let mut clearance = f32::INFINITY;
    for i in 0..points.len() {
        let (a, ra) = points[i];
        let (b, rb) = points[(i + 1) % points.len()];
        normal += (a - center).cross(b - center);
        let edge = b - a;
        if edge.length_squared() < 1e-12 { return; }
        let t = ((center - a).dot(edge) / edge.length_squared()).clamp(0.0, 1.0);
        // Smooth bond flares are bounded by the larger endpoint sphere.
        let edge_radius = if smoothing { bond_radius.max(ra).max(rb) } else { bond_radius };
        clearance = clearance.min(center.distance(a + edge * t) - edge_radius)
            .min(center.distance(a) - ra);
    }
    let normal = normal.normalize_or_zero();
    if normal == Vec3::ZERO || !clearance.is_finite() || clearance <= 1e-5 { return; }
    let radial = points[0].0 - center;
    let u = (radial - normal * radial.dot(normal)).normalize_or_zero();
    if u == Vec3::ZERO { return; }
    let v = normal.cross(u);
    // Reserve an actual gap, including the tube's outer surface. As bonds/balls
    // grow or the ring narrows, both the circle and its tube fit the free opening.
    let tube = if line_width.is_some() { 0.0 } else {
        (bond_radius * 0.3).clamp(0.004, 0.012).min(clearance * 0.18)
    };
    let radius = clearance * 0.85 - tube;
    const SEGMENTS: usize = 64;
    let at = |k: usize| {
        let angle = std::f32::consts::TAU * (k % SEGMENTS) as f32 / SEGMENTS as f32;
        (center + (u * angle.cos() + v * angle.sin()) * radius).to_array()
    };
    for k in 0..SEGMENTS {
        let (p0, p1) = (at(k), at(k + 1));
        if let Some(width) = line_width {
            for pos in [p0, p1] {
                data.lines.push(LineVertex { pos, color, width: width.max(1.6), offset_px: 0.0, cap: 1.0 });
            }
        } else {
            data.cylinders.push(CylinderInstance {
                p0, p1, radius: tube, color, color1: color,
                mat: material.pack_lighting(), offset: [0.0; 2], profile: [0.0; 4],
                smoothing: 0.0, color_blend: 0.0,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polygon(n: usize, scale_y: f32, atom_radius: f32) -> Vec<(Vec3, f32)> {
        (0..n).map(|i| {
            let angle = std::f32::consts::TAU * i as f32 / n as f32;
            (Vec3::new(0.14 * angle.cos(), 0.14 * scale_y * angle.sin(), 0.0), atom_radius)
        }).collect()
    }

    fn tubes(points: &[(Vec3, f32)], bond_radius: f32) -> GeometryData {
        let mut data = GeometryData::default();
        append_ring(&mut data, points, bond_radius, false, None, 0xffffffff, Material::Opaque);
        data
    }

    #[test]
    fn tubes_clear_atoms_and_bonds_in_small_and_distorted_rings() {
        for n in [5, 6, 7] {
            for scale_y in [0.65, 1.0] {
                for bond_radius in [0.015_f32, 0.03, 0.05] {
                    let points = polygon(n, scale_y, bond_radius.max(0.045));
                    let data = tubes(&points, bond_radius);
                    assert!(data.lines.is_empty());
                    assert!(!data.cylinders.is_empty());
                    for (k, tube) in data.cylinders.iter().enumerate() {
                        assert_eq!(tube.p1, data.cylinders[(k + 1) % data.cylinders.len()].p0);
                        assert!(tube.radius > 0.0);
                        for t in [0.0, 0.5, 1.0] {
                            let p = Vec3::from_array(tube.p0).lerp(Vec3::from_array(tube.p1), t);
                            for i in 0..n {
                                let (a, ra) = points[i];
                                let b = points[(i + 1) % n].0;
                                let edge = b - a;
                                let f = ((p - a).dot(edge) / edge.length_squared()).clamp(0.0, 1.0);
                                assert!(p.distance(a) > ra + tube.radius);
                                assert!(p.distance(a + edge * f) > bond_radius + tube.radius);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn circle_shrinks_for_thicker_bonds_and_larger_balls() {
        let radius = |data: GeometryData| Vec3::from_array(data.cylinders[0].p0).length();
        let thin = radius(tubes(&polygon(6, 1.0, 0.03), 0.015));
        let thick = radius(tubes(&polygon(6, 1.0, 0.05), 0.05));
        let balls = radius(tubes(&polygon(6, 1.0, 0.09), 0.015));
        assert!(thick < thin);
        assert!(balls < thick);
        assert!(tubes(&polygon(6, 1.0, 0.15), 0.015).cylinders.is_empty());
    }

    #[test]
    fn lines_and_degenerate_rings_are_handled() {
        let mut data = GeometryData::default();
        append_ring(&mut data, &polygon(6, 1.0, 0.0), 0.0, false, Some(2.0), 0xffffffff, Material::Opaque);
        assert!(!data.lines.is_empty());
        assert!(data.cylinders.is_empty());
        for points in [vec![], vec![(Vec3::ZERO, 0.03); 6], polygon(6, 0.0, 0.03)] {
            assert!(tubes(&points, 0.03).cylinders.is_empty());
        }
    }

    #[test]
    fn circles_follow_selection_and_material() {
        let raw = crate::data::load_records(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"),
            "/../../tests/ligands20.sdf")), &Default::default()).unwrap().remove(0);
        let mut scene = crate::scene::Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        let mol = &mut scene.molecules[0];
        mol.perceive_aromaticity();
        assert_eq!(mol.aromatic_rings.len(), 1);
        let params = RepParams::for_kind(RepKind::BallAndStick);
        for complete in [true, false] {
            let sel = if complete { mol.data.select_all() } else {
                mol.data.evaluate(&format!("not index {}", mol.aromatic_rings[0][0])).unwrap().1
            };
            let bound = mol.data.bind_with_state(&sel, mol.render_state());
            let mut data = GeometryData::default();
            append_aromatic_circles(&mut data, &bound, &mol.aromatic_rings, &params, Material::Transparent);
            assert_eq!(!data.cylinders.is_empty(), complete);
            for tube in data.cylinders {
                assert_eq!(tube.color >> 24, Material::Transparent.opacity_u8() as u32);
                assert_eq!(tube.mat, Material::Transparent.pack_lighting());
            }
        }
    }
}
