//! Molecule-owned, static meshes. PLY coordinates are in viewer world units (nm).
use crate::{
    geometry::{GeometryData, MeshData},
    material::Material,
    render::{LineVertex, MeshVertex, PointInstance, RepGpu},
};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

static REVISION: AtomicU64 = AtomicU64::new(1);
static MESH_ID: AtomicU64 = AtomicU64::new(1);

/// Reserve a stable ID before queueing an external mesh addition.
pub fn reserve_mesh_id() -> u64 {
    MESH_ID.fetch_add(1, Ordering::Relaxed)
}


#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [u8; 4],
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshSource {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl MeshSource {
    pub fn validate(&self) -> Result<(), String> {
        if self.vertices.is_empty() || self.indices.is_empty() || self.indices.len() % 3 != 0 {
            return Err("mesh must contain vertices and triangle faces".into());
        }
        if self
            .indices
            .iter()
            .any(|&i| i as usize >= self.vertices.len())
        {
            return Err("mesh face index out of range".into());
        }
        if self
            .vertices
            .iter()
            .any(|v| v.position.iter().chain(&v.normal).any(|f| !f.is_finite()))
        {
            return Err("mesh contains non-finite coordinates or normals".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeshStyle {
    Points,
    Wireframe,
    #[default]
    Surface,
}
impl MeshStyle {
    pub const ALL: [Self; 3] = [Self::Points, Self::Wireframe, Self::Surface];
    pub fn label(self) -> &'static str {
        match self {
            Self::Points => "Points",
            Self::Wireframe => "Wireframe",
            Self::Surface => "Surface",
        }
    }
}
fn point_size() -> f32 {
    3.0
}
fn line_width() -> f32 {
    1.0
}

/// Shared immutable geometry keeps session and undo snapshots independent of GPU data.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshState {
    pub name: String,
    pub source: Arc<MeshSource>,
    #[serde(default = "visible")]
    pub visible: bool,
    #[serde(default)]
    pub material: Material,
    #[serde(default)]
    pub style: MeshStyle,
    #[serde(default = "point_size")]
    pub point_size: f32,
    #[serde(default = "line_width")]
    pub line_width: f32,
    #[serde(default)]
    pub smoothing: u32,
}
fn visible() -> bool {
    true
}

pub struct Mesh {
    pub id: u64,
    pub state: MeshState,
    pub gpu: RepGpu,
    pub dirty: bool,
    pub revision: u64,
    has_alpha: bool,
    pub closed: bool,
    edges: Vec<(u32, u32)>,
    neighbors: Vec<Vec<usize>>,
    boundary: Vec<bool>,
    smoothed: RefCell<Option<(u32, Arc<MeshSource>)>>,
}
impl Mesh {
    pub fn new(name: String, source: MeshSource) -> Self {
        Self::from_state(MeshState {
            name,
            source: Arc::new(source),
            visible: true,
            material: Material::Opaque,
            style: MeshStyle::Surface,
            point_size: point_size(),
            line_width: line_width(),
            smoothing: 0,
        })
    }
    pub fn from_state(state: MeshState) -> Self {
        let has_alpha = state.source.vertices.iter().any(|v| v.color[3] < 255);
        let mut edges = std::collections::HashMap::<(u32, u32), (u32, i32)>::new();
        for t in state.source.indices.chunks_exact(3) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let edge = edges.entry((a.min(b), a.max(b))).or_default();
                edge.0 += 1;
                edge.1 += if a < b { 1 } else { -1 };
            }
        }
        let closed = !edges.is_empty() && edges.values().all(|&e| e == (2, 0));
        let mut neighbors = vec![Vec::new(); state.source.vertices.len()];
        let mut boundary = vec![false; state.source.vertices.len()];
        for (&(a, b), &(count, _)) in &edges {
            neighbors[a as usize].push(b as usize);
            neighbors[b as usize].push(a as usize);
            if count != 2 {
                boundary[a as usize] = true;
                boundary[b as usize] = true;
            }
        }
        let mut edges: Vec<_> = edges.into_keys().collect();
        edges.sort_unstable();
        for ring in &mut neighbors {
            ring.sort_unstable();
        }
        Self {
            id: reserve_mesh_id(),
            state,
            gpu: RepGpu::default(),
            dirty: true,
            revision: REVISION.fetch_add(1, Ordering::Relaxed),
            has_alpha,
            closed,
            edges,
            neighbors,
            boundary,
            smoothed: RefCell::new(None),
        }
    }
    pub fn uploaded(&mut self) {
        self.dirty = false;
        self.revision = REVISION.fetch_add(1, Ordering::Relaxed);
    }
    /// Smoothing always starts from the imported mesh, so changing the slider is reversible.
    /// Taubin's paired passes reduce shrinkage; open boundaries stay fixed.
    pub fn surface_source(&self) -> Arc<MeshSource> {
        let steps = self.state.smoothing.min(5);
        if steps == 0 {
            return self.state.source.clone();
        }
        if let Some((cached_steps, source)) = self.smoothed.borrow().as_ref() {
            if *cached_steps == steps {
                return source.clone();
            }
        }
        let mut source = (*self.state.source).clone();
        let mut positions: Vec<_> = source
            .vertices
            .iter()
            .map(|v| glam::Vec3::from(v.position).as_dvec3())
            .collect();
        let mut next = positions.clone();
        for _ in 0..steps {
            for factor in [0.5, -0.53] {
                for (i, ring) in self.neighbors.iter().enumerate() {
                    next[i] = positions[i];
                    if self.boundary[i] || ring.is_empty() {
                        continue;
                    }
                    let mean =
                        ring.iter().map(|&j| positions[j]).sum::<glam::DVec3>() / ring.len() as f64;
                    next[i] += factor * (mean - positions[i]);
                }
                std::mem::swap(&mut positions, &mut next);
            }
        }
        for (v, p) in source.vertices.iter_mut().zip(&positions) {
            v.position = p.as_vec3().to_array();
        }
        let mut normals = vec![glam::DVec3::ZERO; positions.len()];
        for t in source.indices.chunks_exact(3) {
            let n = (positions[t[1] as usize] - positions[t[0] as usize])
                .cross(positions[t[2] as usize] - positions[t[0] as usize]);
            for &i in t {
                normals[i as usize] += n;
            }
        }
        for (v, n) in source.vertices.iter_mut().zip(normals) {
            if n.length_squared() > 0.0 {
                v.normal = n.normalize().as_vec3().to_array();
            }
        }
        let source = Arc::new(source);
        *self.smoothed.borrow_mut() = Some((steps, source.clone()));
        source
    }
    fn color(&self, color: [u8; 4]) -> u32 {
        let mut c = color;
        c[3] = (c[3] as f32 * self.state.material.params().opacity).round() as u8;
        u32::from_le_bytes(c)
    }
    pub fn draw_geometry(&self) -> GeometryData {
        match self.state.style {
            MeshStyle::Surface => GeometryData {
                mesh: self.geometry(),
                ..Default::default()
            },
            MeshStyle::Points => GeometryData {
                points: self
                    .state
                    .source
                    .vertices
                    .iter()
                    .map(|v| PointInstance {
                        pos: v.position,
                        size: self.state.point_size.clamp(1.0, 10.0),
                        color: self.color(v.color),
                    })
                    .collect(),
                ..Default::default()
            },
            MeshStyle::Wireframe => GeometryData {
                lines: self
                    .edges
                    .iter()
                    .flat_map(|&(a, b)| [a, b])
                    .map(|i| {
                        let v = &self.state.source.vertices[i as usize];
                        LineVertex {
                            pos: v.position,
                            color: self.color(v.color),
                            width: self.state.line_width.clamp(1.0, 10.0),
                            offset_px: 0.0,
                        }
                    })
                    .collect(),
                ..Default::default()
            },
        }
    }
    pub fn geometry(&self) -> MeshData {
        let source = self.surface_source();
        let mat = self.state.material.pack_lighting();
        MeshData {
            vertices: source
                .vertices
                .iter()
                .map(|v| MeshVertex {
                    pos: v.position,
                    normal: v.normal,
                    color: self.color(v.color),
                    mat,
                })
                .collect(),
            indices: source.indices.clone(),
            ..Default::default()
        }
    }
    pub fn transparent(&self) -> bool {
        self.state.material.is_transparent() || self.has_alpha
    }
}

#[derive(Clone, Copy)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}
impl Scalar {
    fn parse(s: &str) -> Result<Self, String> {
        Ok(match s {
            "char" | "int8" => Self::I8,
            "uchar" | "uint8" => Self::U8,
            "short" | "int16" => Self::I16,
            "ushort" | "uint16" => Self::U16,
            "int" | "int32" => Self::I32,
            "uint" | "uint32" => Self::U32,
            "float" | "float32" => Self::F32,
            "double" | "float64" => Self::F64,
            _ => return Err(format!("unsupported PLY property type: {s}")),
        })
    }
    fn integer(self) -> bool {
        !matches!(self, Self::F32 | Self::F64)
    }
}
struct Property {
    name: String,
    value: Scalar,
    count: Option<Scalar>,
}
struct Element {
    name: String,
    count: usize,
    properties: Vec<Property>,
}
enum Reader<'a> {
    Ascii(std::str::SplitWhitespace<'a>),
    Binary {
        bytes: &'a [u8],
        offset: usize,
        little: bool,
    },
}
impl Reader<'_> {
    fn read(&mut self, ty: Scalar) -> Result<f64, String> {
        let v = match self {
            Self::Ascii(tokens) => {
                let t = tokens.next().ok_or("truncated PLY data")?;
                match ty {
                    Scalar::I8 => t.parse::<i8>().map(|v| v as f64).map_err(|e| crate::errors::error_chain(&e)),
                    Scalar::U8 => t.parse::<u8>().map(|v| v as f64).map_err(|e| crate::errors::error_chain(&e)),
                    Scalar::I16 => t.parse::<i16>().map(|v| v as f64).map_err(|e| crate::errors::error_chain(&e)),
                    Scalar::U16 => t.parse::<u16>().map(|v| v as f64).map_err(|e| crate::errors::error_chain(&e)),
                    Scalar::I32 => t.parse::<i32>().map(|v| v as f64).map_err(|e| crate::errors::error_chain(&e)),
                    Scalar::U32 => t.parse::<u32>().map(|v| v as f64).map_err(|e| crate::errors::error_chain(&e)),
                    Scalar::F32 => t.parse::<f32>().map(|v| v as f64).map_err(|e| crate::errors::error_chain(&e)),
                    Scalar::F64 => t.parse::<f64>().map_err(|e| crate::errors::error_chain(&e)),
                }
                .map_err(|e| format!("invalid PLY value {t}: {e}"))?
            }
            Self::Binary {
                bytes,
                offset,
                little,
            } => {
                let n = match ty {
                    Scalar::I8 | Scalar::U8 => 1,
                    Scalar::I16 | Scalar::U16 => 2,
                    Scalar::I32 | Scalar::U32 | Scalar::F32 => 4,
                    Scalar::F64 => 8,
                };
                let b = bytes
                    .get(*offset..offset.checked_add(n).ok_or("PLY offset overflow")?)
                    .ok_or("truncated PLY data")?;
                *offset += n;
                macro_rules! number {
                    ($t:ty) => {{
                        let a = b.try_into().unwrap();
                        if *little {
                            <$t>::from_le_bytes(a) as f64
                        } else {
                            <$t>::from_be_bytes(a) as f64
                        }
                    }};
                }
                match ty {
                    Scalar::I8 => b[0] as i8 as f64,
                    Scalar::U8 => b[0] as f64,
                    Scalar::I16 => number!(i16),
                    Scalar::U16 => number!(u16),
                    Scalar::I32 => number!(i32),
                    Scalar::U32 => number!(u32),
                    Scalar::F32 => number!(f32),
                    Scalar::F64 => number!(f64),
                }
            }
        };
        if !v.is_finite() {
            return Err("non-finite PLY value".into());
        }
        Ok(v)
    }
    fn count(&mut self, ty: Scalar) -> Result<usize, String> {
        let v = self.read(ty)?;
        if !ty.integer() || v < 0.0 || v > u32::MAX as f64 {
            return Err("invalid PLY list count/index".into());
        }
        Ok(v as usize)
    }
}

