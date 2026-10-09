//! Molecular-surface builder: the solvent-excluded surface (SES, rolling-probe) via
//! a **grid distance field + Surface Nets** — the robust, watertight-by-construction
//! method used by PyMOL/Chimera/EDTSurf (distance maps + carving), rather than
//! analytic patch stitching. This is a grid approximation of the SES.
//!
//! Algorithm (morphological closing of the vdW balls by the probe):
//! 1. Rasterize the **SAS solid** onto a grid: a voxel is "inside" if it lies within
//!    `vdW_i + probe` of some atom.
//! 2. Compute the exact Euclidean distance from every inside voxel to the nearest
//!    **outside** (solvent) voxel (Felzenszwalb–Huttenlocher separable EDT). That
//!    distance is exactly `dist(x, solvent)`, so the SES solid is `{x : that ≥
//!    probe}` — the probe rolled over the atoms.
//! 3. Extract the isosurface `dist − probe = 0` with **Surface Nets** (a dual
//!    marching-cubes that yields one vertex per straddling cell → watertight,
//!    smooth, no lookup tables).
//! Per-vertex normals come from the field gradient. Colors start as the nearest
//! atom's color, then are **Laplacian-smoothed along the mesh** (1-ring averaging)
//! so the hard nearest-atom Voronoi patches become smooth gradients — smoothing
//! along the surface rather than through 3-D space, so colors don't bleed across a
//! crevice. A baseline field filter removes voxel noise; mesh relaxation is projected
//! back to the field to preserve size. Normals use analytic gradients of the cubic field at the
//! relaxed vertex positions, rather than smoothing shading over rough geometry.

use glam::Vec3;
use molar::prelude::*;

use crate::color::Colorizer;
use crate::geometry::MeshData;
use crate::render::MeshVertex;

/// Cap on total grid voxels; above it the spacing is coarsened so a huge system
/// can't exhaust memory.
const MAX_VOXELS: usize = 32_000_000;

/// Grid spacing (nm) for each `quality` level (0 = coarse/fast, 4 = fine/smooth).
fn spacing_for(quality: u32) -> f32 {
    match quality {
        0 => 0.14,
        1 => 0.10,
        2 => 0.07,
        3 => 0.05,
        _ => 0.035,
    }
}

/// Build the SES mesh for the bound selection. `probe` is the rolling-probe radius
/// (nm); `quality` selects the grid resolution.
pub fn build<S>(
    bound: &S,
    colorizer: &Colorizer,
    probe: f32,
    quality: u32,
    smoothing: u32,
) -> MeshData
where
    S: ParticleIterProvider + PosProvider + AtomProvider,
{
    Input::new(bound, colorizer, probe, quality, smoothing).build(|| false)
}

/// Owned numeric input: no borrowed molecule/provider crosses the worker boundary.
pub(crate) struct Input {
    centers: Vec<Vec3>, radii: Vec<f32>, colors: Vec<u32>, ids: Vec<u32>,
    probe: f32, quality: u32, smoothing: u32,
}
impl Input {
    pub(crate) fn new(bound: &impl ParticleIterProvider, colorizer: &Colorizer,
        probe: f32, quality: u32, smoothing: u32) -> Self {
    // Gather atom spheres (SAS radius = vdW + probe) + per-atom color.
    let mut centers: Vec<Vec3> = Vec::new();
    let mut radii: Vec<f32> = Vec::new();
    let mut colors: Vec<u32> = Vec::new();
    // Global atom index of each sphere (the `nearest` voxel labels are local indices).
    let mut ids: Vec<u32> = Vec::new();
    for p in bound.iter_particle() {
        centers.push(Vec3::new(p.pos.x, p.pos.y, p.pos.z));
        ids.push(p.id as u32);
        let r = p.atom.vdw() + probe;
        radii.push(r);
        colors.push(colorizer.color(p.atom, p.id));
    }
        Self { centers, radii, colors, ids, probe, quality, smoothing }
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn atom_count(&self) -> usize { self.centers.len() }
    pub(crate) fn build(self, cancelled: impl Fn() -> bool) -> MeshData {
        let Self { centers, radii, colors, ids, probe, quality, smoothing } = self;
        if cancelled() { return MeshData::default(); }
    if centers.is_empty() {
        return MeshData::default();
    }

    // Grid bounds: atom-sphere extent plus a margin so the surface isn't clipped.
    let mut lo = centers[0];
    let mut hi = centers[0];
    for (c, &r) in centers.iter().zip(&radii) {
        lo = lo.min(*c - Vec3::splat(r));
        hi = hi.max(*c + Vec3::splat(r));
    }
    let pad = Vec3::splat(3.0 * spacing_for(quality));
    lo -= pad;
    hi += pad;
    let extent = hi - lo;

    // Spacing, coarsened if the voxel count would exceed the cap.
    let mut h = spacing_for(quality);
    let dims_at = |h: f32| {
        [
            (extent.x / h).ceil() as usize + 1,
            (extent.y / h).ceil() as usize + 1,
            (extent.z / h).ceil() as usize + 1,
        ]
    };
    loop {
        let d = dims_at(h);
        if d[0].saturating_mul(d[1]).saturating_mul(d[2]) <= MAX_VOXELS {
            break;
        }
        h *= 1.3;
    }
    if h > spacing_for(quality) * 1.001 {
        log::warn!("Surface: coarsened grid spacing to {h:.3} nm to bound voxel count");
    }
    let dims = dims_at(h);
    let (nx, ny, nz) = (dims[0], dims[1], dims[2]);
    let n = nx * ny * nz;
    let idx = |x: usize, y: usize, z: usize| x + nx * (y + ny * z);

    // --- Pass 1: SAS occupancy + nearest atom per voxel (seeds the initial vertex
    // color, which is then Laplacian-smoothed along the mesh). ---
    let mut inside = vec![false; n];
    let mut nearest = vec![u32::MAX; n];
    let mut best_d2 = vec![f32::INFINITY; n];
    for (a, (&c, &r)) in centers.iter().zip(&radii).enumerate() {
        if a % 256 == 0 && cancelled() { return MeshData::default(); }
        // Voxel range covering this atom's SAS sphere.
        let vlo = ((c - Vec3::splat(r) - lo) / h).floor();
        let vhi = ((c + Vec3::splat(r) - lo) / h).ceil();
        let x0 = (vlo.x.max(0.0) as usize).min(nx - 1);
        let y0 = (vlo.y.max(0.0) as usize).min(ny - 1);
        let z0 = (vlo.z.max(0.0) as usize).min(nz - 1);
        let x1 = (vhi.x.max(0.0) as usize).min(nx - 1);
        let y1 = (vhi.y.max(0.0) as usize).min(ny - 1);
        let z1 = (vhi.z.max(0.0) as usize).min(nz - 1);
        let r2 = r * r;
        for z in z0..=z1 {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let p = lo + Vec3::new(x as f32, y as f32, z as f32) * h;
                    let d2 = (p - c).length_squared();
                    let i = idx(x, y, z);
                    if d2 <= r2 {
                        inside[i] = true;
                    }
                    // Nearest atom (by center) for the initial coloring.
                    if d2 < best_d2[i] {
                        best_d2[i] = d2;
                        nearest[i] = a as u32;
                    }
                }
            }
        }
    }

