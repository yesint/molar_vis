//! Viewer color presets for elements, secondary structure and continuous values.
//!
//! VMD values come from colordefs.dat and Scene.C; PyMOL from Color.cpp and
//! menu.py; Mol* from its element-symbol/secondary-structure themes and color lists.
//! Sources:
//! https://github.com/thatchristoph/vmd-cvs-github/blob/master/vmd/scripts/vmd/colordefs.dat
//! https://github.com/thatchristoph/vmd-cvs-github/blob/master/vmd/src/Scene.C
//! https://github.com/schrodinger/pymol-open-source/blob/master/layer1/Color.cpp
//! https://github.com/schrodinger/pymol-open-source/blob/master/modules/pymol/menu.py
//! https://github.com/molstar/molstar/blob/master/packages/graphics/src/theme/color/element-symbol.ts
//! https://github.com/molstar/molstar/blob/master/packages/graphics/src/theme/color/secondary-structure.ts
//! https://github.com/molstar/molstar/blob/master/packages/core/src/util/color/lists.ts
//! Discovery Studio SS conventions: doi:10.3390/md16090341, Figure 8.
//! MOE scalar conventions: https://svl.chemcomp.com/ (Colors atoms by scalar).
//! Discovery Studio and MOE are visual approximations: published illustrations
//! establish their conventions, but complete RGB defaults could not be verified.
//! Unspecified elements use the Mol* element table. Rare SS types are mapped to
//! the closest supported family. Continuous scales use three stops, and all
//! scales run toward red at the high endpoint to retain our charge convention.

use std::sync::OnceLock;

use crate::color::{ColorPreferences, Gradient};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorPreset {
    Default,
    Vmd,
    Pymol,
    DiscoveryStudio,
    Moe,
    Molstar,
}