/// Read ASCII or either binary byte order. Unknown properties/elements are consumed.
/// Polygon faces are triangulated; missing normals are area-weighted vertex normals.
pub fn import_ply(bytes: &[u8]) -> Result<MeshSource, String> {
    let mut offset = 0;
    let mut elements: Vec<Element> = Vec::new();
    let mut encoding = None;
    loop {
        let tail = bytes.get(offset..).ok_or("missing PLY header")?;
        let len = tail
            .iter()
            .position(|&b| b == b'\n')
            .ok_or("unterminated PLY header")?;
        let line = std::str::from_utf8(&tail[..len])
            .map_err(|e| format!("invalid PLY header: {}", crate::errors::error_chain(&e)))?
            .trim();
        offset += len + 1;
        if offset == len + 1 {
            if line != "ply" {
                return Err("not a PLY file".into());
            }
            continue;
        }
        let words: Vec<_> = line.split_whitespace().collect();
        match words.as_slice() {
            ["format", format, "1.0"] if encoding.is_none() => {
                encoding = Some(match *format {
                    "ascii" => 0,
                    "binary_little_endian" => 1,
                    "binary_big_endian" => 2,
                    _ => return Err("unsupported PLY encoding".into()),
                });
            }
            ["element", name, count] => {
                let count = count
                    .parse::<usize>()
                    .map_err(|e| format!("invalid PLY element count: {}", crate::errors::error_chain(&e)))?;
                // Each record must occupy at least one byte; reject absurd counts before allocation.
                if count > bytes.len() || count > u32::MAX as usize {
                    return Err("PLY element count exceeds file size".into());
                }
                elements.push(Element {
                    name: name.to_string(),
                    count,
                    properties: Vec::new(),
                });
            }
            ["property", ty, name] => {
                elements
                    .last_mut()
                    .ok_or("PLY property without element")?
                    .properties
                    .push(Property {
                        name: name.to_string(),
                        value: Scalar::parse(ty)?,
                        count: None,
                    });
            }
            ["property", "list", count, ty, name] => {
                let count = Scalar::parse(count)?;
                if !count.integer() {
                    return Err("PLY list count must be an integer".into());
                }
                elements
                    .last_mut()
                    .ok_or("PLY property without element")?
                    .properties
                    .push(Property {
                        name: name.to_string(),
                        value: Scalar::parse(ty)?,
                        count: Some(count),
                    });
            }
            ["end_header"] => break,
            ["comment", ..] | ["obj_info", ..] | [] => {}
            _ => return Err(format!("unsupported PLY header: {line}")),
        }
    }
    let data = &bytes[offset..];
    let mut reader = match encoding.ok_or("missing PLY format")? {
        0 => Reader::Ascii(
            std::str::from_utf8(data)
                .map_err(|e| format!("invalid ASCII PLY data: {}", crate::errors::error_chain(&e)))?
                .split_whitespace(),
        ),
        n => Reader::Binary {
            bytes: data,
            offset: 0,
            little: n == 1,
        },
    };
    let vertex = elements
        .iter()
        .find(|e| e.name == "vertex")
        .ok_or("missing PLY vertex element")?;
    if elements.iter().filter(|e| e.name == "vertex").count() != 1 {
        return Err("duplicate PLY vertex element".into());
    }
    let has = |name: &str| {
        vertex
            .properties
            .iter()
            .any(|p| p.name == name && p.count.is_none())
    };
    if !["x", "y", "z"].iter().all(|s| has(s)) {
        return Err("PLY vertices require x, y, z".into());
    }
    let normals = ["nx", "ny", "nz"].iter().filter(|s| has(s)).count();
    let colors = ["red", "green", "blue"].iter().filter(|s| has(s)).count();
    if normals != 0 && normals != 3 {
        return Err("incomplete PLY vertex normals".into());
    }
    if colors != 0 && colors != 3 {
        return Err("incomplete PLY vertex colors".into());
    }
    let mut out = MeshSource {
        vertices: Vec::new(),
        indices: Vec::new(),
    };
    let mut faces = Vec::new();
    for e in &elements {
        for _ in 0..e.count {
            let mut v = Vertex {
                position: [0.0; 3],
                normal: [0.0; 3],
                color: [255; 4],
            };
            for p in &e.properties {
                if let Some(count) = p.count {
                    let n = reader.count(count)?;
                    if n > bytes.len() {
                        return Err("PLY list exceeds file size".into());
                    }
                    let face = e.name == "face"
                        && matches!(p.name.as_str(), "vertex_indices" | "vertex_index");
                    let mut indices = Vec::new();
                    for _ in 0..n {
                        if face {
                            indices.push(reader.count(p.value)? as u32);
                        } else {
                            reader.read(p.value)?;
                        }
                    }
                    if face {
                        if n < 3 {
                            return Err("PLY face requires at least three vertices".into());
                        }
                        faces.push(indices);
                    }
                } else {
                    let value = reader.read(p.value)?;
                    if e.name != "vertex" {
                        continue;
                    }
                    match p.name.as_str() {
                        "x" | "y" | "z" | "nx" | "ny" | "nz" => {
                            let f = value as f32;
                            if !f.is_finite() {
                                return Err("PLY coordinate/normal exceeds f32 range".into());
                            }
                            let i = match p.name.as_str() {
                                "x" | "nx" => 0,
                                "y" | "ny" => 1,
                                _ => 2,
                            };
                            if p.name.starts_with('n') {
                                v.normal[i] = f;
                            } else {
                                v.position[i] = f;
                            }
                        }
                        "red" | "green" | "blue" | "alpha" => {
                            let value = if p.value.integer() {
                                value
                            } else {
                                value * 255.0
                            };
                            if !(0.0..=255.0).contains(&value) {
                                return Err("PLY color outside supported range".into());
                            }
                            let i = match p.name.as_str() {
                                "red" => 0,
                                "green" => 1,
                                "blue" => 2,
                                _ => 3,
                            };
                            v.color[i] = value.round() as u8;
                        }
                        _ => {}
                    }
                }
            }
            if e.name == "vertex" {
                out.vertices.push(v);
            }
        }
    }
    for face in faces {
        if face.iter().any(|&i| i as usize >= out.vertices.len()) {
            return Err("PLY face index out of range".into());
        }
        triangulate(&face, &out.vertices, &mut out.indices)?;
    }
    if out.vertices.is_empty() || out.indices.is_empty() {
        return Err("PLY contains no polygon mesh".into());
    }
    if normals == 0 {
        let mut sums = vec![glam::DVec3::ZERO; out.vertices.len()];
        for t in out.indices.chunks_exact(3) {
            let a = glam::Vec3::from(out.vertices[t[0] as usize].position).as_dvec3();
            let b = glam::Vec3::from(out.vertices[t[1] as usize].position).as_dvec3();
            let c = glam::Vec3::from(out.vertices[t[2] as usize].position).as_dvec3();
            let n = (b - a).cross(c - a);
            for &i in t {
                sums[i as usize] += n;
            }
        }
        for (v, n) in out.vertices.iter_mut().zip(sums) {
            v.normal = n.normalize_or_zero().as_vec3().to_array();
        }
    } else {
        for v in &mut out.vertices {
            v.normal = glam::Vec3::from(v.normal)
                .as_dvec3()
                .normalize_or_zero()
                .as_vec3()
                .to_array();
        }
    }
    out.validate()?;
    Ok(out)
}