    drop(best_d2);

    // --- Pass 2: exact EDT from each inside voxel to the nearest outside voxel. ---
    // Seed: 0 at outside (feature) voxels, +inf at inside; the separable transform
    // gives distance to solvent voxel centers. Subtract the half-cell boundary offset
    // and probe radius to form the SES level set.
    let big = (nx * nx + ny * ny + nz * nz) as f32 + 1.0;
    let mut g: Vec<f32> = (0..n).map(|i| if inside[i] { big } else { 0.0 }).collect();
    drop(inside);
    if cancelled() { return MeshData::default(); }
    edt_3d(&mut g, nx, ny, nz);
    // The EDT measures to solvent voxel centers, half a cell beyond the boundary.
    // Correct that offset; retain negative values outside even for a zero-radius probe.
    let mut field = g;
    for d2 in &mut field {
        *d2 = d2.sqrt() * h - 0.5 * h - probe;
    }

    // Light separable [1,2,1] blur of the distance field: the binary occupancy makes
    // the EDT (and its gradient = our normals) stair-step at voxel resolution, which
    // reads as a rugged/faceted surface. Blurring the field removes that
    // high-frequency noise so both the extracted isosurface and the shading come out
    // smooth, at O(voxels) cost. The slider adds passes to the baseline reconstruction.
    // One reconstruction pass removes binary-occupancy stair steps even at smoothing 0.
    // The control adds further filtering, rather than relying on shading to hide them.
    if cancelled() { return MeshData::default(); }
    smooth_field(&mut field, dims, 1 + smoothing as usize);

    // --- Pass 3: Surface Nets isosurface at field = 0 (vertices seeded with the
    // nearest-atom color). ---
    if cancelled() { return MeshData::default(); }
    let mut mesh = surface_nets(&field, &nearest, &colors, &ids, dims, lo, h);

    // Laplacian-smooth the mesh: the nearest-atom coloring is patchy (Voronoi
    // cells), so spread it along the surface into smooth gradients. Iteration counts scale with grid
    // resolution so the *physical* smoothing distance stays roughly constant.
    let uniform_color = colors.iter().all(|&c| c == colors[0]);
    let color_iters = if uniform_color {
        0
    } else {
        ((0.2 / h * (0.2 / h)).round() as usize).clamp(4, 64)
    };
    if cancelled() { return MeshData::default(); }
    relax_on_field(&mut mesh, &field, dims, lo, h);
    laplacian_smooth(&mut mesh, color_iters);
    if cancelled() { return MeshData::default(); }
    refine_on_field(&mut mesh, &field, dims, lo, h, quality);
    mesh
    }
}

/// Blend colors along mesh edges so nearest-atom patches become continuous gradients
/// without bleeding across spatially close but disconnected surface regions.
fn laplacian_smooth(mesh: &mut MeshData, color_iters: usize) {
    if mesh.vertices.is_empty() || mesh.indices.len() < 3 {
        return;
    }
    if color_iters > 0 {
        let mut rgb: Vec<[f32; 3]> = mesh
            .vertices
            .iter()
            .map(|v| {
                [
                    (v.color & 0xff) as f32,
                    ((v.color >> 8) & 0xff) as f32,
                    ((v.color >> 16) & 0xff) as f32,
                ]
            })
            .collect();
        smooth_attr(&mut rgb, &mesh.indices, color_iters);
        for (v, c) in mesh.vertices.iter_mut().zip(&rgb) {
            let q = |x: f32| x.round().clamp(0.0, 255.0) as u32;
            v.color = q(c[0]) | (q(c[1]) << 8) | (q(c[2]) << 16) | (0xff << 24);
        }
    }
}

