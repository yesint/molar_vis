//! Hydrogen-bond local geometry adapted from Mol* (MIT; see licenses/molstar.txt).
//!
//! Reference revision: fe1ac1555001d1acb023077cd4dc6ca1d06870f8,
//! packages/model/src/props/computed/interactions/hydrogen-bonds.ts and
//! packages/model/src/props/computed/chemistry/geometry.ts.
//! Copyright (c) 2017–2026 Mol* contributors.
//!
//! Chemical roles are assigned before the spatial search. The geometry test applies
//! to every donor and acceptor, including water, without residue-specific exceptions.

use crate::interactions::{AtomInfo, DetectParams};
use glam::Vec3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Geometry {
    Linear,
    Trigonal,
    Tetrahedral,
    #[default]
    Unknown,
}

impl Geometry {
    pub fn from_coordination(n: i32) -> Self {
        match n {
            2 => Self::Linear,
            3 => Self::Trigonal,
            4 => Self::Tetrahedral,
            _ => Self::Unknown,
        }
    }

    pub fn angle(self) -> f32 {
        match self {
            Self::Linear => 180.0,
            Self::Tetrahedral => 109.4721,
            _ => 120.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Role {
    pub donor: bool,
    pub acceptor: bool,
    pub geometry: Geometry,
}

#[derive(Default)]
pub(crate) struct Chemistry {
    pub roles: Vec<Role>,
    pub neighbors: Vec<Vec<u32>>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Site {
    pub role: Role,
    pub heavy_neighbors: Vec<Vec3>,
    pub plane_normal: Option<Vec3>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Contact {
    /// Index in the donor's attached_h list. None means a chemically inferred H.
    pub hydrogen: Option<usize>,
    pub angle: Option<f32>,
}

fn angle(a: Vec3, b: Vec3) -> Option<f32> {
    let norm = a.length() * b.length();
    (norm > 1e-12).then(|| (a.dot(b) / norm).clamp(-1.0, 1.0).acos().to_degrees())
}

/// Mol* defaults: 45° donor/acceptor deviation from ideal local geometry,
/// 45° donor out-of-plane tolerance, and no acceptor out-of-plane restriction.
pub(crate) fn contact(
    donor: &AtomInfo,
    acc: &AtomInfo,
    d: f32,
    p: &DetectParams,
) -> Option<Contact> {
    if !donor.hbond.role.donor || !acc.hbond.role.acceptor {
        return None;
    }
    let cutoff = if donor.attached_h.is_empty() {
        p.hbond_dist_heavy
    } else {
        p.hbond_dist_h
    };
    if d > cutoff {
        return None;
    }
    let toward_acc = acc.pos - donor.pos;
    let ideal = donor.hbond.role.geometry.angle();
    for &neighbor in &donor.hbond.heavy_neighbors {
        if (angle(neighbor - donor.pos, toward_acc)? - ideal).abs() > 45.0 {
            return None;
        }
    }
    if donor.hbond.role.geometry == Geometry::Trigonal {
        if let Some(normal) = donor.hbond.plane_normal {
            if (90.0 - angle(normal, toward_acc)?).abs() > 45.0 {
                return None;
            }
        }
    }
    // Mol* selects the closest explicit H, provided a donor O/N/S–H direction
    // points toward the acceptor. Never replace an invalid explicit H with D.
    let hydrogen = if donor.attached_h.is_empty() {
        None
    } else {
        if !donor
            .attached_h
            .iter()
            .any(|&h| angle(h - donor.pos, toward_acc).is_some_and(|a| a < 45.0))
        {
            return None;
        }
        let (index, h) = donor
            .attached_h
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                a.distance_squared(acc.pos)
                    .total_cmp(&b.distance_squared(acc.pos))
            })?;
        if h.distance_squared(acc.pos) >= donor.pos.distance_squared(acc.pos) {
            return None;
        }
        Some(index)
    };
    let from = hydrogen.map_or(donor.pos, |i| donor.attached_h[i]);
    let ideal = acc.hbond.role.geometry.angle();
    for &neighbor in acc.hbond.heavy_neighbors.iter().chain(&acc.attached_h) {
        if ideal - angle(neighbor - acc.pos, from - acc.pos)? > 45.0 {
            return None;
        }
    }
    // Retain the viewer's existing user-configurable D–H···A lower bound as an
    // additional filter. It cannot relax the Mol* local-geometry checks above.
    let h_angle = if hydrogen.is_some() {
        let a = angle(donor.pos - from, acc.pos - from)?;
        if a <= p.hbond_angle_min {
            return None;
        }
        Some(a)
    } else {
        None
    };
    Some(Contact {
        hydrogen,
        angle: h_angle,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactions::InteractionSettings;

    #[derive(serde::Deserialize)]
    struct Shape {
        pos: Vec3,
        geometry: String,
        h: Vec<Vec3>,
        heavy: Vec<Vec3>,
        normal: Option<Vec3>,
    }
    impl Shape {
        fn atom(self) -> AtomInfo {
            let geometry = match self.geometry.as_str() {
                "Linear" => Geometry::Linear,
                "Trigonal" => Geometry::Trigonal,
                "Tetrahedral" => Geometry::Tetrahedral,
                _ => Geometry::Unknown,
            };
            AtomInfo {
                id: Default::default(),
                pos: self.pos,
                atomicnum: 8,
                res_key: 0,
                only_ch_neighbors: false,
                attached_h: self.h,
                attached_h_ids: Vec::new(),
                antecedent: None,
                hbond: Site {
                    role: Role {
                        donor: true,
                        acceptor: true,
                        geometry,
                    },
                    heavy_neighbors: self.heavy,
                    plane_normal: self.normal,
                },
            }
        }
    }
    #[derive(serde::Deserialize)]
    struct Case {
        name: String,
        donor: Shape,
        acceptor: Shape,
        accepted: bool,
    }

    #[test]
    fn geometry_matches_pinned_molstar_reference() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../../../tests/hbond_geometry_reference.json"))
                .unwrap();
        let mut p = InteractionSettings::default().detect();
        // Test the port against Mol* independently of the viewer's extra cutoffs.
        p.hbond_dist_heavy = 10.0;
        p.hbond_dist_h = 10.0;
        p.hbond_angle_min = 0.0;
        for case in cases {
            let donor = case.donor.atom();
            let acc = case.acceptor.atom();
            assert_eq!(
                contact(&donor, &acc, donor.pos.distance(acc.pos), &p).is_some(),
                case.accepted,
                "{}",
                case.name
            );
        }
    }
}
