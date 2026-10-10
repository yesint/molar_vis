//! VMD-style and illustrative materials: a per-representation appearance preset controlling
//! lighting (ambient / diffuse / specular / shininess) and **opacity**.
//!
//! The values are GPU-packed per geometry element: the four lighting
//! coefficients pack into a single `u32` (`mat`, carried per instance/vertex) and
//! the opacity rides in the alpha channel of the element's color. Shaders unpack
//! both; transparent materials (`opacity < 1`) are drawn in a second,
//! depth-write-off, alpha-blended pass.

/// A representation's material preset. Values approximate VMD's built-in
/// materials (the real-time-relevant subset) and Molecular Nodes appearance recipes.
#[derive(Clone, Copy, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Material {
    #[default]
    Opaque,
    Transparent,
    Glass,
    Translucent,
    Ghost,
    Glossy,
    Diffuse,
    Metal,
    /// VMD's `AOChalky`: matte, no specular, high diffuse — designed for
    /// ambient-occlusion rendering (AO supplies the crevice shading).
    AoChalky,
    /// VMD's `AOShiny`: like AOChalky but with a specular highlight.
    AoShiny,
    /// VMD's `AOEdgy`: matte like AOChalky, plus a dark silhouette **outline**
    /// (grazing-angle edge darkening) — an illustrative, "edgy" look.
    AoEdgy,
    /// Flat atom colors with crisp dark contour lines and no internal lighting effects.
    FlatOutline,
    /// Soft dielectric studio shading with gentle contact occlusion, inspired by
    /// Molecular Nodes' default Principled material.
    MolecularNodes,
    /// Edited preset; the stable index refers to ALL, whose order must not change.
    Custom { preset: u8, options: MaterialOptions },
}

/// Specialized shading uses reserved material words; existing coefficient encodings
/// remain byte-identical. These words are injected into all shared shader sources.
pub(crate) const FLAT_OUTLINE_WORD: u32 = 0xffff_fffe;
pub(crate) const MOLECULAR_NODES_WORD: u32 = 0xffff_fffd;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shading {
    Classic,
    FlatOutline,
    MolecularNodes,
}

/// Persisted shader coefficients, written as full records when a preset is edited.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MaterialOptions {
    pub ambient: f32,
    pub diffuse: f32,
    pub specular: f32,
    pub shininess: f32,
    pub opacity: f32,
    pub outline: f32,
    pub outline_width: f32,
}

/// Lighting + opacity coefficients (each 0..1).
pub struct MaterialParams {
    pub shading: Shading,
    pub ambient: f32,
    pub diffuse: f32,
    pub specular: f32,
    /// 0 = broad highlight, 1 = tight/sharp highlight (maps to a specular exponent).
    pub shininess: f32,
    pub opacity: f32,
    /// VMD "Outline": silhouette/edge darkening at grazing angles (0 = off). Packed
    /// as a flag (the top bit of the shininess byte) with a fixed shader strength.
    pub outline: f32,
    pub outline_width: f32,
}

impl Material {
    pub fn preset(self) -> Self {
        match self {
            Self::Custom { preset, .. } => Self::ALL.get(preset as usize).copied().unwrap_or(Self::Opaque),
            _ => self,
        }
    }
    pub fn options(self) -> MaterialOptions {
        if let Self::Custom { options, .. } = self { return options; }
        let p = self.params();
        MaterialOptions { ambient: p.ambient, diffuse: p.diffuse, specular: p.specular,
            shininess: p.shininess, opacity: p.opacity, outline: p.outline, outline_width: 0.5 }
    }
    pub fn with_options(self, options: MaterialOptions) -> Self {
        let preset = Self::ALL.iter().position(|&m| m == self.preset()).unwrap_or(0) as u8;
        Self::Custom { preset, options }
    }

