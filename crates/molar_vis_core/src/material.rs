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
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
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
}

/// Lighting + opacity coefficients (each 0..1).
pub struct MaterialParams {
    pub ambient: f32,
    pub diffuse: f32,
    pub specular: f32,
    /// 0 = broad highlight, 1 = tight/sharp highlight (maps to a specular exponent).
    pub shininess: f32,
    pub opacity: f32,
}

impl Material {
    pub const ALL: [Material; 8] = [
        Material::Opaque,
        Material::Transparent,
        Material::Glass,
        Material::Translucent,
        Material::Ghost,
        Material::Glossy,
        Material::Diffuse,
        Material::Metal,
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
        }
    }

    pub fn params(self) -> MaterialParams {
        let m = |ambient, diffuse, specular, shininess, opacity| MaterialParams {
            ambient,
            diffuse,
            specular,
            shininess,
            opacity,
        };
        match self {
            Material::Opaque => m(0.10, 0.75, 0.45, 0.55, 1.00),
            Material::Transparent => m(0.10, 0.75, 0.45, 0.55, 0.30),
            Material::Glass => m(0.10, 0.45, 0.90, 0.85, 0.50),
            Material::Translucent => m(0.10, 0.75, 0.45, 0.55, 0.70),
            Material::Ghost => m(0.00, 0.20, 1.00, 0.55, 0.15),
            Material::Glossy => m(0.05, 0.65, 1.00, 0.95, 1.00),
            Material::Diffuse => m(0.18, 0.90, 0.00, 0.00, 1.00),
            Material::Metal => m(0.10, 0.35, 0.95, 0.30, 1.00),
        }
    }

    /// Opacity as a u8 for the color's alpha channel.
    pub fn opacity_u8(self) -> u8 {
        (self.params().opacity.clamp(0.0, 1.0) * 255.0).round() as u8
    }

    /// Pack the lighting coefficients into a `u32`:
    /// `ambient | diffuse<<8 | specular<<16 | shininess<<24` (each a u8). The
    /// shaders unpack this per fragment (see the impostor/mesh shaders).
    pub fn pack_lighting(self) -> u32 {
        let p = self.params();
        let q = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
        q(p.ambient) | (q(p.diffuse) << 8) | (q(p.specular) << 16) | (q(p.shininess) << 24)
    }

    /// Whether this material needs the alpha-blended (transparent) pass.
    pub fn is_transparent(self) -> bool {
        self.params().opacity < 0.999
    }
}
