//! A uniform spatial grid of atom positions for **ray-neighborhood** queries — the
//! hover-detail "lens" needs the atoms within a radius of the view *line* (a tube
//! down the line of sight), which `within`/`dist point` can't prune (a line spans
//! the whole box → O(N) brute force). This mirrors the *idea* of molar's
//! distance-search grid (bin atoms into fixed cells) but drops the periodic
//! machinery: build once over a molecule's atoms, then a query walks only the cells
//! the ray's R-tube passes through — O(cells along ray + nearby atoms), not O(N).
//! Pure logic, WASM-safe.

use glam::Vec3;
use std::collections::HashSet;

/// Uniform grid binning atoms by position. Cells store `(atom index, position)` so
/// a query is self-contained (no external coordinate lookup).
pub struct AtomGrid {
    lower: Vec3,
    /// Per-axis cell size (nm). Cells are `extent / dims` so binning is exact.
    cell: Vec3,
    dims: [usize; 3],
    cells: Vec<Vec<(u32, Vec3)>>,
}

impl AtomGrid {
    /// Build over `atoms` (global index + world position) spanning `[min, max]`.
    /// `target_cell` is the desired cell size (nm) — pass it ≈ the query radius so
    /// the R-skirt around the ray is just a cell or two.
    pub fn build(
        atoms: impl Iterator<Item = (u32, Vec3)>,
        min: Vec3,
        max: Vec3,
        target_cell: f32,
    ) -> Self {
        let target_cell = target_cell.max(1e-3);
        let extent = (max - min).max(Vec3::splat(1e-3));
        let dims = [
            ((extent.x / target_cell).floor() as usize).max(1),
            ((extent.y / target_cell).floor() as usize).max(1),
            ((extent.z / target_cell).floor() as usize).max(1),
        ];
        let cell = Vec3::new(
            extent.x / dims[0] as f32,
            extent.y / dims[1] as f32,
            extent.z / dims[2] as f32,
        );
        let mut grid = Self {
            lower: min,
            cell,
            dims,
            cells: vec![Vec::new(); dims[0] * dims[1] * dims[2]],
        };
        for (id, p) in atoms {
            // Clamp into range (atoms are expected within [min,max]; clamp guards
            // float error / slight overshoot rather than dropping atoms).
            let c = [
                (((p.x - min.x) / cell.x).floor() as isize).clamp(0, dims[0] as isize - 1) as usize,
                (((p.y - min.y) / cell.y).floor() as isize).clamp(0, dims[1] as isize - 1) as usize,
                (((p.z - min.z) / cell.z).floor() as isize).clamp(0, dims[2] as isize - 1) as usize,
            ];
            let i = c[0] + c[1] * dims[0] + c[2] * dims[0] * dims[1];
            grid.cells[i].push((id, p));
        }
        grid
    }

    fn flat(&self, c: [isize; 3]) -> Option<usize> {
        for d in 0..3 {
            if c[d] < 0 || c[d] >= self.dims[d] as isize {
                return None;
            }
        }
        Some(c[0] as usize + c[1] as usize * self.dims[0] + c[2] as usize * self.dims[0] * self.dims[1])
    }

    fn cell_of(&self, p: Vec3) -> [isize; 3] {
        [
            ((p.x - self.lower.x) / self.cell.x).floor() as isize,
            ((p.y - self.lower.y) / self.cell.y).floor() as isize,
            ((p.z - self.lower.z) / self.cell.z).floor() as isize,
        ]
    }