/// Ear clipping on the dominant plane handles concave polygons without changing winding.
fn triangulate(face: &[u32], vertices: &[Vertex], out: &mut Vec<u32>) -> Result<(), String> {
    if face.len() == 3 {
        out.extend_from_slice(face);
        return Ok(());
    }
    let points: Vec<_> = face
        .iter()
        .map(|&i| glam::Vec3::from(vertices[i as usize].position).as_dvec3())
        .collect();
    let mut normal = glam::DVec3::ZERO;
    for i in 0..points.len() {
        normal += points[i].cross(points[(i + 1) % points.len()]);
    }
    let n = normal.abs();
    let plane = if n.x > n.y && n.x > n.z {
        0
    } else if n.y > n.z {
        1
    } else {
        2
    };
    let p: Vec<_> = points
        .iter()
        .map(|v| match plane {
            0 => glam::DVec2::new(v.y, v.z),
            1 => glam::DVec2::new(v.z, v.x),
            _ => glam::DVec2::new(v.x, v.y),
        })
        .collect();
    let area: f64 = (0..p.len())
        .map(|i| p[i].perp_dot(p[(i + 1) % p.len()]))
        .sum();
    if area == 0.0 {
        return Err("degenerate PLY polygon".into());
    }
    let sign = area.signum();
    let mut remaining: Vec<_> = (0..face.len()).collect();
    while remaining.len() > 3 {
        let mut ear = None;
        for j in 0..remaining.len() {
            let a = remaining[(j + remaining.len() - 1) % remaining.len()];
            let b = remaining[j];
            let c = remaining[(j + 1) % remaining.len()];
            let cross = |a: usize, b: usize, c: usize| (p[b] - p[a]).perp_dot(p[c] - p[a]) * sign;
            if cross(a, b, c) <= 0.0 {
                continue;
            }
            if remaining.iter().any(|&q| {
                q != a
                    && q != b
                    && q != c
                    && cross(a, b, q) >= 0.0
                    && cross(b, c, q) >= 0.0
                    && cross(c, a, q) >= 0.0
            }) {
                continue;
            }
            out.extend([face[a], face[b], face[c]]);
            ear = Some(j);
            break;
        }
        remaining
            .remove(ear.ok_or("cannot triangulate PLY polygon (degenerate or self-intersecting)")?);
    }
    out.extend(remaining.iter().map(|&i| face[i]));
    Ok(())
}