/// One-ring averaging of a per-vertex `vec3` attribute, `iters` passes; neighbors are
/// gathered from triangle edges. The surface is closed (every edge shared by two
/// triangles), so each neighbor is counted exactly twice — a uniform weighting.
/// Scratch buffers are reused across passes.
fn smooth_attr(attr: &mut [[f32; 3]], indices: &[u32], iters: usize) {
    let n = attr.len();
    let mut sum = vec![[0.0f32; 3]; n];
    let mut cnt = vec![0u32; n];
    // Connectivity is fixed throughout the color filter. Count the two
    // triangle-mates once per vertex incidence, retaining duplicate weights.
    for tri in indices.chunks_exact(3) {
        for &vertex in tri {
            cnt[vertex as usize] += 2;
        }
    }
    let inv_count: Vec<f32> = cnt
        .into_iter()
        .map(|n| if n == 0 { 0.0 } else { 1.0 / n as f32 })
        .collect();
    for _ in 0..iters {
        sum.iter_mut().for_each(|s| *s = [0.0; 3]);
        for tri in indices.chunks_exact(3) {
            let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            // Each vertex accumulates its two triangle-mates.
            for &(i, j) in &[(a, b), (a, c), (b, a), (b, c), (c, a), (c, b)] {
                sum[i][0] += attr[j][0];
                sum[i][1] += attr[j][1];
                sum[i][2] += attr[j][2];
            }
        }
        for v in 0..n {
            if inv_count[v] > 0.0 {
                let inv = inv_count[v];
                attr[v] = [sum[v][0] * inv, sum[v][1] * inv, sum[v][2] * inv];
            }
        }
    }
}

/// Separable [1,2,1]/4 blur of a scalar grid, applied `passes` times along each
/// axis (edges clamped). Cheap (O(voxels·passes)); smooths the distance field so the
/// extracted surface and its gradient normals lose the voxel-staircase ruggedness.
fn smooth_field(field: &mut [f32], dims: [usize; 3], passes: usize) {
    smooth_field_impl(field, dims, passes, true);
}
fn smooth_field_impl(field: &mut [f32], dims: [usize; 3], passes: usize, allow_parallel: bool) {
    let _timing = crate::performance::span("surface-field-smoothing");
    if passes == 0 {
        return;
    }
    let (nx, ny, nz) = (dims[0], dims[1], dims[2]);
    let idx = |x: usize, y: usize, z: usize| x + nx * (y + ny * z);
    let mut line = vec![0.0f32; nx.max(ny).max(nz)];
    let blur = |line: &[f32], i: usize, len: usize| {
        let a = line[i.saturating_sub(1)];
        let b = line[i];
        let c = line[(i + 1).min(len - 1)];
        (a + 2.0 * b + c) * 0.25
    };
    #[cfg(not(target_arch = "wasm32"))]
    if allow_parallel && use_parallel_z(field.len(), nx * ny, nz) {
        use rayon::prelude::*;
        let mut transposed = vec![0.0; field.len()];
        for _ in 0..passes {
            field.par_chunks_mut(nx * ny).for_each_init(|| vec![0.0; nx.max(ny)], |line, plane| {
                for row in plane.chunks_mut(nx) {
                    line[..nx].copy_from_slice(row);
                    for x in 0..nx { row[x] = blur(line, x, nx); }
                }
                for x in 0..nx {
                    for y in 0..ny { line[y] = plane[y * nx + x]; }
                    for y in 0..ny { plane[y * nx + x] = blur(line, y, ny); }
                }
            });
            parallel_z_lines(field, nx * ny, nz, &mut transposed, |row, scratch| {
                scratch.line[..nz].copy_from_slice(row);
                for z in 0..nz { row[z] = blur(&scratch.line, z, nz); }
            });
        }
        return;
    }
    #[cfg(target_arch = "wasm32")]
    let _ = allow_parallel;
    for _ in 0..passes {
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    line[x] = field[idx(x, y, z)];
                }
                for x in 0..nx {
                    field[idx(x, y, z)] = blur(&line, x, nx);
                }
            }
        }
        for z in 0..nz {
            for x in 0..nx {
                for y in 0..ny {
                    line[y] = field[idx(x, y, z)];
                }
                for y in 0..ny {
                    field[idx(x, y, z)] = blur(&line, y, ny);
                }
            }
        }
        for y in 0..ny {
            for x in 0..nx {
                for z in 0..nz {
                    line[z] = field[idx(x, y, z)];
                }
                for z in 0..nz {
                    field[idx(x, y, z)] = blur(&line, z, nz);
                }
            }
        }
    }
}

