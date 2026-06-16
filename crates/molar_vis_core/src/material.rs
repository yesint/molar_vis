//! VMD-style materials: a per-representation appearance preset controlling
//! lighting (ambient / diffuse / specular / shininess) and **opacity**.
//!
//! The values are GPU-packed per geometry element: the four lighting
//! coefficients pack into a single `u32` (`mat`, carried per instance/vertex) and
//! the opacity rides in the alpha channel of the element's color. Shaders unpack
//! both; transparent materials (`opacity < 1`) are drawn in a second,
//! depth-write-off, alpha-blended pass.

/// A representation's material preset. Values approximate VMD's built-in
/// materials (the real-time-relevant subset; ray-tracing-only ones are omitted).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
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
}

/// Lighting + opacity coefficients (each 0..1).
pub struct MaterialParams {
    pub ambient: f32,
    pub diffuse: f32,
    pub specular: f32,
    /// 0 = broad highlight, 1 = tight/sharp highlight (maps to a specular exponent).
    pub shininess: f32,
    pub opacity: f32,
    /// VMD "Outline": silhouette/edge darkening at grazing angles (0 = off). Packed
    /// as a flag (the top bit of the shininess byte) with a fixed shader strength.
    pub outline: f32,
}

impl Material {
    pub const ALL: [Material; 11] = [
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
        }
    }

    pub fn params(self) -> MaterialParams {
        let m = |ambient, diffuse, specular, shininess, opacity, outline| MaterialParams {
            ambient,
            diffuse,
            specular,
            shininess,
            opacity,
            outline,
        };
        match self {
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
    /// fragment (see the impostor/mesh shaders).
    pub fn pack_lighting(self) -> u32 {
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
