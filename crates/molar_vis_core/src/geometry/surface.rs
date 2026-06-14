//! Molecular-surface builder: the solvent-excluded surface (SES, rolling-probe) via
//! a **grid distance field + Surface Nets** — the robust, watertight-by-construction
//! method used by PyMOL/Chimera/EDTSurf (distance maps + carving), rather than
//! analytic patch stitching (which can't be made reliably watertight).
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
//! Per-vertex normals come from the field gradient; colors from the nearest atom.

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
    // Gather atom spheres (SAS radius = vdW + probe) + per-atom color.
    let mut centers: Vec<Vec3> = Vec::new();
    let mut radii: Vec<f32> = Vec::new();
    let mut colors: Vec<u32> = Vec::new();
    let mut rmax = 0.0_f32;
    for p in bound.iter_particle() {
        centers.push(Vec3::new(p.pos.x, p.pos.y, p.pos.z));
        let r = p.atom.vdw() + probe;
        radii.push(r);
        rmax = rmax.max(r);
        colors.push(colorizer.color(p.atom, p.id));
    }
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

    // --- Pass 1: SAS occupancy + nearest atom per voxel (for coloring). ---
    let mut inside = vec![false; n];
    let mut nearest = vec![u32::MAX; n];
    let mut best_d2 = vec![f32::INFINITY; n];
    for (a, (&c, &r)) in centers.iter().zip(&radii).enumerate() {
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
                    // Nearest atom (by center) for coloring, tracked everywhere in range.
                    if d2 < best_d2[i] {
                        best_d2[i] = d2;
                        nearest[i] = a as u32;
                    }
                }
            }
        }
    }

    // --- Pass 2: exact EDT from each inside voxel to the nearest outside voxel. ---
    // Seed: 0 at outside (feature) voxels, +inf at inside; F-H separable transform
    // then sqrt gives voxel distance, ×h gives nm. `field = edt - probe` is the SES
    // level set (0 outside SAS too, so field = -probe there → continuous).
    let big = (nx * nx + ny * ny + nz * nz) as f32 + 1.0;
    let mut g: Vec<f32> = (0..n).map(|i| if inside[i] { big } else { 0.0 }).collect();
    edt_3d(&mut g, nx, ny, nz);
    let mut field: Vec<f32> = g.iter().map(|&d2| d2.sqrt() * h - probe).collect();

    // Light separable [1,2,1] blur of the distance field: the binary occupancy makes
    // the EDT (and its gradient = our normals) stair-step at voxel resolution, which
    // reads as a rugged/faceted surface. Blurring the field removes that
    // high-frequency noise so both the extracted isosurface and the shading come out
    // smooth, at O(voxels) cost. Driven by the rep's `smoothing` slider.
    smooth_field(&mut field, dims, smoothing as usize);

    // --- Pass 3: Surface Nets isosurface at field = 0. ---
    surface_nets(&field, &nearest, &colors, dims, lo, h)
}

/// Separable [1,2,1]/4 blur of a scalar grid, applied `passes` times along each
/// axis (edges clamped). Cheap (O(voxels·passes)); smooths the distance field so the
/// extracted surface and its gradient normals lose the voxel-staircase ruggedness.
fn smooth_field(field: &mut [f32], dims: [usize; 3], passes: usize) {
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
    let idx = |x: usize, y: usize, z: usize| x + nx * (y + ny * z);
    let mut line = vec![0.0f32; nx.max(ny).max(nz)];
    // Along x.
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                line[x] = g[idx(x, y, z)];
            }
            let d = dt_1d(&line[..nx]);
            for x in 0..nx {
                g[idx(x, y, z)] = d[x];
            }
        }
    }
    // Along y.
    for z in 0..nz {
        for x in 0..nx {
            for y in 0..ny {
                line[y] = g[idx(x, y, z)];
            }
            let d = dt_1d(&line[..ny]);
            for y in 0..ny {
                g[idx(x, y, z)] = d[y];
            }
        }
    }
    // Along z.
    for y in 0..ny {
        for x in 0..nx {
            for z in 0..nz {
                line[z] = g[idx(x, y, z)];
            }
            let d = dt_1d(&line[..nz]);
            for z in 0..nz {
                g[idx(x, y, z)] = d[z];
            }
        }
    }
}