    pub const ALL: [Material; 13] = [
        Material::Opaque,
        Material::Transparent,
        Material::Glass,
        Material::Translucent,
        Material::Ghost,
        Material::Glossy,
        Material::Diffuse,
        Material::Metal,
        Material::AoChalky,
        Material::AoShiny,
        Material::AoEdgy,
        Material::FlatOutline,
        Material::MolecularNodes,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Material::Opaque => "Opaque",
            Material::Transparent => "Transparent",
            Material::Glass => "Glass",
            Material::Translucent => "Translucent",
            Material::Ghost => "Ghost",
            Material::Glossy => "Glossy",
            Material::Diffuse => "Diffuse",
            Material::Metal => "Metal",
            Material::AoChalky => "AO Chalky",
            Material::AoShiny => "AO Shiny",
            Material::AoEdgy => "AO Edgy",
            Material::FlatOutline => "Outline",
            Material::MolecularNodes => "Mat. Nodes",
            Material::Custom { .. } => self.preset().label(),
        }
    }

    pub fn params(self) -> MaterialParams {
        let m = |ambient, diffuse, specular, shininess, opacity, outline| MaterialParams {
            shading: Shading::Classic,
            ambient,
            diffuse,
            specular,
            shininess,
            opacity,
            outline,
            outline_width: 0.5,
        };
        match self {
            Material::Custom { options: p, .. } => {
                let mut result = self.preset().params();
                result.ambient = p.ambient; result.diffuse = p.diffuse;
                result.specular = p.specular; result.shininess = p.shininess;
                result.opacity = p.opacity; result.outline = p.outline; result.outline_width = p.outline_width;
                result
            }
            Material::Opaque => m(0.10, 0.75, 0.45, 0.55, 1.00, 0.0),
            Material::Transparent => m(0.10, 0.75, 0.45, 0.55, 0.30, 0.0),
            Material::Glass => m(0.10, 0.45, 0.90, 0.85, 0.50, 0.0),
            Material::Translucent => m(0.10, 0.75, 0.45, 0.55, 0.70, 0.0),
            Material::Ghost => m(0.00, 0.20, 1.00, 0.55, 0.15, 0.0),
            Material::Glossy => m(0.05, 0.65, 1.00, 0.95, 1.00, 0.0),
            Material::Diffuse => m(0.18, 0.90, 0.00, 0.00, 1.00, 0.0),
            Material::Metal => m(0.10, 0.35, 0.95, 0.30, 1.00, 0.0),
            // VMD's AO materials use ambient 0 (AO + sky light fills the shadows);
            // we keep a small ambient so they're not pitch-black without AO yet.
            Material::AoChalky => m(0.12, 1.00, 0.00, 0.00, 1.00, 0.0),
            Material::AoShiny => m(0.08, 0.85, 0.50, 0.85, 1.00, 0.0),
            // AOChalky + a silhouette outline.
            Material::AoEdgy => m(0.12, 1.00, 0.00, 0.00, 1.00, 0.7),
            Material::FlatOutline => MaterialParams {
                shading: Shading::FlatOutline, ..m(1.0, 0.0, 0.0, 0.0, 1.0, 1.0)
            },
            Material::MolecularNodes => MaterialParams {
                shading: Shading::MolecularNodes, ..m(0.34, 0.62, 0.3, 0.4, 1.0, 0.0)
            },
        }
    }

    /// Opacity as a u8 for the color's alpha channel.
    pub fn opacity_u8(self) -> u8 {
        (self.params().opacity.clamp(0.0, 1.0) * 255.0).round() as u8
    }

    /// Pack the lighting coefficients into a `u32`:
    /// `ambient | diffuse<<8 | specular<<16 | shininess<<24` (each a u8). The
    /// shininess byte uses its low **7 bits** for shininess and the **top bit** as
    /// the VMD `outline` flag (silhouette darkening). The shaders unpack this per
    /// fragment (see the impostor/mesh shaders). Specialized shading presets use
    /// reserved words instead of coefficient packing.
    pub fn pack_lighting(self) -> u32 {
        match self {
            Self::Custom { options: p, .. } => {
                let q = |v: f32| (v.clamp(0.0, 1.0) * 127.0).round() as u32;
                let mode = match self.preset() {
                    Self::FlatOutline => 0xa,
                    Self::MolecularNodes => 0xb,
                    _ => if p.outline > 0.5 { 0xd } else { 0xc },
                };
                let values = if mode == 0xa { [p.outline_width, p.outline, 0.0, 0.0] }
                    else { [p.ambient, p.diffuse, p.specular, p.shininess] };
                return (mode << 28) | q(values[0]) | (q(values[1]) << 7)
                    | (q(values[2]) << 14) | (q(values[3]) << 21);
            }
            Self::FlatOutline => return FLAT_OUTLINE_WORD,
            Self::MolecularNodes => return MOLECULAR_NODES_WORD,
            _ => {}
        }
        let p = self.params();
        let q = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
        let q7 = |x: f32| (x.clamp(0.0, 1.0) * 127.0).round() as u32;
        let shin_byte = (q7(p.shininess) & 0x7f) | (u32::from(p.outline > 0.5) << 7);
        q(p.ambient) | (q(p.diffuse) << 8) | (q(p.specular) << 16) | (shin_byte << 24)
    }

    /// Whether this material needs the alpha-blended (transparent) pass.
    pub fn is_transparent(self) -> bool {
        self.params().opacity < 0.999
    }
}


/// CPU version of the studio shader, used by material picker previews.
pub(crate) fn shade_molecular_nodes(base: glam::Vec3, normal: glam::Vec3, view: glam::Vec3, params: &MaterialParams) -> glam::Vec3 {
    let light = |direction: glam::Vec3| {
        let l = direction.normalize();
        let nl = normal.dot(l).max(0.0);
        let nv = normal.dot(view).max(0.001);
        let h = (view + l).normalize();
        let nh = normal.dot(h).max(0.0);
        let vh = view.dot(h).max(0.0);
        let roughness = params.shininess.clamp(0.05, 1.0);
        let alpha2 = roughness.powi(4);
        let d = alpha2 / (std::f32::consts::PI * (nh * nh * (alpha2 - 1.0) + 1.0).powi(2));
        let k = (roughness + 1.0).powi(2) / 8.0;
        let g = nv / (nv * (1.0 - k) + k) * nl / (nl * (1.0 - k) + k);
        let f = 0.04 + 0.96 * (1.0 - vh).powi(5);
        let spec = d * g * f / (4.0 * nv).max(0.001);
        base * (1.0 - f) * nl + glam::Vec3::splat(spec * params.specular / 0.3)
    };
    base * params.ambient + light(glam::vec3(-0.45, 0.65, 1.0)) * params.diffuse
        + light(glam::vec3(0.7, -0.2, 0.9)) * (params.diffuse * (0.28 / 0.62))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specialized_materials_round_trip_without_colliding_with_classic_words() {
        for material in [Material::FlatOutline, Material::MolecularNodes] {
            let restored: Material = serde_json::from_str(&serde_json::to_string(&material).unwrap()).unwrap();
            assert_eq!(material, restored);
            assert!(!material.is_transparent());
            assert!(Material::ALL.iter().all(|&other|
                other == material || other.pack_lighting() != material.pack_lighting()));
            assert_eq!(crate::script::command::parse_material(material.label()), Some(material));
        }
    }
}