    /// Atom indices within `r` of the ray segment `origin + t·dir`, `t ∈ [t_min,
    /// t_max]`. Marches the segment in sub-cell steps, gathering the cells within the
    /// R-skirt of each step (deduped) and testing their atoms' distance to the
    /// segment — so each candidate atom is tested at most once and total work scales
    /// with the tube, not the molecule.
    pub fn atoms_near_ray(
        &self,
        origin: Vec3,
        dir: Vec3,
        r: f32,
        t_min: f32,
        t_max: f32,
    ) -> Vec<u32> {
        let dir = dir.normalize_or_zero();
        if dir == Vec3::ZERO || t_max <= t_min {
            return Vec::new();
        }
        let a = origin + dir * t_min;
        let b = origin + dir * t_max;
        let r2 = r * r;
        let pad = [
            (r / self.cell.x).ceil() as isize,
            (r / self.cell.y).ceil() as isize,
            (r / self.cell.z).ceil() as isize,
        ];
        let step = self.cell.min_element().max(1e-3) * 0.5;
        let n_steps = (((t_max - t_min) / step).ceil() as i64).max(1);

        let mut seen: HashSet<usize> = HashSet::new();
        let mut out: Vec<u32> = Vec::new();
        for s in 0..=n_steps {
            let t = (t_min + step * s as f32).min(t_max);
            let base = self.cell_of(origin + dir * t);
            for dz in -pad[2]..=pad[2] {
                for dy in -pad[1]..=pad[1] {
                    for dx in -pad[0]..=pad[0] {
                        if let Some(ci) = self.flat([base[0] + dx, base[1] + dy, base[2] + dz]) {
                            if seen.insert(ci) {
                                for &(id, p) in &self.cells[ci] {
                                    if dist2_point_seg(p, a, b) <= r2 {
                                        out.push(id);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    }
}

/// Squared distance from point `p` to the segment `[a, b]`.
fn dist2_point_seg(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 <= 1e-12 {
        return (p - a).length_squared();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length_squared()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_of(pts: &[(u32, Vec3)]) -> AtomGrid {
        AtomGrid::build(pts.iter().copied(), Vec3::splat(-5.0), Vec3::splat(5.0), 1.0)
    }

    #[test]
    fn ray_finds_atoms_near_line_only() {
        // Atoms strung along / near the x axis at various perpendicular offsets.
        let pts = [
            (0, Vec3::new(-3.0, 0.0, 0.0)),  // on the axis
            (1, Vec3::new(0.0, 0.2, 0.0)),   // 0.2 off
            (2, Vec3::new(2.0, 0.0, 0.4)),   // 0.4 off
            (3, Vec3::new(1.0, 1.5, 0.0)),   // 1.5 off → excluded at r=0.5
            (4, Vec3::new(0.0, 0.0, 3.0)),   // far off the axis
        ];
        let g = grid_of(&pts);
        // Ray along +x through the origin.
        let mut got = g.atoms_near_ray(Vec3::new(-5.0, 0.0, 0.0), Vec3::X, 0.5, 0.0, 10.0);
        got.sort_unstable();
        assert_eq!(got, vec![0, 1, 2], "only atoms within 0.5 of the x axis");
    }

    #[test]
    fn ray_respects_segment_bounds() {
        let pts = [
            (0, Vec3::new(-3.0, 0.0, 0.0)),
            (1, Vec3::new(3.0, 0.0, 0.0)),
        ];
        let g = grid_of(&pts);
        // Segment only covers x ∈ [-1, +1] worth of the axis → both endpoints excluded.
        let got = g.atoms_near_ray(Vec3::new(0.0, 0.0, 0.0), Vec3::X, 0.5, -1.0, 1.0);
        assert!(got.is_empty(), "atoms beyond the segment are excluded, got {got:?}");
    }

    #[test]
    fn diagonal_ray() {
        let pts = [
            (0, Vec3::new(1.0, 1.0, 1.0)),       // on the diagonal
            (1, Vec3::new(2.0, 2.0, 2.3)),       // ~0.23 off
            (2, Vec3::new(-2.0, 2.0, 0.0)),      // far off
        ];
        let g = grid_of(&pts);
        let mut got = g.atoms_near_ray(Vec3::splat(-4.0), Vec3::ONE, 0.5, 0.0, 20.0);
        got.sort_unstable();
        assert_eq!(got, vec![0, 1]);
    }
}