/// 1-D squared distance transform (lower envelope of parabolas), Felzenszwalb &
/// Huttenlocher 2012.
fn dt_1d(f: &[f32]) -> Vec<f32> {
    let n = f.len();
    let mut d = vec![0.0f32; n];
    let mut v = vec![0usize; n];
    let mut z = vec![0.0f32; n + 1];
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
    d
}

/// Naive Surface Nets: one vertex per cell straddling `field = 0`, placed at the
/// average of the cell's edge crossings; quads connect cells across each straddling
/// grid edge. Watertight by construction. Normals = −∇field; colors = nearest atom.
fn surface_nets(
    field: &[f32],
    nearest: &[u32],
    colors: &[u32],
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

    // The 8 corner offsets and the 12 cube edges (corner index pairs).
    const CORNER: [[usize; 3]; 8] = [
        [0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0],
        [0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1],
    ];
    const EDGE: [[usize; 2]; 12] = [
        [0, 1], [1, 2], [2, 3], [3, 0],
        [4, 5], [5, 6], [6, 7], [7, 4],
        [0, 4], [1, 5], [2, 6], [3, 7],
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
                // Normal from the field gradient (central differences, trilinear-ish
                // at the nearest grid sample), pointing outward (toward solvent).
                let gx = vgrid.x.round() as usize;
                let gy = vgrid.y.round() as usize;
                let gz = vgrid.z.round() as usize;
                let normal = -field_gradient(field, dims, gx, gy, gz);
                let ni = idx(gx.min(nx - 1), gy.min(ny - 1), gz.min(nz - 1));
                let aid = nearest[ni];
                let color = if aid != u32::MAX {
                    colors.get(aid as usize).copied().unwrap_or(0xffff_ffff)
                } else {
                    0xffff_ffff
                };
                cell_vert[cidx(x, y, z)] = vertices.len() as u32;
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
    // four cells sharing that edge. Winding is irrelevant (two-sided rendering), so
    // we emit a fixed split; the cells must all exist (interior edges only).
    let mut indices: Vec<u32> = Vec::new();
    let quad = |a: u32, b: u32, c: u32, d: u32, indices: &mut Vec<u32>| {
        if a != u32::MAX && b != u32::MAX && c != u32::MAX && d != u32::MAX {
            indices.extend_from_slice(&[a, b, c, a, c, d]);
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

    MeshData { vertices, indices }
}

/// Central-difference gradient of the scalar field at a grid sample (clamped).
fn field_gradient(field: &[f32], dims: [usize; 3], x: usize, y: usize, z: usize) -> Vec3 {
    let (nx, ny, nz) = (dims[0], dims[1], dims[2]);
    let idx = |x: usize, y: usize, z: usize| x + nx * (y + ny * z);
    let s = |a: usize, lim: usize| a.min(lim - 1);
    let xm = idx(x.saturating_sub(1), s(y, ny), s(z, nz));
    let xp = idx(s(x + 1, nx), s(y, ny), s(z, nz));
    let ym = idx(s(x, nx), y.saturating_sub(1), s(z, nz));
    let yp = idx(s(x, nx), s(y + 1, ny), s(z, nz));
    let zm = idx(s(x, nx), s(y, ny), z.saturating_sub(1));
    let zp = idx(s(x, nx), s(y, ny), s(z + 1, nz));
    let g = Vec3::new(
        field[xp] - field[xm],
        field[yp] - field[ym],
        field[zp] - field[zm],
    );
    g.normalize_or_zero()
}