impl ColorPreset {
    pub const ALL: [Self; 6] = [
        Self::Default,
        Self::Vmd,
        Self::Pymol,
        Self::DiscoveryStudio,
        Self::Moe,
        Self::Molstar,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Vmd => "VMD",
            Self::Pymol => "PyMOL",
            Self::DiscoveryStudio => "Discovery Studio",
            Self::Moe => "MOE",
            Self::Molstar => "Mol*",
        }
    }

    pub fn parse(name: &str) -> Result<Self, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "default" | "molar_vis" => Ok(Self::Default),
            "vmd" => Ok(Self::Vmd),
            "pymol" => Ok(Self::Pymol),
            "discovery studio" | "discoverystudio" => Ok(Self::DiscoveryStudio),
            "moe" => Ok(Self::Moe),
            "mol*" | "molstar" => Ok(Self::Molstar),
            _ => Err(format!("unknown color preset: {name}; expected Default, VMD, PyMOL, Discovery Studio, MOE or Mol*")),
        }
    }

    fn colors(self) -> &'static ColorPreferences {
        static PRESETS: OnceLock<[ColorPreferences; 6]> = OnceLock::new();
        &PRESETS.get_or_init(|| Self::ALL.map(Self::build))[self as usize]
    }

    /// Replace atom, SS and gradient colors, retaining the user's discrete palette.
    pub fn apply(self, colors: &mut ColorPreferences) {
        let preset = self.colors();
        colors.elements.clone_from(&preset.elements);
        colors.secondary = preset.secondary;
        colors.continuous = preset.continuous;
    }

    fn build(self) -> ColorPreferences {
        let mut colors = ColorPreferences::default();
        if self == Self::Default {
            return colors;
        }
        colors.elements = MOLSTAR_ELEMENTS
            .iter()
            .enumerate()
            .map(|(number, &hex)| (number as u8, rgba(hex)))
            .collect();
        let (secondary, stops) = match self {
            Self::Vmd => {
                // VMD assigns ochre to all elements except H/C/N/O/P/S. Elements
                // added after this table use the same fallback. Unknown is cyan.
                for color in colors.elements.values_mut() {
                    *color = rgba(0x804d00);
                }
                for (number, hex) in [
                    (0, 0x40bfbf),
                    (1, 0xffffff),
                    (6, 0x40bfbf),
                    (7, 0x0000ff),
                    (8, 0xff0000),
                    (15, 0x808033),
                    (16, 0xffff00),
                ] {
                    colors.elements.insert(number, rgba(hex));
                }
                // VMD has no bend/polyproline categories; both use Coil. BWR is
                // VMD's supported inverse of RWB, preserving positive-red charges.
                (
                    [
                        0xa600a6, 0x0000ff, 0xff0000, 0xffff00, 0x808033, 0x40bfbf, 0xffffff,
                        0xffffff, 0xffffff,
                    ],
                    [0x0000ff, 0xffffff, 0xff0000],
                )
            }
            Self::Pymol => {
                for &(number, hex) in PYMOL_ELEMENT_OVERRIDES {
                    colors.elements.insert(number, rgba(hex));
                }
                colors.elements.insert(0, rgba(0xe6e6e6));
                // PyMOL's first "by ss" menu palette: red/yellow/green. It uses
                // three families, so all helices/sheets/nonregular classes follow
                // their family. The gradient is its blue_white_red scale.
                (
                    [
                        0xff0000, 0xff0000, 0xff0000, 0xffff00, 0xffff00, 0x00ff00, 0x00ff00,
                        0x00ff00, 0x00ff00,
                    ],
                    [0x0000ff, 0xffffff, 0xff0000],
                )
            }
            Self::DiscoveryStudio => {
                // Visual approximation of Discovery Studio's CPK atoms and
                // red helices / blue sheets / green turns / grey coils.
                for (number, hex) in [
                    (0, 0x808080),
                    (1, 0xffffff),
                    (6, 0x909090),
                    (7, 0x0000ff),
                    (8, 0xff0000),
                    (9, 0x00ff00),
                    (15, 0xff8000),
                    (16, 0xffff00),
                    (17, 0x00ff00),
                ] {
                    colors.elements.insert(number, rgba(hex));
                }
                (
                    [
                        0xff0000, 0xff0000, 0xff0000, 0x0000ff, 0x0000ff, 0x00a000, 0x00a000,
                        0x808080, 0x808080,
                    ],
                    [0x0000ff, 0xffffff, 0xff0000],
                )
            }
            Self::Moe => {
                // Visual approximation of MOE's element and SS conventions;
                // scalar coloring spans violet to red with green in the middle.
                for (number, hex) in [
                    (0, 0xff00ff),
                    (1, 0xffffff),
                    (6, 0x808080),
                    (7, 0x0000ff),
                    (8, 0xff0000),
                    (9, 0x00ff00),
                    (15, 0xff8000),
                    (16, 0xffff00),
                    (17, 0x00ff00),
                ] {
                    colors.elements.insert(number, rgba(hex));
                }
                (
                    [
                        0xff0000, 0xff0000, 0xff0000, 0xffff00, 0xffff00, 0x0000ff, 0x0000ff,
                        0xffffff, 0xffffff,
                    ],
                    [0x8000ff, 0x00ff00, 0xff0000],
                )
            }
            Self::Molstar => {
                // Use Mol*'s unadjusted base tables: no chain-based carbon colors
                // or theme saturation/lightness modifiers. Beta bridges share
                // betaStrand; polyproline uses coil. RdYlBu is reduced to three
                // stops and reversed to preserve our charge sign convention.
                (
                    [
                        0xff0080, 0xa00080, 0x600080, 0xffc800, 0xffc800, 0x00b266, 0x66d8c9,
                        0xffffff, 0xffffff,
                    ],
                    [0x313695, 0xffffbf, 0xa50026],
                )
            }
            Self::Default => unreachable!(),
        };
        colors.secondary = secondary.map(rgba);
        colors.continuous = Gradient {
            midpoint: 0.5,
            stops: stops.map(rgba),
        };
        colors
    }
}

impl ColorPreferences {
    /// Identify the active preset from its actual colors, also after loading an
    /// older session or editing through an API. None means custom colors.
    pub fn preset(&self) -> Option<ColorPreset> {
        ColorPreset::ALL.into_iter().find(|preset| {
            let reference = preset.colors();
            self.secondary == reference.secondary
                && self.continuous == reference.continuous
                && (0..=118).all(|number| self.element(number) == reference.element(number))
        })
    }
}