/// In-place exact Euclidean distance transform (squared) by Felzenszwalb &
/// Huttenlocher: a 1-D parabola lower-envelope transform applied along x, then y,
/// then z. Input `g` holds the seed (0 at features, large elsewhere); output holds
/// the squared distance (in voxel units) to the nearest feature.
fn edt_3d(g: &mut [f32], nx: usize, ny: usize, nz: usize) {
    edt_3d_impl(g, nx, ny, nz, true);
}
fn edt_3d_impl(g: &mut [f32], nx: usize, ny: usize, nz: usize, allow_parallel_z: bool) {
    let _timing = crate::performance::span("surface-edt");
    let plane_len = nx * ny;
    // X and Y transforms are independent between XY planes. Combining them per
    // plane retains the arithmetic order along each line and improves locality.
    #[cfg(not(target_arch = "wasm32"))]
    let parallel = g.len() >= 262_144 && nz > 1;
    #[cfg(target_arch = "wasm32")]
    let parallel = false;
    #[cfg(not(target_arch = "wasm32"))]
    if parallel {
        use rayon::prelude::*;
        g.par_chunks_mut(plane_len).for_each_init(
            || EdtScratch::new(nx.max(ny)),
            |scratch, plane| scratch.xy(plane, nx, ny),
        );
    }
    if !parallel {
        let mut scratch = EdtScratch::new(nx.max(ny));
        for plane in g.chunks_mut(plane_len) {
            scratch.xy(plane, nx, ny);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    if allow_parallel_z && use_parallel_z(g.len(), plane_len, nz) {
        let mut transposed = vec![0.0; g.len()];
        parallel_z_lines(g, plane_len, nz, &mut transposed, |line, scratch| {
            scratch.line[..nz].copy_from_slice(line);
            scratch.transform(nz);
            line.copy_from_slice(&scratch.d[..nz]);
        });
        return;
    }
    #[cfg(target_arch = "wasm32")]
    let _ = allow_parallel_z;
    // Small grids retain the low-memory strided serial path.
    let mut scratch = EdtScratch::new(nz);
    for xy in 0..plane_len {
        for z in 0..nz {
            scratch.line[z] = g[xy + plane_len * z];
        }
        scratch.transform(nz);
        for z in 0..nz {
            g[xy + plane_len * z] = scratch.d[z];
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn use_parallel_z(voxels: usize, plane: usize, nz: usize) -> bool {
    // One shared transpose volume, never one volume per worker. Cap extra scratch.
    (1_048_576..=67_108_864).contains(&voxels) && plane >= 64 && nz >= 16
        && rayon::current_num_threads() > 1
}

#[cfg(not(target_arch = "wasm32"))]
fn parallel_z_lines(
    grid: &mut [f32], plane_len: usize, nz: usize, transposed: &mut [f32],
    transform: impl Fn(&mut [f32], &mut EdtScratch) + Sync,
) {
    use rayon::prelude::*;
    // Gather in XY tiles, keeping each worker's transpose writes cache-local.
    transposed.par_chunks_mut(nz * 64).enumerate().for_each_init(
        || EdtScratch::new(nz),
        |scratch, (tile, output)| {
            let count = output.len() / nz;
            for z in 0..nz {
                for xy in 0..count { output[xy * nz + z] = grid[tile * 64 + xy + z * plane_len]; }
            }
            for line in output.chunks_mut(nz) { transform(line, scratch); }
        },
    );
    // Each worker owns whole Z slabs; no overlapping mutable strided writes.
    grid.par_chunks_mut(plane_len * 16).enumerate().for_each(|(slab, output)| {
        let count = output.len() / plane_len;
        for xy in 0..plane_len {
            for z in 0..count { output[z * plane_len + xy] = transposed[xy * nz + slab * 16 + z]; }
        }
    });
}

struct EdtScratch {
    line: Vec<f32>,
    d: Vec<f32>,
    v: Vec<usize>,
    boundaries: Vec<f32>,
}

impl EdtScratch {
    fn new(len: usize) -> Self {
        Self {
            line: vec![0.0; len],
            d: vec![0.0; len],
            v: vec![0; len],
            boundaries: vec![0.0; len + 1],
        }
    }

    fn transform(&mut self, len: usize) {
        dt_1d(
            &self.line[..len],
            &mut self.d,
            &mut self.v,
            &mut self.boundaries,
        );
    }

    fn xy(&mut self, plane: &mut [f32], nx: usize, ny: usize) {
        for row in plane.chunks_mut(nx) {
            self.line[..nx].copy_from_slice(row);
            self.transform(nx);
            row.copy_from_slice(&self.d[..nx]);
        }
        for x in 0..nx {
            for y in 0..ny {
                self.line[y] = plane[x + nx * y];
            }
            self.transform(ny);
            for y in 0..ny {
                plane[x + nx * y] = self.d[y];
            }
        }
    }
}

/// 1-D squared distance transform (lower envelope of parabolas), Felzenszwalb &
/// Huttenlocher 2012.
fn dt_1d(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    let mut k: isize = 0;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        let qf = q as f32;
        loop {
            let vk = v[k as usize];
            let s = ((f[q] + qf * qf) - (f[vk] + (vk * vk) as f32)) / (2.0 * qf - 2.0 * vk as f32);
            if s <= z[k as usize] && k > 0 {
                k -= 1;
            } else {
                k += 1;
                v[k as usize] = q;
                z[k as usize] = s;
                z[k as usize + 1] = f32::INFINITY;
                break;
            }
        }
    }
    k = 0;
    for q in 0..n {
        let qf = q as f32;
        while z[k as usize + 1] < qf {
            k += 1;
        }
        let vk = v[k as usize];
        let dq = qf - vk as f32;
        d[q] = dq * dq + f[vk];
    }
}

/// Naive Surface Nets: one vertex per cell straddling `field = 0`, placed at the
/// average of the cell's edge crossings; quads connect cells across each straddling
/// grid edge. Watertight by construction. Normals = −∇field; each vertex is seeded
/// with its nearest atom's color (later Laplacian-smoothed by [`laplacian_smooth`]) and
/// tagged with that atom's global index `ids[nearest]` in `vert_atom`.
fn surface_nets(
    field: &[f32],
    nearest: &[u32],
    colors: &[u32],
    ids: &[u32],
    dims: [usize; 3],
    origin: Vec3,
    h: f32,
) -> MeshData {
    let (nx, ny, nz) = (dims[0], dims[1], dims[2]);
    let idx = |x: usize, y: usize, z: usize| x + nx * (y + ny * z);
    // Cell (x,y,z) spans corners x..x+1 etc.; (nx-1)×(ny-1)×(nz-1) cells.
    let (cx, cy, cz) = (nx - 1, ny - 1, nz - 1);
    if cx == 0 || cy == 0 || cz == 0 {
        return MeshData::default();
    }
    let cidx = |x: usize, y: usize, z: usize| x + cx * (y + cy * z);
    let mut cell_vert = vec![u32::MAX; cx * cy * cz];

    let mut vertices: Vec<MeshVertex> = Vec::new();
    let mut vert_atom: Vec<u32> = Vec::new();

    // The 8 corner offsets and the 12 cube edges (corner index pairs).
    const CORNER: [[usize; 3]; 8] = [
        [0, 0, 0],
        [1, 0, 0],
        [1, 1, 0],
        [0, 1, 0],
        [0, 0, 1],
        [1, 0, 1],
        [1, 1, 1],
        [0, 1, 1],
    ];
    const EDGE: [[usize; 2]; 12] = [
        [0, 1],
        [1, 2],
        [2, 3],
        [3, 0],
        [4, 5],
        [5, 6],
        [6, 7],
        [7, 4],
        [0, 4],
        [1, 5],
        [2, 6],
        [3, 7],
    ];

    // Place one vertex per straddling cell.
    for z in 0..cz {
        for y in 0..cy {
            for x in 0..cx {
                let mut corner_d = [0.0f32; 8];
                let mut neg = false;
                let mut pos = false;
                for (c, off) in CORNER.iter().enumerate() {
                    let d = field[idx(x + off[0], y + off[1], z + off[2])];
                    corner_d[c] = d;
                    if d < 0.0 {
                        neg = true;
                    } else {
                        pos = true;
                    }
                }
                if !(neg && pos) {
                    continue; // cell entirely inside or outside
                }
                let mut acc = Vec3::ZERO;
                let mut cnt = 0.0f32;
                for e in &EDGE {
                    let (a, b) = (e[0], e[1]);
                    let (da, db) = (corner_d[a], corner_d[b]);
                    if (da < 0.0) != (db < 0.0) {
                        let t = da / (da - db);
                        let pa = Vec3::new(
                            (x + CORNER[a][0]) as f32,
                            (y + CORNER[a][1]) as f32,
                            (z + CORNER[a][2]) as f32,
                        );
                        let pb = Vec3::new(
                            (x + CORNER[b][0]) as f32,
                            (y + CORNER[b][1]) as f32,
                            (z + CORNER[b][2]) as f32,
                        );
                        acc += pa.lerp(pb, t);
                        cnt += 1.0;
                    }
                }
                let vgrid = acc / cnt; // vertex in grid coordinates
                let pos_world = origin + vgrid * h;
                // Evaluate the continuous field gradient at the actual vertex, then normalize.
                // Rounding to a grid node made normals jump between adjacent cells.
                let gx = vgrid.x.round() as usize;
                let gy = vgrid.y.round() as usize;
                let gz = vgrid.z.round() as usize;
                let normal = -sample_gradient(field, dims, vgrid).normalize_or_zero();
                let ni = idx(gx.min(nx - 1), gy.min(ny - 1), gz.min(nz - 1));
                let aid = nearest[ni];
                let color = if aid != u32::MAX {
                    colors.get(aid as usize).copied().unwrap_or(0xffff_ffff)
                } else {
                    0xffff_ffff
                };
                cell_vert[cidx(x, y, z)] = vertices.len() as u32;
                vert_atom.push(
                    ids.get(aid as usize)
                        .copied()
                        .unwrap_or(crate::geometry::NO_ATOM),
                );
                vertices.push(MeshVertex {
                    pos: [pos_world.x, pos_world.y, pos_world.z],
                    normal: [normal.x, normal.y, normal.z],
                    color,
                    mat: 0,
                });
            }
        }
    }

    // Quads: for each grid edge along +x/+y/+z whose endpoints straddle, connect the
    // four cells sharing that edge. Use the shorter diagonal, with winding consistent
    // with the outward field normals (also used for secondary-ray geometry).
    let mut indices: Vec<u32> = Vec::new();
    let quad = |a: u32, b: u32, c: u32, d: u32, indices: &mut Vec<u32>| {
        if a != u32::MAX && b != u32::MAX && c != u32::MAX && d != u32::MAX {
            let pos = |i: u32| Vec3::from_array(vertices[i as usize].pos);
            let tris = if pos(a).distance_squared(pos(c)) <= pos(b).distance_squared(pos(d)) {
                [[a, b, c], [a, c, d]]
            } else {
                [[a, b, d], [b, c, d]]
            };
            for [i, j, k] in tris {
                let face = (pos(j) - pos(i)).cross(pos(k) - pos(i));
                let outward = Vec3::from_array(vertices[i as usize].normal)
                    + Vec3::from_array(vertices[j as usize].normal)
                    + Vec3::from_array(vertices[k as usize].normal);
                if face.dot(outward) >= 0.0 {
                    indices.extend_from_slice(&[i, j, k]);
                } else {
                    indices.extend_from_slice(&[i, k, j]);
                }
            }
        }
    };
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let here = field[idx(x, y, z)] < 0.0;
                // +x edge → cells around it vary in y,z.
                if x + 1 < nx && y >= 1 && z >= 1 && (here != (field[idx(x + 1, y, z)] < 0.0)) {
                    quad(
                        cell_vert[cidx(x, y - 1, z - 1)],
                        cell_vert[cidx(x, y, z - 1)],
                        cell_vert[cidx(x, y, z)],
                        cell_vert[cidx(x, y - 1, z)],
                        &mut indices,
                    );
                }
                // +y edge → cells vary in x,z.
                if y + 1 < ny && x >= 1 && z >= 1 && (here != (field[idx(x, y + 1, z)] < 0.0)) {
                    quad(
                        cell_vert[cidx(x - 1, y, z - 1)],
                        cell_vert[cidx(x, y, z - 1)],
                        cell_vert[cidx(x, y, z)],
                        cell_vert[cidx(x - 1, y, z)],
                        &mut indices,
                    );
                }
                // +z edge → cells vary in x,y.
                if z + 1 < nz && x >= 1 && y >= 1 && (here != (field[idx(x, y, z + 1)] < 0.0)) {
                    quad(
                        cell_vert[cidx(x - 1, y - 1, z)],
                        cell_vert[cidx(x, y - 1, z)],
                        cell_vert[cidx(x, y, z)],
                        cell_vert[cidx(x - 1, y, z)],
                        &mut indices,
                    );
                }
            }
        }
    }

    if std::env::var("MOLAR_VIS_DEBUG_SURF").is_ok() {
        log::info!(
            "Surface grid: {}x{}x{} voxels (h={h:.3}) -> {} verts, {} tris",
            nx,
            ny,
            nz,
            vertices.len(),
            indices.len() / 3
        );
    }

    MeshData {
        vertices,
        indices,
        vert_res: Vec::new(),
        vert_atom,
    }
}

/// Catmull-Rom reconstruction gives a C1 field across voxel boundaries. Its analytic
/// derivative supplies normals of the same surface used for vertex projection.
fn sample_field_gradient(field: &[f32], dims: [usize; 3], p: Vec3) -> (f32, Vec3) {
    let q = p.clamp(
        Vec3::ZERO,
        Vec3::new(
            (dims[0] - 1) as f32,
            (dims[1] - 1) as f32,
            (dims[2] - 1) as f32,
        ),
    );
    let base = q.floor();
    let f = q - base;
    let weights = |t: f32| {
        let t2 = t * t;
        let t3 = t2 * t;
        (
            [
                -0.5 * t + t2 - 0.5 * t3,
                1.0 - 2.5 * t2 + 1.5 * t3,
                0.5 * t + 2.0 * t2 - 1.5 * t3,
                -0.5 * t2 + 0.5 * t3,
            ],
            [
                -0.5 + 2.0 * t - 1.5 * t2,
                -5.0 * t + 4.5 * t2,
                0.5 + 4.0 * t - 4.5 * t2,
                -t + 1.5 * t2,
            ],
        )
    };
    let (wx, dx) = weights(f.x);
    let (wy, dy) = weights(f.y);
    let (wz, dz) = weights(f.z);
    let mut value = 0.0;
    let mut gradient = Vec3::ZERO;
    for z in 0..4 {
        for y in 0..4 {
            for x in 0..4 {
                let ix = (base.x as isize + x as isize - 1).clamp(0, dims[0] as isize - 1) as usize;
                let iy = (base.y as isize + y as isize - 1).clamp(0, dims[1] as isize - 1) as usize;
                let iz = (base.z as isize + z as isize - 1).clamp(0, dims[2] as isize - 1) as usize;
                let v = field[ix + dims[0] * (iy + dims[1] * iz)];
                value += v * wx[x] * wy[y] * wz[z];
                gradient += v * Vec3::new(
                    dx[x] * wy[y] * wz[z],
                    wx[x] * dy[y] * wz[z],
                    wx[x] * wy[y] * dz[z],
                );
            }
        }
    }
    (value, gradient)
}

fn sample_gradient(field: &[f32], dims: [usize; 3], p: Vec3) -> Vec3 {
    sample_field_gradient(field, dims, p).1
}

/// Relax irregular Surface Nets cells tangentially, then project back to the SES
/// level set. Projection prevents the shrinkage of unconstrained Laplacian smoothing.
fn relax_on_field(mesh: &mut MeshData, field: &[f32], dims: [usize; 3], origin: Vec3, h: f32) {
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
fn refine_on_field(
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

/// Triangulate a transition quad using its shorter diagonal, preserving winding.
fn refine_quad(vertices: &[MeshVertex], [a, b, c, d]: [u32; 4], indices: &mut Vec<u32>) {
    let pos = |i: u32| Vec3::from_array(vertices[i as usize].pos);
    if pos(a).distance_squared(pos(c)) <= pos(b).distance_squared(pos(d)) {
        indices.extend_from_slice(&[a, b, c, a, c, d]);
    } else {
        indices.extend_from_slice(&[a, b, d, b, c, d]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_filter_preserves_shared_edge_weights_and_isolated_vertices() {
        let mut colors = [
            [0.0, 0.0, 0.0],
            [1.0, 2.0, 3.0],
            [4.0, 5.0, 6.0],
            [7.0, 8.0, 9.0],
            [9.0, 10.0, 11.0],
        ];
        smooth_attr(&mut colors, &[0, 1, 2, 0, 2, 3], 2);
        // Shared-edge neighbors occur twice, in the original triangle order.
        assert_eq!(
            colors,
            [
                [2.0, 2.5, 3.0],
                [3.0, 3.75, 4.5],
                [3.0, 3.75, 4.5],
                [3.0, 3.75, 4.5],
                [9.0, 10.0, 11.0]
            ]
        );
    }

    #[test]
    fn tiled_z_transform_and_field_smoothing_match_serial_on_partial_tiles() {
        // Neither the XY plane count nor the Z count is a multiple of a tile size.
        let dims = [33usize, 37, 1000];
        let count = dims.iter().product();
        let mut seeds = vec![1e12; count];
        for index in (0..count).step_by(7919) { seeds[index] = 0.0; }
        let mut serial = seeds.clone();
        edt_3d_impl(&mut serial, dims[0], dims[1], dims[2], false);
        #[cfg(not(target_arch = "wasm32"))]
        for threads in [2, 4] {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            let mut actual = seeds.clone();
            pool.install(|| edt_3d_impl(&mut actual, dims[0], dims[1], dims[2], true));
            assert!(actual.iter().zip(&serial).all(|(a,b)| a.to_bits() == b.to_bits()));
            let mut expected = serial.clone();
            smooth_field_impl(&mut expected, dims, 3, false);
            pool.install(|| smooth_field_impl(&mut actual, dims, 3, true));
            assert!(actual.iter().zip(&expected).all(|(a,b)| a.to_bits() == b.to_bits()));
        }
    }

    #[test]
    #[ignore = "manual CPU surface grid benchmark"]
    fn benchmark_parallel_z_and_field_smoothing() {
        use std::{hint::black_box, time::Instant};
        for size in [64usize, 128, 256] {
            let dims = [size; 3];
            let mut seed = vec![1e12; size * size * size];
            for i in (0..seed.len()).step_by(7919) { seed[i] = 0.0; }
            let mut old_dt = Vec::new(); let mut new_dt = Vec::new();
            let mut old_smooth = Vec::new(); let mut new_smooth = Vec::new();
            for _ in 0..6 {
                let mut a = seed.clone(); let mut b = seed.clone();
                let start = Instant::now(); edt_3d_impl(&mut a, size, size, size, false); old_dt.push(start.elapsed());
                let start = Instant::now(); edt_3d_impl(&mut b, size, size, size, true); new_dt.push(start.elapsed());
                assert!(a.iter().zip(&b).all(|(a,b)| a.to_bits() == b.to_bits()));
                let start = Instant::now(); smooth_field_impl(&mut a, dims, 3, false); old_smooth.push(start.elapsed());
                let start = Instant::now(); smooth_field_impl(&mut b, dims, 3, true); new_smooth.push(start.elapsed());
                assert!(a.iter().zip(&b).all(|(a,b)| a.to_bits() == b.to_bits()));
                black_box((a,b));
            }
            for (label, mut old, mut new) in [("EDT", old_dt, new_dt), ("smooth", old_smooth, new_smooth)] {
                old.remove(0); new.remove(0); old.sort(); new.sort();
                eprintln!("{label} {size}³: old={:?}, new={:?}, speedup={:.2}x", old[2], new[2], old[2].as_secs_f64()/new[2].as_secs_f64());
            }
        }
    }

    #[test]
    fn distance_transform_matches_nearest_seed_on_rectangular_grids() {
        // The large case exercises native parallel planes; short/unequal axes
        // exercise scratch reuse with different transform lengths.
        for [nx, ny, nz] in [[1, 1, 1], [2, 7, 3], [9, 2, 5], [65, 67, 63]] {
            let seeds = [
                [0, 0, 0],
                [nx - 1, ny - 1, nz - 1],
                [nx / 2, ny / 3, nz / 2],
            ];
            let big = (nx * nx + ny * ny + nz * nz) as f32 + 1.0;
            let mut grid = vec![big; nx * ny * nz];
            for [x, y, z] in seeds {
                grid[x + nx * (y + ny * z)] = 0.0;
            }
            edt_3d(&mut grid, nx, ny, nz);
            for z in 0..nz {
                for y in 0..ny {
                    for x in 0..nx {
                        let expected = seeds
                            .iter()
                            .map(|&[sx, sy, sz]| {
                                (x.abs_diff(sx).pow(2)
                                    + y.abs_diff(sy).pow(2)
                                    + z.abs_diff(sz).pow(2)) as f32
                            })
                            .fold(f32::INFINITY, f32::min);
                        assert_eq!(grid[x + nx * (y + ny * z)], expected);
                    }
                }
            }
        }
    }

    fn plane() -> (Vec<f32>, [usize; 3], Vec3) {
        let dims = [5, 5, 5];
        let origin = Vec3::splat(-2.0);
        let mut field = Vec::new();
        for z in 0..5 {
            for _ in 0..25 {
                field.push(-(origin.z + z as f32));
            }
        }
        (field, dims, origin)
    }

    fn vertex(p: Vec3, angle: f32) -> MeshVertex {
        MeshVertex {
            pos: p.to_array(),
            normal: (glam::Quat::from_rotation_y(angle.to_radians()) * Vec3::Z).to_array(),
            color: 0xffffffff,
            mat: 0,
        }
    }

    #[test]
    fn adaptive_splits_preserve_area_winding_and_flat_regions() {
        let (field, dims, origin) = plane();
        for (angles, children) in [
            ([0.0, 0.0, 0.0], 1),
            ([0.0, 30.0, 15.0], 2),
            ([15.0, 0.0, 30.0], 2),
            ([30.0, 15.0, 0.0], 2),
            ([0.0, 30.0, 0.0], 3),
            ([0.0, 0.0, 30.0], 3),
            ([30.0, 0.0, 0.0], 3),
            ([0.0, 30.0, -30.0], 4),
        ] {
            let mut mesh = MeshData {
                vertices: vec![
                    vertex(Vec3::ZERO, angles[0]),
                    vertex(Vec3::X, angles[1]),
                    vertex(Vec3::Y, angles[2]),
                ],
                indices: vec![0, 1, 2],
                vert_atom: vec![0, 1, 2],
                ..Default::default()
            };
            refine_on_field(&mut mesh, &field, dims, origin, 1.0, 2);
            assert_eq!(mesh.indices.len() / 3, children);
            assert_eq!(mesh.vert_atom.len(), mesh.vertices.len());
            let mut area = 0.0;
            for tri in mesh.indices.chunks_exact(3) {
                let a = Vec3::from_array(mesh.vertices[tri[0] as usize].pos);
                let b = Vec3::from_array(mesh.vertices[tri[1] as usize].pos);
                let c = Vec3::from_array(mesh.vertices[tri[2] as usize].pos);
                let signed = (b - a).cross(c - a).z * 0.5;
                assert!(signed > 0.0);
                area += signed;
            }
            assert!((area - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn adaptive_transitions_share_edges_and_obey_quality() {
        let (field, dims, origin) = plane();
        let mesh = MeshData {
            vertices: vec![
                vertex(Vec3::ZERO, 0.0),
                vertex(Vec3::X, 0.0),
                vertex(Vec3::X + Vec3::Y, 30.0),
                vertex(Vec3::Y, 15.0),
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            ..Default::default()
        };
        let mut coarse = mesh.clone();
        refine_on_field(&mut coarse, &field, dims, origin, 1.0, 0);
        assert_eq!(coarse.indices, mesh.indices);
        let mut fine = mesh.clone();
        refine_on_field(&mut fine, &field, dims, origin, 1.0, 4);
        assert!(fine.indices.len() > mesh.indices.len());
        let mut edges = std::collections::HashMap::new();
        for tri in fine.indices.chunks_exact(3) {
            for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        for ((a, b), count) in edges {
            assert!(count == 1 || count == 2);
            if count == 1 {
                let a = Vec3::from_array(fine.vertices[a as usize].pos);
                let b = Vec3::from_array(fine.vertices[b as usize].pos);
                assert!(
                    (a.x == b.x && (a.x == 0.0 || a.x == 1.0))
                        || (a.y == b.y && (a.y == 0.0 || a.y == 1.0)),
                    "transition must not introduce an internal boundary"
                );
            }
        }
    }

    #[test]
    fn reconstruction_preserves_the_level_set_and_continuous_normals() {
        let dims = [25, 25, 25];
        let h = 0.05;
        let origin = Vec3::splat(-0.6);
        let radius = 0.2;
        let mut field = Vec::new();
        for z in 0..25 {
            for y in 0..25 {
                for x in 0..25 {
                    let p = origin + Vec3::new(x as f32, y as f32, z as f32) * h;
                    field.push(radius - p.length());
                }
            }
        }
        let nearest = vec![0; field.len()];
        let mut mesh = surface_nets(&field, &nearest, &[0xffffffff], &[0], dims, origin, h);
        relax_on_field(&mut mesh, &field, dims, origin, h);
        let original_triangles = mesh.indices.len() / 3;
        refine_on_field(&mut mesh, &field, dims, origin, h, 4);
        assert!(mesh.indices.len() / 3 > original_triangles);
        assert!(mesh.indices.len() / 3 < 4 * original_triangles);
        assert!(mesh.vertices.len() > 100);
        for v in &mesh.vertices {
            let p = Vec3::from_array(v.pos);
            assert!(
                (p.length() - radius).abs() < h * 0.1,
                "relaxation must preserve surface size"
            );
            assert!(
                Vec3::from_array(v.normal).dot(p.normalize()) > 0.999,
                "normals must follow the continuous field"
            );
        }
        let mut edges = std::collections::HashMap::new();
        for tri in mesh.indices.chunks_exact(3) {
            for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        assert!(
            edges.values().all(|&count| count == 2),
            "surface must stay closed"
        );
    }

    #[test]
    fn zero_probe_builds_a_finite_outward_surface() {
        let atom = Atom::new().with_name("C").guess();
        let raw = crate::data::RawMolecule::single_atom("surface", atom, Vec3::ZERO).unwrap();
        let bound = raw.system.select_all_bound();
        let colorizer = Colorizer::new(crate::color::ColorSpec::default(), &bound, 1, None);
        let mesh = build(&bound, &colorizer, 0.0, 2, 0);
        assert!(!mesh.indices.is_empty());
        for tri in mesh.indices.chunks_exact(3) {
            let a = &mesh.vertices[tri[0] as usize];
            let b = &mesh.vertices[tri[1] as usize];
            let c = &mesh.vertices[tri[2] as usize];
            let face = (Vec3::from_array(b.pos) - Vec3::from_array(a.pos))
                .cross(Vec3::from_array(c.pos) - Vec3::from_array(a.pos));
            let normals = Vec3::from_array(a.normal)
                + Vec3::from_array(b.normal)
                + Vec3::from_array(c.normal);
            assert!(
                face.dot(normals) > -1e-10,
                "refinement must preserve outward winding"
            );
        }
        for v in &mesh.vertices {
            let p = Vec3::from_array(v.pos);
            let n = Vec3::from_array(v.normal);
            assert!(p.is_finite() && n.is_finite());
            assert!(n.dot(p.normalize()) > 0.9);
        }
    }
}
