//! A small orthographic depth rasterizer for ligand view selection. The grid has
//! the same world scale for every direction. Counting its visible target pixels
//! measures projected area, including self-overlap, without GPU readbacks.
use crate::geometry::GeometryRef;
use glam::Vec3;

#[derive(Default)]
pub(crate) struct ViewGeometry {
    spheres: Vec<(Vec3, f32)>,
    capsules: Vec<(Vec3, Vec3, f32, f32)>,
    triangles: Vec<[Vec3; 3]>,
    lines: Vec<(Vec3, Vec3, f32, f32)>,
    points: Vec<(Vec3, f32)>,
}

impl ViewGeometry {
    pub(crate) fn extend(&mut self, geometry: GeometryRef<'_>) {
        self.spheres
            .extend(geometry.spheres.iter().map(|s| (s.center.into(), s.radius)));
        self.capsules.extend(geometry.cylinders.iter().map(|c| {
            (
                c.p0.into(),
                c.p1.into(),
                c.radius,
                c.offset[0] * c.offset[1],
            )
        }));
        self.lines.extend(
            geometry
                .lines
                .chunks_exact(2)
                .map(|s| (s[0].pos.into(), s[1].pos.into(), s[0].width, s[0].offset_px)),
        );
        self.points
            .extend(geometry.points.iter().map(|p| (p.pos.into(), p.size)));
        self.triangles.extend(
            geometry
                .mesh
                .indices
                .chunks_exact(3)
                .map(|t| [0, 1, 2].map(|i| geometry.mesh.vertices[t[i] as usize].pos.into())),
        );
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.spheres.is_empty() && self.capsules.is_empty() && self.triangles.is_empty()
    }

    fn bounds(&self) -> (Vec3, f32) {
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        let mut include = |p: Vec3, r: f32| {
            lo = lo.min(p - Vec3::splat(r));
            hi = hi.max(p + Vec3::splat(r));
        };
        for &(p, r) in &self.spheres {
            include(p, r);
        }
        for &(a, b, r, shift) in &self.capsules {
            include(a, r + shift.abs());
            include(b, r + shift.abs());
        }
        for t in &self.triangles {
            for &p in t {
                include(p, 0.0);
            }
        }
        let center = (lo + hi) * 0.5;
        // Bounding sphere, fixed in world space rather than refitted per direction.
        (center, (hi - lo).length() * 0.5 * 1.01)
    }

    fn raster(
        &self,
        frame: &Frame,
        depth: &mut [f32],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        self.raster_solid(frame, depth, cancelled)?;
        self.raster_screen(frame, depth, cancelled)
    }

    fn raster_screen(
        &self,
        frame: &Frame,
        depth: &mut [f32],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        for (i, &(a, b, width, offset)) in self.lines.iter().enumerate() {
            if i % 256 == 0 && cancelled() {
                return Err("cancelled".into());
            }
            let mut a = frame.project(a);
            let mut b = frame.project(b);
            let axis = b - a;
            let len2 = axis.x * axis.x + axis.y * axis.y;
            if len2 < 1e-12 {
                continue;
            }
            let perp = Vec3::new(-axis.y, axis.x, 0.0) / len2.sqrt();
            a += perp * offset * frame.pixel_world_size;
            b += perp * offset * frame.pixel_world_size;
            let r = width * 0.5 * frame.pixel_world_size;
            frame.visit(
                a.min(b) - Vec3::splat(r),
                a.max(b) + Vec3::splat(r),
                depth,
                |x, y| {
                    let q = Vec3::new(x - a.x, y - a.y, 0.0);
                    let t = (q.x * axis.x + q.y * axis.y) / len2;
                    if (0.0..=1.0).contains(&t) && q.dot(perp).abs() <= r {
                        a.z + t * axis.z
                    } else {
                        f32::NEG_INFINITY
                    }
                },
            );
        }
        for &(p, size) in &self.points {
            let p = frame.project(p);
            let r = size * 0.5 * frame.pixel_world_size;
            frame.visit(p - Vec3::splat(r), p + Vec3::splat(r), depth, |x, y| {
                if (x - p.x).powi(2) + (y - p.y).powi(2) <= r * r {
                    p.z
                } else {
                    f32::NEG_INFINITY
                }
            });
        }
        Ok(())
    }

