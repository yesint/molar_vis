//! Coordinate-independent inputs for trajectory updates of atom/bond styles.
use super::*;

pub(crate) struct PrimitiveCache {
    ids: Vec<usize>,
    colors: Vec<u32>,
    radii: Vec<f32>,
    bonds: Vec<Bond>,
    bonded: Vec<bool>,
    params: RepParams,
    material: Material,
}

impl PrimitiveCache {
    pub(crate) fn new(
        bound: &(impl ParticleIterProvider + AtomProvider),
        n_atoms: usize,
        bonds: &[Bond],
        params: &RepParams,
        color: ColorSpec,
        material: Material,
        ss: Option<&SsMap>,
    ) -> Option<Self> {
        let scale = match *params {
            RepParams::Vdw { scale } => scale,
            RepParams::BallAndStick { sphere_scale, .. } => sphere_scale,
            RepParams::Licorice { .. } | RepParams::Lines { .. } => 0.0,
            _ => return None,
        };
        let colorizer = Colorizer::new(color, bound, n_atoms, ss);
        let mut ids = Vec::new();
        let mut colors = Vec::new();
        let mut radii = Vec::new();
        // The full-molecule lookup is temporary. Playback stores only selected atoms
        // and bonds, preserving their original iteration order and global color IDs.
        let needs_bonds = !matches!(params, RepParams::Vdw { .. });
        let mut local = vec![usize::MAX; if needs_bonds { n_atoms } else { 0 }];
        for p in bound.iter_particle() {
            if needs_bonds {
                local[p.id] = ids.len();
            }
            ids.push(p.id);
            colors.push(colorizer.color(p.atom, p.id));
            radii.push(match *params {
                RepParams::Licorice { bond_radius } => bond_radius,
                _ => p.atom.vdw() * scale,
            });
        }
        let mut bonded = vec![false; ids.len()];
        let bonds = if needs_bonds { bonds } else { &[] };
        let bonds = bonds
            .iter()
            .filter_map(|bond| {
                let [a, b] = bond.pair();
                let (a, b) = (*local.get(a)?, *local.get(b)?);
                if a == usize::MAX || b == usize::MAX {
                    return None;
                }
                bonded[a] = true;
                bonded[b] = true;
                Some(Bond::with_order(a, b, bond.order))
            })
            .collect();
        Some(Self {
            ids,
            colors,
            radii,
            bonds,
            bonded,
            params: *params,
            material,
        })
    }

