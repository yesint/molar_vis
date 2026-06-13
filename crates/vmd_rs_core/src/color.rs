//! Color tables and (from M5) per-scheme atom coloring.
//!
//! Colors are returned as RGBA8 and packed little-endian into a `u32` for use as
//! a per-instance vertex attribute (`r | g<<8 | b<<16 | a<<24`), matching the
//! `unpack_color` helper in the WGSL shaders.

/// CPK-style element colors as RGBA8, indexed by atomic number. Unknown elements
/// render magenta so they stand out.
pub fn element_color(atomic_number: u8) -> [u8; 4] {
    let rgb: [u8; 3] = match atomic_number {
        1 => [240, 240, 240],  // H  white
        6 => [144, 144, 144],  // C  grey
        7 => [48, 80, 248],    // N  blue
        8 => [255, 40, 40],    // O  red
        9 => [144, 224, 80],   // F  green
        11 => [171, 92, 242],  // Na violet
        12 => [138, 255, 0],   // Mg
        15 => [255, 128, 0],   // P  orange
        16 => [255, 220, 48],  // S  yellow
        17 => [31, 240, 31],   // Cl green
        19 => [143, 64, 212],  // K
        20 => [61, 255, 0],    // Ca
        26 => [224, 102, 51],  // Fe
        30 => [125, 128, 176], // Zn
        35 => [166, 41, 41],   // Br
        53 => [148, 0, 148],   // I
        _ => [255, 0, 255],    // unknown / unassigned
    };
    [rgb[0], rgb[1], rgb[2], 255]
}

/// Pack RGBA8 into a little-endian `u32` for upload as a vertex attribute.
pub fn pack_rgba8(c: [u8; 4]) -> u32 {
    (c[0] as u32) | ((c[1] as u32) << 8) | ((c[2] as u32) << 16) | ((c[3] as u32) << 24)
}