const fn rgba(hex: u32) -> [u8; 4] {
    [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8, 255]
}

// Mol* ElementSymbolColors, indexed by atomic number (0 = unknown).
// Source: packages/graphics/src/theme/color/element-symbol.ts (MIT).
const MOLSTAR_ELEMENTS: [u32; 119] = [
    0xffffff, 0xffffff, 0xd9ffff, 0xcc80ff, 0xc2ff00, 0xffb5b5, 0x909090,
    0x3050f8, // Unknown H He Li Be B C N
    0xff0d0d, 0x90e050, 0xb3e3f5, 0xab5cf2, 0x8aff00, 0xbfa6a6, 0xf0c8a0,
    0xff8000, // O F Ne Na Mg Al Si P
    0xffff30, 0x1ff01f, 0x80d1e3, 0x8f40d4, 0x3dff00, 0xe6e6e6, 0xbfc2c7,
    0xa6a6ab, // S Cl Ar K Ca Sc Ti V
    0x8a99c7, 0x9c7ac7, 0xe06633, 0xf090a0, 0x50d050, 0xc88033, 0x7d80b0,
    0xc28f8f, // Cr Mn Fe Co Ni Cu Zn Ga
    0x668f8f, 0xbd80e3, 0xffa100, 0xa62929, 0x5cb8d1, 0x702eb0, 0x00ff00,
    0x94ffff, // Ge As Se Br Kr Rb Sr Y
    0x94e0e0, 0x73c2c9, 0x54b5b5, 0x3b9e9e, 0x248f8f, 0x0a7d8c, 0x006985,
    0xc0c0c0, // Zr Nb Mo Tc Ru Rh Pd Ag
    0xffd98f, 0xa67573, 0x668080, 0x9e63b5, 0xd47a00, 0x940094, 0x940094,
    0x57178f, // Cd In Sn Sb Te I Xe Cs
    0x00c900, 0x70d4ff, 0xffffc7, 0xd9ffc7, 0xc7ffc7, 0xa3ffc7, 0x8fffc7,
    0x61ffc7, // Ba La Ce Pr Nd Pm Sm Eu
    0x45ffc7, 0x30ffc7, 0x1fffc7, 0x00ff9c, 0x00e675, 0x00d452, 0x00bf38,
    0x00ab24, // Gd Tb Dy Ho Er Tm Yb Lu
    0x4dc2ff, 0x4da6ff, 0x2194d6, 0x267dab, 0x266696, 0x175487, 0xd0d0e0,
    0xffd123, // Hf Ta W Re Os Ir Pt Au
    0xb8b8d0, 0xa6544d, 0x575961, 0x9e4fb5, 0xab5c00, 0x754f45, 0x428296,
    0x420066, // Hg Tl Pb Bi Po At Rn Fr
    0x007d00, 0x70abfa, 0x00baff, 0x00a1ff, 0x008fff, 0x0080ff, 0x006bff,
    0x545cf2, // Ra Ac Th Pa U Np Pu Am
    0x785ce3, 0x8a4fe3, 0xa136d4, 0xb31fd4, 0xb31fba, 0xb30da6, 0xbd0d87,
    0xc70066, // Cm Bk Cf Es Fm Md No Lr
    0xcc0059, 0xd1004f, 0xd90045, 0xe00038, 0xe6002e, 0xeb0026, 0xffffff,
    0xffffff, // Rf Db Sg Bh Hs Mt Ds Rg
    0xffffff, 0xffffff, 0xffffff, 0xffffff, 0xffffff, 0xffffff,
    0xffffff, // Cn Nh Fl Mc Lv Ts Og
];