    fn raster_solid(
        &self,
        frame: &Frame,
        depth: &mut [f32],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        depth.fill(f32::NEG_INFINITY);
        for (i, &(p, r)) in self.spheres.iter().enumerate() {
            if i % 256 == 0 && cancelled() {
                return Err("cancelled".into());
            }
            let p = frame.project(p);
            frame.visit(p - Vec3::splat(r), p + Vec3::splat(r), depth, |x, y| {
                let q = r * r - (x - p.x).powi(2) - (y - p.y).powi(2);
                if q >= 0.0 {
                    p.z + q.sqrt()
                } else {
                    f32::NEG_INFINITY
                }
            });
        }
        for (i, &(a, b, r, shift)) in self.capsules.iter().enumerate() {
            if i % 256 == 0 && cancelled() {
                return Err("cancelled".into());
            }
            let mut a = frame.project(a);
            let mut b = frame.project(b);
            let mut perp = Vec3::new(b.y - a.y, a.x - b.x, 0.0).normalize_or_zero();
            if perp == Vec3::ZERO {
                perp = Vec3::X;
            }
            a += perp * shift;
            b += perp * shift;
            let ba = b - a;
            let baba = ba.length_squared();
            let aa = baba - ba.z * ba.z;
            frame.visit(
                a.min(b) - Vec3::splat(r),
                a.max(b) + Vec3::splat(r),
                depth,
                |x, y| {
                    let mut z = f32::NEG_INFINITY;
                    for p in [a, b] {
                        let q = r * r - (x - p.x).powi(2) - (y - p.y).powi(2);
                        if q >= 0.0 {
                            z = z.max(p.z + q.sqrt());
                        }
                    }
                    if aa > 1e-12 {
                        let oa = Vec3::new(x, y, 0.0) - a;
                        let bo = ba.dot(oa);
                        let bb = baba * oa.z - bo * ba.z;
                        let cc = baba * oa.length_squared() - bo * bo - r * r * baba;
                        let disc = bb * bb - aa * cc;
                        if disc >= 0.0 {
                            let t = (-bb + disc.sqrt()) / aa;
                            let h = bo + t * ba.z;
                            if h >= 0.0 && h <= baba {
                                z = z.max(t);
                            }
                        }
                    }
                    z
                },
            );
        }
        for (i, t) in self.triangles.iter().enumerate() {
            if i % 256 == 0 && cancelled() {
                return Err("cancelled".into());
            }
            let [a, b, c] = t.map(|p| frame.project(p));
            let ab = b - a;
            let ac = c - a;
            let det = ab.x * ac.y - ab.y * ac.x;
            if det.abs() < 1e-12 {
                continue;
            }
            frame.visit(a.min(b).min(c), a.max(b).max(c), depth, |x, y| {
                let qx = x - a.x;
                let qy = y - a.y;
                let u = (qx * ac.y - qy * ac.x) / det;
                let v = (ab.x * qy - ab.y * qx) / det;
                if u >= 0.0 && v >= 0.0 && u + v <= 1.0 {
                    a.z + u * ab.z + v * ac.z
                } else {
                    f32::NEG_INFINITY
                }
            });
        }
        Ok(())
    }
}

struct Frame {
    center: Vec3,
    u: Vec3,
    v: Vec3,
    d: Vec3,
    radius: f32,
    size: usize,
    pixel_world_size: f32,
}
impl Frame {
    fn project(&self, p: Vec3) -> Vec3 {
        let p = p - self.center;
        Vec3::new(p.dot(self.u), p.dot(self.v), p.dot(self.d))
    }
    fn visit(
        &self,
        lo: Vec3,
        hi: Vec3,
        depth: &mut [f32],
        mut surface: impl FnMut(f32, f32) -> f32,
    ) {
        let scale = self.size as f32 / (2.0 * self.radius);
        let x0 = ((lo.x + self.radius) * scale).floor().max(0.0) as usize;
        let y0 = ((lo.y + self.radius) * scale).floor().max(0.0) as usize;
        let x1 = (((hi.x + self.radius) * scale).ceil().max(0.0) as usize).min(self.size);
        let y1 = (((hi.y + self.radius) * scale).ceil().max(0.0) as usize).min(self.size);
        for y in y0..y1 {
            for x in x0..x1 {
                let z = surface(
                    (x as f32 + 0.5) / scale - self.radius,
                    (y as f32 + 0.5) / scale - self.radius,
                );
                let pixel = &mut depth[y * self.size + x];
                *pixel = pixel.max(z);
            }
        }
    }
}