    pub(crate) fn build(&self, state: &State, dashed_pbc: bool) -> GeometryData {
        let _timing = crate::performance::span("primitive-cache-build");
        if matches!(self.params, RepParams::Vdw { .. }) {
            let mut data = GeometryData {
                spheres: self
                    .ids
                    .iter()
                    .enumerate()
                    .map(|(i, &id)| {
                        let p = &state.coords[id];
                        SphereInstance {
                            center: [p.x, p.y, p.z],
                            radius: self.radii[i],
                            color: self.colors[i],
                            mat: 0,
                            pick: [0, 0],
                        }
                    })
                    .collect(),
                ..Default::default()
            };
            stamp_material(&mut data, self.material);
            return data;
        }
        let lut: Vec<_> = self
            .ids
            .iter()
            .zip(&self.colors)
            .map(|(&id, &color)| {
                let p = &state.coords[id];
                Some(([p.x, p.y, p.z], color))
            })
            .collect();
        let pbox = if dashed_pbc {
            state.pbox.as_ref().filter(|b| {
                let e = b.get_box_extents();
                e.x.min(e.y).min(e.z) >= MIN_USABLE_BOX_NM
            })
        } else {
            None
        };
        let mut data = GeometryData::default();
        if !matches!(self.params, RepParams::Lines { .. }) {
            data.spheres = lut
                .iter()
                .enumerate()
                .filter_map(|(i, entry)| {
                    if matches!(self.params, RepParams::Licorice { .. }) && self.bonded[i] {
                        return None;
                    }
                    let (center, color) = entry.unwrap();
                    Some(SphereInstance {
                        center,
                        radius: self.radii[i],
                        color,
                        mat: 0,
                        pick: [0, 0],
                    })
                })
                .collect();
        }
        match self.params {
            RepParams::Licorice { bond_radius } => {
                data.cylinders = cylinders(&lut, &self.bonds, bond_radius, pbox, None, 0.0);
            }
            RepParams::BallAndStick {
                bond_radius,
                bond_smoothing,
                ..
            } => {
                let radii = (bond_smoothing > 0.0).then_some(self.radii.as_slice());
                data.cylinders =
                    cylinders(&lut, &self.bonds, bond_radius, pbox, radii, bond_smoothing);
            }
            RepParams::Lines { width } => {
                data.lines = lines(&lut, &self.bonds, width, pbox);
                for (i, entry) in lut.iter().enumerate() {
                    if self.bonded[i] {
                        continue;
                    }
                    let (center, color) = entry.unwrap();
                    for axis in 0..3 {
                        let (mut lo, mut hi) = (center, center);
                        lo[axis] -= LINES_CROSS_HALF_LEN;
                        hi[axis] += LINES_CROSS_HALF_LEN;
                        data.lines.push(LineVertex {
                            pos: lo,
                            color,
                            width,
                            offset_px: 0.0,
                        });
                        data.lines.push(LineVertex {
                            pos: hi,
                            color,
                            width,
                            offset_px: 0.0,
                        });
                    }
                }
            }
            RepParams::Vdw { .. } => (),
            _ => unreachable!("only primitive styles have a coordinate cache"),
        }
        stamp_material(&mut data, self.material);
        data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "manual CPU geometry benchmark"]
    fn benchmark_coordinate_cache() {
        use std::{hint::black_box, time::Instant};
        let raw = crate::data::load(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/2lao.pdb"
        )))
        .unwrap();
        let mut scene = crate::scene::Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        let mol = &scene.molecules[0];
        for selection in ["all", "name CA"] {
            let (_, sel) = mol.data.evaluate(selection).unwrap();
            let bound = mol.data.bind_with_state(&sel, mol.render_state());
            for kind in [
                RepKind::Vdw,
                RepKind::Licorice,
                RepKind::BallAndStick,
                RepKind::Lines,
            ] {
                let params = RepParams::for_kind(kind);
                let cache = PrimitiveCache::new(
                    &bound,
                    mol.n_atoms,
                    &mol.bonds,
                    &params,
                    ColorMethod::Element.into(),
                    Material::Opaque,
                    None,
                )
                .unwrap();
                black_box(cache.build(mol.render_state(), false));
                let start = Instant::now();
                for _ in 0..500 {
                    black_box(build(
                        &bound,
                        mol.n_atoms,
                        &mol.bonds,
                        &params,
                        ColorMethod::Element.into(),
                        Material::Opaque,
                        None,
                        false,
                    ));
                }
                let full = start.elapsed();
                let start = Instant::now();
                for _ in 0..500 {
                    black_box(cache.build(mol.render_state(), false));
                }
                let cached = start.elapsed();
                eprintln!(
                    "{kind:?}, {selection}: full={full:?}, cached={cached:?}, speedup={:.2}x",
                    full.as_secs_f64() / cached.as_secs_f64()
                );
            }
        }
    }

    #[test]
    fn cached_frames_match_full_build_with_sparse_selection_and_periodic_bonds() {
        let raw = crate::data::load(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/2lao.pdb"
        )))
        .unwrap();
        let mut scene = crate::scene::Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        let mol = &scene.molecules[0];
        let (_, sel) = mol.data.evaluate("name CA").unwrap();
        let bound = mol.data.bind_with_state(&sel, mol.render_state());
        let ids: Vec<_> = bound.iter_particle().map(|p| p.id).collect();
        assert!(ids.len() > 5);
        let bonds = vec![
            Bond::with_order(ids[0], ids[1], BondOrder::Double),
            Bond::with_order(ids[1], ids[2], BondOrder::Triple),
            Bond::with_order(ids[2], ids[3], BondOrder::Aromatic),
            // An endpoint outside the selection must not create a drawn bond.
            Bond::new(ids[4], 0),
        ];
        assert_ne!(ids[0], 0);
        let ss = SsMap::compute(&bound, Default::default());
        for params in [
            RepParams::Vdw { scale: 0.9 },
            RepParams::Licorice { bond_radius: 0.035 },
            RepParams::BallAndStick {
                sphere_scale: 0.3,
                bond_radius: 0.025,
                bond_smoothing: 0.0,
            },
            RepParams::BallAndStick {
                sphere_scale: 0.4,
                bond_radius: 0.025,
                bond_smoothing: 0.7,
            },
            RepParams::Lines { width: 1.7 },
        ] {
            for color in ColorMethod::ALL {
                let cache = PrimitiveCache::new(
                    &bound,
                    mol.n_atoms,
                    &bonds,
                    &params,
                    color.into(),
                    Material::Transparent,
                    Some(&ss),
                )
                .unwrap();
                assert_eq!(
                    cache.bonds.len(),
                    if matches!(params, RepParams::Vdw { .. }) {
                        0
                    } else {
                        3
                    }
                );
                for frame in 0..3 {
                    let mut state = mol.render_state().clone();
                    state.pbox = match frame {
                        0 => None,
                        1 => Some(
                            PeriodicBox::from_vectors_angles(2.0, 2.0, 2.0, 90.0, 90.0, 90.0)
                                .unwrap(),
                        ),
                        _ => Some(
                            PeriodicBox::from_vectors_angles(0.1, 0.1, 0.1, 90.0, 90.0, 90.0)
                                .unwrap(),
                        ),
                    };
                    for (i, p) in state.coords.iter_mut().enumerate() {
                        p.x += (i as f32 * 0.13 + frame as f32).sin() * 0.7;
                        p.y *= 1.0 + frame as f32 * 0.1;
                    }
                    for dashed in [false, true] {
                        let moved = mol.data.bind_with_state(&sel, &state);
                        let expected = build(
                            &moved,
                            mol.n_atoms,
                            &bonds,
                            &params,
                            color.into(),
                            Material::Transparent,
                            Some(&ss),
                            dashed,
                        );
                        let actual = cache.build(&state, dashed);
                        fn bytes<T: bytemuck::Pod>(v: &[T]) -> &[u8] {
                            bytemuck::cast_slice(v)
                        }
                        assert_eq!(
                            bytes(&actual.spheres),
                            bytes(&expected.spheres),
                            "{params:?} {color:?} frame {frame}"
                        );
                        assert_eq!(
                            bytes(&actual.cylinders),
                            bytes(&expected.cylinders),
                            "{params:?} {color:?} frame {frame}"
                        );
                        assert_eq!(
                            bytes(&actual.lines),
                            bytes(&expected.lines),
                            "{params:?} {color:?} frame {frame}"
                        );
                    }
                }
            }
        }
    }
}
