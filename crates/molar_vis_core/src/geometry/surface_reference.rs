//! Audit baseline from 10c1d8c, retained only for differential tests.
use super::*;

pub(super) fn relax_on_field(mesh: &mut MeshData, field: &[f32], dims: [usize; 3], origin: Vec3, h: f32) {
    let mut sums = vec![Vec3::ZERO; mesh.vertices.len()];
    let mut counts = vec![0u32; mesh.vertices.len()];
    for _ in 0..2 {
        sums.fill(Vec3::ZERO);
        counts.fill(0);
        for tri in mesh.indices.chunks_exact(3) {
            for &(i, j) in &[
                (tri[0], tri[1]),
                (tri[0], tri[2]),
                (tri[1], tri[0]),
                (tri[1], tri[2]),
                (tri[2], tri[0]),
                (tri[2], tri[1]),
            ] {
                sums[i as usize] += Vec3::from_array(mesh.vertices[j as usize].pos);
                counts[i as usize] += 1;
            }
        }
        for (i, v) in mesh.vertices.iter_mut().enumerate() {
            if counts[i] == 0 {
                continue;
            }
            let old = (Vec3::from_array(v.pos) - origin) / h;
            let average = (sums[i] / counts[i] as f32 - origin) / h;
            let mut p = old.lerp(average, 0.5);
            for _ in 0..3 {
                let (value, g) = sample_field_gradient(field, dims, p);
                if g.length_squared() < 1e-12 {
                    break;
                }
                p -= g * (value / g.length_squared());
                // Bound displacement to protect thin necks and prevent topology changes.
                p = old + (p - old).clamp_length_max(0.5);
            }
            v.pos = (origin + p * h).to_array();
            v.normal = (-sample_gradient(field, dims, p))
                .normalize_or_zero()
                .to_array();
        }
    }
}

/// Refine edges whose normal angle or distance from the continuous field exceeds
/// the quality tolerance. A shared edge decision and conforming 0/1/2/3-edge splits
/// keep neighboring triangles connected without forcing subdivision of flat regions.
pub(super) fn refine_on_field(
    mesh: &mut MeshData,
    field: &[f32],
    dims: [usize; 3],
    origin: Vec3,
    h: f32,
    quality: u32,
) {
    let quality = quality.min(4) as usize;
    let cosine = [40.0_f32, 30.0, 22.0, 16.0, 12.0][quality]
        .to_radians()
        .cos();
    let tolerance = h * [0.12, 0.10, 0.08, 0.06, 0.04][quality];
    let before = mesh.indices.len() / 3;
    let mut midpoints = std::collections::HashMap::new();
    let mut indices = Vec::with_capacity(mesh.indices.len());
    for tri in mesh.indices.chunks_exact(3) {
        let mut edge = |a: u32, b: u32| -> Option<u32> {
            *midpoints.entry((a.min(b), a.max(b))).or_insert_with(|| {
                let va = mesh.vertices[a as usize];
                let vb = mesh.vertices[b as usize];
                let pa = Vec3::from_array(va.pos);
                let pb = Vec3::from_array(vb.pos);
                if pa.distance_squared(pb) < (0.25 * h).powi(2) {
                    return None;
                }
                let initial = ((pa + pb) * 0.5 - origin) / h;
                let (value, gradient) = sample_field_gradient(field, dims, initial);
                let error = value.abs() * h / gradient.length().max(1e-12);
                let curved = Vec3::from_array(va.normal).dot(Vec3::from_array(vb.normal)) < cosine;
                if !curved && error <= tolerance {
                    return None;
                }
                let mut p = initial;
                for _ in 0..3 {
                    let (value, g) = sample_field_gradient(field, dims, p);
                    if g.length_squared() < 1e-12 {
                        break;
                    }
                    p -= g * (value / g.length_squared());
                    p = initial + (p - initial).clamp_length_max(0.5);
                }
                let color = (0..4).fold(0, |packed, c| {
                    let value =
                        (((va.color >> (c * 8)) & 255) + ((vb.color >> (c * 8)) & 255) + 1) / 2;
                    packed | (value << (c * 8))
                });
                let id = mesh.vertices.len() as u32;
                mesh.vertices.push(MeshVertex {
                    pos: (origin + p * h).to_array(),
                    normal: (-sample_gradient(field, dims, p))
                        .normalize_or_zero()
                        .to_array(),
                    color,
                    mat: va.mat,
                });
                if !mesh.vert_atom.is_empty() {
                    mesh.vert_atom.push(mesh.vert_atom[a as usize]);
                }
                Some(id)
            })
        };
        let [a, b, c] = [tri[0], tri[1], tri[2]];
        let ab = edge(a, b);
        let bc = edge(b, c);
        let ca = edge(c, a);
        match (ab, bc, ca) {
            (None, None, None) => indices.extend_from_slice(&[a, b, c]),
            (Some(m), None, None) => indices.extend_from_slice(&[a, m, c, m, b, c]),
            (None, Some(m), None) => indices.extend_from_slice(&[b, m, a, m, c, a]),
            (None, None, Some(m)) => indices.extend_from_slice(&[c, m, b, m, a, b]),
            (Some(ab), Some(bc), None) => {
                indices.extend_from_slice(&[ab, b, bc]);
                refine_quad(&mesh.vertices, [a, ab, bc, c], &mut indices);
            }
            (None, Some(bc), Some(ca)) => {
                indices.extend_from_slice(&[bc, c, ca]);
                refine_quad(&mesh.vertices, [b, bc, ca, a], &mut indices);
            }
            (Some(ab), None, Some(ca)) => {
                indices.extend_from_slice(&[ca, a, ab]);
                refine_quad(&mesh.vertices, [c, ca, ab, b], &mut indices);
            }
            (Some(ab), Some(bc), Some(ca)) => {
                indices.extend_from_slice(&[a, ab, ca, ab, b, bc, ca, bc, c, ab, bc, ca])
            }
        }
    }
    mesh.indices = indices;
    if std::env::var("MOLAR_VIS_DEBUG_SURF").is_ok() {
        log::info!(
            "Surface adaptive refinement: {before} -> {} triangles (quality {quality})",
            mesh.indices.len() / 3
        );
        #[cfg(test)]
        eprintln!(
            "Surface adaptive refinement: {before} -> {} triangles (quality {quality})",
            mesh.indices.len() / 3
        );
    }
}