/// Binary little-endian PLY preserving source colors, normals and coordinates.
pub fn export_ply(source: &MeshSource) -> Vec<u8> {
    let mut bytes = format!("ply\nformat binary_little_endian 1.0\ncomment molar_vis coordinates in nanometers\nelement vertex {}\nproperty float x\nproperty float y\nproperty float z\nproperty float nx\nproperty float ny\nproperty float nz\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nproperty uchar alpha\nelement face {}\nproperty list uchar uint vertex_indices\nend_header\n", source.vertices.len(), source.indices.len() / 3).into_bytes();
    for v in &source.vertices {
        for f in v.position.iter().chain(v.normal.iter()) {
            bytes.extend(f.to_le_bytes());
        }
        bytes.extend(v.color);
    }
    for t in source.indices.chunks_exact(3) {
        bytes.push(3);
        for i in t {
            bytes.extend(i.to_le_bytes());
        }
    }
    bytes
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    const TRIANGLE: &str = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n-1 -1 0 255 0 0\n1 -1 0 0 255 0\n0 1 0 0 0 255\n3 0 1 2\n";
    pub(crate) fn triangle() -> Mesh {
        Mesh::new(
            "triangle.ply".into(),
            import_ply(TRIANGLE.as_bytes()).unwrap(),
        )
    }
    pub(crate) fn scene() -> crate::scene::Scene {
        let raw = crate::data::load(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/export_4677470_10902_4.sdf"
        )))
        .unwrap();
        let mut scene = crate::scene::Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        scene.molecules[0].reps.clear();
        scene
    }
    pub(crate) fn fixture_mesh() -> Mesh {
        let source = import_ply(include_bytes!(
            "../../../tests/export_4677470_10902_4_surface_mesh.ply"
        ))
        .unwrap();
        Mesh::new("COSMO surface.ply".into(), source)
    }

    #[test]
    fn supplied_cosmo_mesh_supports_all_styles_and_reversible_smoothing() {
        let mut mesh = fixture_mesh();
        assert_eq!(mesh.state.source.vertices.len(), 2285);
        assert_eq!(mesh.state.source.indices.len(), 4566 * 3);
        let original = export_ply(&mesh.state.source);
        mesh.state.style = MeshStyle::Points;
        mesh.state.point_size = 7.0;
        let points = mesh.draw_geometry();
        assert_eq!(points.points.len(), 2285);
        assert!(points.lines.is_empty() && points.mesh.indices.is_empty());
        assert!(points.points.iter().all(|p| p.size == 7.0));
        mesh.state.style = MeshStyle::Wireframe;
        mesh.state.line_width = 4.0;
        let wire = mesh.draw_geometry();
        let actual: std::collections::HashSet<_> = mesh
            .state
            .source
            .indices
            .chunks_exact(3)
            .flat_map(|t| {
                [
                    (t[0].min(t[1]), t[0].max(t[1])),
                    (t[1].min(t[2]), t[1].max(t[2])),
                    (t[2].min(t[0]), t[2].max(t[0])),
                ]
            })
            .collect();
        assert_eq!(wire.lines.len(), actual.len() * 2);
        assert!(wire.lines.iter().all(|l| l.width == 4.0));
        assert!(wire.points.is_empty() && wire.mesh.indices.is_empty());
        assert!(wire.lines.chunks_exact(2).all(|l| l[0].pos != l[1].pos));
        mesh.state.style = MeshStyle::Surface;
        mesh.state.smoothing = 5;
        let smoothed = mesh.surface_source();
        assert!(smoothed
            .vertices
            .iter()
            .zip(&mesh.state.source.vertices)
            .any(|(a, b)| a.position != b.position));
        assert!(smoothed
            .vertices
            .iter()
            .zip(&mesh.state.source.vertices)
            .all(|(a, b)| a.color == b.color));
        assert_eq!(smoothed.indices, mesh.state.source.indices);
        assert!(smoothed
            .vertices
            .iter()
            .all(|v| (glam::Vec3::from(v.normal).length() - 1.0).abs() < 1e-5));
        assert!(Arc::ptr_eq(&smoothed, &mesh.surface_source()));
        mesh.state.smoothing = 0;
        assert!(Arc::ptr_eq(&mesh.state.source, &mesh.surface_source()));
        assert_eq!(original, export_ply(&mesh.state.source));
        let scene = scene();
        assert_eq!(scene.molecules[0].n_atoms, 72);
        // SDF coordinates are converted from Å into the same nm frame as the PLY.
        let first = scene.molecules[0].data.state().coords[0];
        assert!((first.x + 0.49179).abs() < 1e-5);
    }

    #[test]
    fn mesh_style_options_persist_and_old_sessions_default_to_surface() {
        use crate::{
            history::{EditState, History},
            session::{Session, ViewState},
        };
        let mut scene = scene();
        scene.molecules[0].meshes.push(fixture_mesh());
        let mut history = History::new(EditState::capture(&scene));
        let mesh = &mut scene.molecules[0].meshes[0];
        mesh.state.style = MeshStyle::Wireframe;
        mesh.state.line_width = 6.0;
        mesh.state.point_size = 8.0;
        mesh.state.smoothing = 4;
        history.maybe_record(EditState::capture(&scene));
        let json = Session::capture(&scene, ViewState::default())
            .to_json()
            .unwrap();
        let saved = Session::from_json(&json).unwrap();
        let state = &saved.molecules[0].meshes[0];
        assert_eq!(state.style, MeshStyle::Wireframe);
        assert_eq!(state.line_width, 6.0);
        assert_eq!(state.point_size, 8.0);
        assert_eq!(state.smoothing, 4);
        history.undo(&mut scene).unwrap();
        assert_eq!(scene.molecules[0].meshes[0].state.style, MeshStyle::Surface);
        assert_eq!(scene.molecules[0].meshes[0].state.smoothing, 0);
        history.redo(&mut scene).unwrap();
        assert!(scene.molecules[0].meshes[0].state == *state);
        let mut old = serde_json::to_value(state).unwrap();
        for key in ["style", "point_size", "line_width", "smoothing"] {
            old.as_object_mut().unwrap().remove(key);
        }
        let old: MeshState = serde_json::from_value(old).unwrap();
        assert_eq!(old.style, MeshStyle::Surface);
        assert_eq!(old.point_size, 3.0);
        assert_eq!(old.line_width, 1.0);
        assert_eq!(old.smoothing, 0);
    }

    #[test]
    fn stable_mesh_ids_do_not_follow_vector_indices() {
        let mut scene = scene();
        let first = fixture_mesh();
        let second = fixture_mesh();
        assert_ne!(first.id, second.id);
        let second_id = second.id;
        scene.molecules[0].meshes.extend([first, second]);
        scene.molecules[0].meshes.remove(0);
        assert_eq!(scene.molecules[0].meshes[0].id, second_id);
        assert_eq!(scene.molecules[0].meshes[0].state.name, "COSMO surface.ply");
    }

    #[test]
    fn ascii_colors_generated_normals_and_binary_round_trip() {
        let source = import_ply(TRIANGLE.as_bytes()).unwrap();
        assert_eq!(source.indices, [0, 1, 2]);
        assert_eq!(source.vertices[0].color, [255, 0, 0, 255]);
        assert!(source.vertices.iter().all(|v| v.normal == [0.0, 0.0, 1.0]));
        let back = import_ply(&export_ply(&source)).unwrap();
        assert!(source == back);
    }
    #[test]
    fn big_endian_normals_alpha_and_unknown_properties() {
        let mut bytes = b"ply\r\nformat binary_big_endian 1.0\r\nelement vertex 3\r\nproperty uchar confidence\r\nproperty float z\r\nproperty float y\r\nproperty float x\r\nproperty float nz\r\nproperty float nx\r\nproperty float ny\r\nproperty float blue\r\nproperty float red\r\nproperty float green\r\nproperty uchar alpha\r\nelement face 1\r\nproperty uchar label\r\nproperty list uchar int vertex_index\r\nelement edge 1\r\nproperty list uchar float weights\r\nend_header\r\n".to_vec();
        for p in [[-1.0_f32, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]] {
            bytes.push(10);
            for v in [p[2], p[1], p[0], 2.0, 0.0, 0.0, 0.0, 1.0, 0.5] {
                bytes.extend(v.to_be_bytes());
            }
            bytes.push(128);
        }
        bytes.extend([7, 3]);
        for i in [0_i32, 1, 2] {
            bytes.extend(i.to_be_bytes());
        }
        bytes.push(2);
        bytes.extend(0.2_f32.to_be_bytes());
        bytes.extend(0.4_f32.to_be_bytes());
        let source = import_ply(&bytes).unwrap();
        assert_eq!(source.vertices[0].position, [-1.0, -1.0, 0.0]);
        assert_eq!(source.vertices[0].normal, [0.0, 0.0, 1.0]);
        assert_eq!(source.vertices[0].color, [255, 128, 0, 128]);
        let mesh = Mesh::new("big.ply".into(), source);
        assert!(mesh.transparent());
    }
    #[test]
    fn concave_polygons_preserve_area_and_winding() {
        let vertices: Vec<_> = [
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 2.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 2.0, 0.0],
        ]
        .into_iter()
        .map(|position| Vertex {
            position,
            normal: [0.0, 0.0, 1.0],
            color: [255; 4],
        })
        .collect();
        for face in [vec![0, 1, 2, 3, 4], vec![4, 3, 2, 1, 0]] {
            let mut indices = Vec::new();
            triangulate(&face, &vertices, &mut indices).unwrap();
            assert_eq!(indices.len(), 9);
            let areas: Vec<_> = indices
                .chunks_exact(3)
                .map(|t| {
                    let p = |i: u32| glam::Vec3::from(vertices[i as usize].position);
                    (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).z * 0.5
                })
                .collect();
            assert!(areas.iter().all(|a| a.signum() == areas[0].signum()));
            assert!((areas.iter().sum::<f32>().abs() - 3.0).abs() < 1e-6);
        }
    }
    #[test]
    fn invalid_and_truncated_files_are_rejected() {
        for data in [
            TRIANGLE.replace("3 0 1 2", "3 0 1 9"),
            TRIANGLE.replace("3 0 1 2", "3 0 -1 2"),
            TRIANGLE.replace("-1 -1 0", "NaN -1 0"),
            TRIANGLE.replace("element vertex 3", "element vertex 999999999"),
            TRIANGLE.replace("property float z\n", ""),
            TRIANGLE.replace("255 0 0", "256 0 0"),
            TRIANGLE.replace("property float x", "property unsupported x"),
        ] {
            assert!(import_ply(data.as_bytes()).is_err());
        }
        let binary = export_ply(&triangle().state.source);
        for end in [0, 5, binary.len() - 1, binary.len() - 10] {
            assert!(import_ply(&binary[..end]).is_err());
        }
    }
    #[test]
    fn material_changes_leave_saved_geometry_unchanged() {
        let mut mesh = triangle();
        let before = export_ply(&mesh.state.source);
        mesh.state.material = Material::Transparent;
        let geom = mesh.geometry();
        assert_eq!(geom.vertices[0].color >> 24, 77);
        assert_eq!(geom.vertices[0].mat, Material::Transparent.pack_lighting());
        assert_eq!(before, export_ply(&mesh.state.source));
    }
    #[test]
    fn meshes_persist_in_sessions_and_undo_redo() {
        use crate::{
            history::{EditState, History},
            session::{Session, ViewState},
        };
        let mut scene = scene();
        let mut history = History::new(EditState::capture(&scene));
        scene.molecules[0].meshes.push(triangle());
        history.maybe_record(EditState::capture(&scene));
        let geometry = scene.molecules[0].meshes[0].state.source.clone();
        scene.molecules[0].meshes[0].state.visible = false;
        scene.molecules[0].meshes[0].state.material = Material::Glossy;
        history.maybe_record(EditState::capture(&scene));
        let json = Session::capture(&scene, ViewState::default())
            .to_json()
            .unwrap();
        let saved = Session::from_json(&json).unwrap();
        let state = &saved.molecules[0].meshes[0];
        assert!(!state.visible);
        assert_eq!(state.material, Material::Glossy);
        assert!(*state.source == *geometry);
        history.undo(&mut scene).unwrap();
        assert!(scene.molecules[0].meshes[0].state.visible);
        assert_eq!(
            scene.molecules[0].meshes[0].state.material,
            Material::Opaque
        );
        history.undo(&mut scene).unwrap();
        assert!(scene.molecules[0].meshes.is_empty());
        history.redo(&mut scene).unwrap();
        history.redo(&mut scene).unwrap();
        assert!(!scene.molecules[0].meshes[0].state.visible);
        scene.molecules[0].meshes.clear();
        history.maybe_record(EditState::capture(&scene));
        history.undo(&mut scene).unwrap();
        assert!(Arc::ptr_eq(
            &scene.molecules[0].meshes[0].state.source,
            &geometry
        ));
    }
}