/// Supplies the final camera fit so screen-width lines retain their actual size.
pub(crate) struct PixelView<'a> {
    pub camera: &'a crate::camera::Camera,
    pub bounds: &'a [(Vec3, f32)],
    pub size: [u32; 2],
    pub zoom_out: f32,
}
impl PixelView<'_> {
    fn pixel_world_size(&self, d: Vec3) -> f32 {
        let mut camera = *self.camera;
        camera.orientation = super::look_along_quat(d);
        // Startup searches can precede the first viewport layout. Use a normal
        // display size until it exists, rather than treating a line as world-wide.
        let [width, height] = if self.size.iter().all(|&n| n > 1) {
            self.size
        } else {
            [1280, 720]
        };
        camera.focus_visual_bounds(
            self.bounds,
            width.max(1) as f32 / height.max(1) as f32,
            self.zoom_out,
        );
        2.0 * camera.distance * (camera.fov_y * 0.5).tan() / height.max(1) as f32
    }
}

#[derive(Default)]
pub(crate) struct PixelScene {
    pub target: ViewGeometry,
    pub occluders: ViewGeometry,
}
impl PixelScene {
    pub(crate) fn best_direction(
        &self,
        resolution: usize,
        view: Option<&PixelView<'_>>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec3, String> {
        if self.target.is_empty() {
            return Ok(Vec3::Z);
        }
        let (center, radius) = self.target.bounds();
        let size = 192;
        let buffers = || {
            (
                vec![f32::NEG_INFINITY; size * size],
                vec![f32::NEG_INFINITY; size * size],
            )
        };
        let score = |buffers: &mut (Vec<f32>, Vec<f32>), d: Vec3| {
            if cancelled() {
                return Err("cancelled".into());
            }
            let (u, v) = super::tangent_basis(d);
            let frame = Frame {
                center,
                u,
                v,
                d,
                radius: radius.max(0.001),
                size,
                pixel_world_size: view
                    .map_or(2.0 * radius / 720.0, |view| view.pixel_world_size(d)),
            };
            let (target, blockers) = buffers;
            self.target.raster(&frame, target, cancelled)?;
            self.occluders.raster_solid(&frame, blockers, cancelled)?;
            let area = target.iter().filter(|t| t.is_finite()).count();
            let mut solid_covered = 0;
            for y in 1..size - 1 {
                for x in 1..size - 1 {
                    let i = y * size + x;
                    if !target[i].is_finite() {
                        continue;
                    }
                    // One grid-cell guard band avoids grazing a tube/ribbon edge at
                    // export resolution. Screen lines retain their exact width below.
                    let near = [-1_isize, 0, 1].into_iter().any(|dy| {
                        [-1_isize, 0, 1].into_iter().any(|dx| {
                            blockers[(i as isize + dy * size as isize + dx) as usize]
                                > target[i] + 1e-5
                        })
                    });
                    if near {
                        solid_covered += 1;
                    }
                }
            }
            self.occluders.raster_screen(&frame, blockers, cancelled)?;
            // Each pixel belongs only to the front surface of the target: hidden
            // atoms/bonds cannot contribute area twice. External geometry then
            // removes the pixels it covers, using actual front-surface depth.
            let visible = target
                .iter()
                .zip(blockers.iter())
                .filter(|(t, b)| t.is_finite() && **t >= **b - 1e-5)
                .count();
            let covered = area - visible;
            // A covered fragment harms readability more than an equally sized
            // gain elsewhere helps it. Preserve the area objective (edge-on
            // collapse loses area), but prefer openings with clear silhouettes.
            // Prefer a clear solid-receptor opening. Half a percent tolerates
            // grid sampling at silhouette edges; among such openings maximize
            // readable area. If none is clear, retain the area/occlusion tradeoff.
            Ok((
                solid_covered * 200 <= area,
                visible as i64 - 4 * covered as i64,
            ))
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            use rayon::prelude::*;
            // Do not consume the whole machine while a GUI search runs.
            static POOL: std::sync::OnceLock<Option<rayon::ThreadPool>> =
                std::sync::OnceLock::new();
            if let Some(pool) = POOL.get_or_init(|| {
                let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .thread_name(|i| format!("view-score-{i}"))
                    .build()
                    .ok()
            }) {
                return pool.install(|| {
                    super::search_directions_with_seeds(resolution, 8, |dirs| {
                        dirs.par_iter()
                            .map_init(buffers, |scratch, &d| score(scratch, d))
                            .collect()
                    })
                });
            }
        }
        let mut scratch = buffers();
        super::search_directions_with_seeds(resolution, 8, |dirs| {
            dirs.iter().map(|&d| score(&mut scratch, d)).collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn licorice_ring_prefers_a_face_over_a_collapsed_edge() {
        let mut scene = PixelScene::default();
        for i in 0..6 {
            let p = |i: usize| {
                let a = i as f32 * std::f32::consts::TAU / 6.0;
                Vec3::new(a.cos(), a.sin(), 0.0)
            };
            scene.target.capsules.push((p(i), p(i + 1), 0.1, 0.0));
        }
        let d = scene.best_direction(256, None, &|| false).unwrap();
        assert!(d.z.abs() > 0.9, "ring must have a visible interior: {d:?}");
    }
    #[test]
    fn front_surface_depth_and_self_overlap_match_silhouette() {
        let frame = Frame {
            center: Vec3::ZERO,
            u: Vec3::X,
            v: Vec3::Y,
            d: Vec3::Z,
            radius: 2.0,
            size: 128,
            pixel_world_size: 0.01,
        };
        let mut g = ViewGeometry::default();
        g.spheres.push((Vec3::ZERO, 0.5));
        let mut depth = vec![0.0; 128 * 128];
        g.raster(&frame, &mut depth, &|| false).unwrap();
        let original = depth.clone();
        g.spheres.push((Vec3::new(0.0, 0.0, -0.2), 0.5));
        g.raster(&frame, &mut depth, &|| false).unwrap();
        assert_eq!(depth, original, "a hidden sphere cannot add target pixels");
        assert!(depth[64 * 128 + 64] > 0.49);
    }
    #[test]
    fn receptor_bond_extent_blocks_even_when_both_atom_centers_miss() {
        let frame = Frame {
            center: Vec3::ZERO,
            u: Vec3::X,
            v: Vec3::Y,
            d: Vec3::Z,
            radius: 2.0,
            size: 128,
            pixel_world_size: 0.01,
        };
        let mut g = ViewGeometry::default();
        g.capsules.push((
            Vec3::new(-1.0, 0.0, 0.2),
            Vec3::new(1.0, 0.0, 0.2),
            0.3,
            0.0,
        ));
        let mut depth = vec![0.0; 128 * 128];
        g.raster(&frame, &mut depth, &|| false).unwrap();
        assert!(
            depth[64 * 128 + 64] > 0.49,
            "finite receptor bond must cover the centre ray"
        );
    }
    #[test]
    fn receptor_line_uses_its_full_screen_width() {
        let frame = Frame {
            center: Vec3::ZERO,
            u: Vec3::X,
            v: Vec3::Y,
            d: Vec3::Z,
            radius: 2.0,
            size: 128,
            pixel_world_size: 0.01,
        };
        let mut g = ViewGeometry::default();
        g.lines.push((
            Vec3::new(0.0, -1.0, 1.0),
            Vec3::new(0.0, 1.0, 1.0),
            20.0,
            0.0,
        ));
        let mut depth = vec![0.0; 128 * 128];
        g.raster(&frame, &mut depth, &|| false).unwrap();
        assert_eq!(
            depth[64 * 128 + 66],
            1.0,
            "ray misses centreline but hits the wide line"
        );
        assert!(!depth[64 * 128 + 68].is_finite());
    }
    #[test]
    fn cancellation_stops_pixel_search() {
        let mut scene = PixelScene::default();
        scene.target.spheres.push((Vec3::ZERO, 1.0));
        assert!(scene.best_direction(256, None, &|| true).is_err());
    }
}