// PyMOL Color.cpp named element colors relative to the table above.
const PYMOL_ELEMENT_OVERRIDES: &[(u8, u32)] = &[
    (1, 0xe6e6e6),  // H
    (6, 0x33ff33),  // C
    (7, 0x3333ff),  // N
    (8, 0xff4d4d),  // O
    (9, 0xb3ffff),  // F
    (16, 0xe6c640), // S
    (54, 0x429eb0), // Xe
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_presets_replaces_custom_colors_and_preserves_discrete_palette() {
        let palette = vec![[12, 34, 56, 255], [90, 80, 70, 255]];
        let mut colors = ColorPreferences::default();
        colors.categorical = palette.clone();
        for preset in ColorPreset::ALL.into_iter().cycle().take(12) {
            colors.elements.insert(118, [1, 2, 3, 255]);
            colors.secondary[0] = [4, 5, 6, 255];
            colors.continuous.midpoint = 0.25;
            preset.apply(&mut colors);
            assert_eq!(colors.preset(), Some(preset));
            assert_eq!(colors.categorical, palette);
            assert_eq!(colors.continuous.midpoint, 0.5);
            assert_ne!(colors.element(118), [1, 2, 3, 255]);
        }
    }

    #[test]
    fn edits_are_custom_but_equivalent_element_overrides_keep_the_preset_name() {
        for preset in ColorPreset::ALL {
            let mut colors = ColorPreferences::default();
            preset.apply(&mut colors);
            let base = colors.clone();
            colors.elements.insert(6, colors.element(6));
            assert_eq!(colors.preset(), Some(preset));
            colors.elements.insert(6, [1, 2, 3, 255]);
            assert_eq!(colors.preset(), None);
            colors = base.clone();
            colors.secondary[0] = [1, 2, 3, 255];
            assert_eq!(colors.preset(), None);
            colors = base;
            colors.continuous.midpoint = 0.3;
            assert_eq!(colors.preset(), None);
        }
    }

    #[test]
    fn presets_and_custom_edits_roundtrip_through_settings_and_sessions() {
        for preset in ColorPreset::ALL {
            let mut settings = crate::settings::Settings::default();
            preset.apply(&mut settings.colors);
            for custom in [false, true] {
                if custom {
                    settings.colors.continuous.midpoint = 0.3;
                }
                let expected = if custom { None } else { Some(preset) };
                let json = serde_json::to_string(&settings).unwrap();
                let loaded: crate::settings::Settings = serde_json::from_str(&json).unwrap();
                assert_eq!(loaded.colors, settings.colors);
                assert_eq!(loaded.colors.preset(), expected);
                let view = crate::session::ViewState {
                    colors: Some(settings.colors.clone()),
                    ..Default::default()
                };
                let json = serde_json::to_string(&view).unwrap();
                let loaded: crate::session::ViewState = serde_json::from_str(&json).unwrap();
                let colors = loaded.colors.unwrap();
                assert_eq!(colors, settings.colors);
                assert_eq!(colors.preset(), expected);
            }
        }
        let old: crate::settings::Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.colors.preset(), Some(ColorPreset::Default));
    }

    #[test]
    fn source_colors_cover_organic_and_heavy_elements_and_charge_orientation() {
        let mut colors = ColorPreferences::default();
        ColorPreset::Pymol.apply(&mut colors);
        assert_eq!(colors.element(6), [51, 255, 51, 255]);
        assert_eq!(colors.element(8), [255, 77, 77, 255]);
        assert_eq!(colors.element(9), [179, 255, 255, 255]);
        assert_eq!(colors.element(79), [255, 209, 35, 255]);
        ColorPreset::Vmd.apply(&mut colors);
        assert_eq!(colors.element(6), [64, 191, 191, 255]);
        assert_eq!(colors.element(15), [128, 128, 51, 255]);
        ColorPreset::Molstar.apply(&mut colors);
        assert_eq!(colors.element(8), [255, 13, 13, 255]);
        assert_eq!(colors.secondary[0], [255, 0, 128, 255]);
        for preset in ColorPreset::ALL {
            preset.apply(&mut colors);
            let high = colors.continuous.sample(1.0);
            assert!(high[0] > high[2], "{} must end in red", preset.name());
            for number in 0..=118 {
                assert_eq!(colors.element(number)[3], 255);
            }
            assert_eq!(ColorPreset::parse(preset.name()), Ok(preset));
        }
        assert!(ColorPreset::parse("Custom").is_err());
    }
}
