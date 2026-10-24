//! Mesh input for argent.
//!
//! Jewelry arrives as ZBrush/Rhino/CrossGems/RingDesigner exports: STL
//! (binary or ASCII, no shared vertices, no normals), OBJ, PLY or 3MF, in
//! millimetres, often millions of triangles, in whatever orientation the
//! sculptor left it. This crate turns any of those into a welded,
//! smooth-shaded [`Mesh`] in a canonical pose, and computes the per-vertex
//! masks the renderer uses to oxidise recesses.
//!
//! CONTRACT (other crates compile against these signatures; keep them):
//! - [`load`] / [`load_many`] dispatch on extension *and* magic bytes.
//! - Positions are millimetres. `normals.len() == positions.len()`.
//! - `parts` partition `triangles` (ranges are contiguous, in order).
//! - [`orient::detect`] returns a pose; [`Mesh::apply_pose`] applies it.
//! - [`cavity::patina_mask`] returns one value in `0..=1` per vertex.

pub mod cavity;
pub mod io;
pub mod normals;
pub mod orient;
pub mod weld;

use std::ops::Range;
use std::path::{Path, PathBuf};

/// Triangle mesh with per-vertex shading normals.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub parts: Vec<Part>,
}

/// A named run of triangles: an OBJ group/object, a 3MF object, or a whole
/// file in [`load_many`]. `material` carries an OBJ `usemtl` name or a 3MF
/// material name when present; the renderer maps names like `"sapphire"`,
/// `"stone"`, `"gem"` or `"metal"` onto materials.
#[derive(Clone, Debug)]
pub struct Part {
    pub name: String,
    pub triangles: Range<u32>,
    pub material: Option<String>,
}

#[derive(Clone, Debug)]
pub struct LoadOptions {
    /// Merge coincident vertices (STL has none shared).
    pub weld: bool,
    /// Weld tolerance as a fraction of the bounding-box diagonal.
    pub weld_tolerance: f32,
    /// Edges whose dihedral angle exceeds this stay sharp (vertices split).
    pub crease_angle_deg: f32,
    /// Multiply positions on load (1.0 = file is in mm; 25.4 for inches).
    pub unit_scale: f32,
}

impl Default for LoadOptions {
    fn default() -> Self {
        LoadOptions { weld: true, weld_tolerance: 1e-6, crease_angle_deg: 50.0, unit_scale: 1.0 }
    }
}

/// Load one file (STL ascii/binary, OBJ, PLY ascii/binary, 3MF), weld it and
/// compute crease-aware smooth normals.
pub fn load(path: &Path, opts: &LoadOptions) -> anyhow::Result<Mesh> {
    io::load(path, opts)
}

/// Load several files into one mesh, one [`Part`] per file (named by stem),
/// e.g. `ring.stl` + `stone.stl`.
pub fn load_many(paths: &[PathBuf], opts: &LoadOptions) -> anyhow::Result<Mesh> {
    let mut out = Mesh::default();
    for p in paths {
        let mut m = load(p, opts)?;
        if m.parts.len() <= 1 {
            let n = m.triangles.len() as u32;
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            m.parts = vec![Part { name: stem, triangles: 0..n, material: None }];
        }
        out.merge(m);
    }
    Ok(out)
}

/// A rigid pose: translate by `-center`, then rotate (row-major 3×3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub center: [f32; 3],
    pub rotation: [[f32; 3]; 3],
}

impl Pose {
    pub const IDENTITY: Pose = Pose { center: [0.0; 3], rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] };

    #[inline]
    pub fn apply_point(&self, p: [f32; 3]) -> [f32; 3] {
        let q = [p[0] - self.center[0], p[1] - self.center[1], p[2] - self.center[2]];
        self.apply_vector(q)
    }

    #[inline]
    pub fn apply_vector(&self, v: [f32; 3]) -> [f32; 3] {
        let r = &self.rotation;
        [
            r[0][0] * v[0] + r[0][1] * v[1] + r[0][2] * v[2],
            r[1][0] * v[0] + r[1][1] * v[1] + r[1][2] * v[2],
            r[2][0] * v[0] + r[2][1] * v[1] + r[2][2] * v[2],
        ]
    }
}

impl Mesh {
    /// Axis-aligned bounds `(min, max)`; zeros for an empty mesh.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        if self.positions.is_empty() {
            return ([0.0; 3], [0.0; 3]);
        }
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in &self.positions {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        (lo, hi)
    }

    /// Append `other`, offsetting its indices and part ranges.
    pub fn merge(&mut self, other: Mesh) {
        let vo = self.positions.len() as u32;
        let to = self.triangles.len() as u32;
        self.positions.extend(other.positions);
        self.normals.extend(other.normals);
        self.triangles.extend(other.triangles.into_iter().map(|t| [t[0] + vo, t[1] + vo, t[2] + vo]));
        if other.parts.is_empty() {
            let end = self.triangles.len() as u32;
            self.parts.push(Part { name: format!("part{}", self.parts.len()), triangles: to..end, material: None });
        } else {
            self.parts.extend(other.parts.into_iter().map(|mut p| {
                p.triangles = p.triangles.start + to..p.triangles.end + to;
                p
            }));
        }
    }

    /// Rigidly move the mesh (positions and normals).
    pub fn apply_pose(&mut self, pose: &Pose) {
        for p in &mut self.positions {
            *p = pose.apply_point(*p);
        }
        for n in &mut self.normals {
            *n = pose.apply_vector(*n);
        }
    }

    /// Uniform scale about the origin.
    pub fn scale(&mut self, s: f32) {
        for p in &mut self.positions {
            for v in p.iter_mut() {
                *v *= s;
            }
        }
    }

    /// Total surface area, mm².
    pub fn area(&self) -> f64 {
        self.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| self.positions[i as usize]);
                let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let cx = e1[1] * e2[2] - e1[2] * e2[1];
                let cy = e1[2] * e2[0] - e1[0] * e2[2];
                let cz = e1[0] * e2[1] - e1[1] * e2[0];
                0.5 * ((cx * cx + cy * cy + cz * cz) as f64).sqrt()
            })
            .sum()
    }

    /// Enclosed volume, mm³ (signed-tetrahedra sum; meaningful when closed).
    pub fn volume(&self) -> f64 {
        self.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| self.positions[i as usize].map(|v| v as f64));
                (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                    + a[2] * (b[0] * c[1] - b[1] * c[0]))
                    / 6.0
            })
            .sum::<f64>()
            .abs()
    }
}
