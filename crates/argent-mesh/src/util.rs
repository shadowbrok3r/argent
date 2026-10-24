//! Crate-private helpers: small vector math, union-find, vertex→face
//! adjacency, triangle compaction, a 3×3 symmetric eigen solver and hashing.

use crate::{Mesh, Part};
use rayon::prelude::*;

pub(crate) type V3 = [f32; 3];
pub(crate) type D3 = [f64; 3];

#[inline]
pub(crate) fn d3(a: V3) -> D3 {
    [a[0] as f64, a[1] as f64, a[2] as f64]
}
#[inline]
pub(crate) fn sub_d(a: D3, b: D3) -> D3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
pub(crate) fn cross_d(a: D3, b: D3) -> D3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
#[inline]
pub(crate) fn dot_d(a: D3, b: D3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
pub(crate) fn len_d(a: D3) -> f64 {
    dot_d(a, a).sqrt()
}
#[cfg(test)]
#[inline]
pub(crate) fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Unit normal and area of a triangle (computed in f64). Zero normal for
/// degenerate triangles.
#[inline]
pub(crate) fn tri_normal_area(p: &[V3], t: [u32; 3]) -> (V3, f32) {
    let a = d3(p[t[0] as usize]);
    let b = d3(p[t[1] as usize]);
    let c = d3(p[t[2] as usize]);
    let n = cross_d(sub_d(b, a), sub_d(c, a));
    let l = len_d(n);
    if l > 0.0 && l.is_finite() {
        ([(n[0] / l) as f32, (n[1] / l) as f32, (n[2] / l) as f32], (0.5 * l) as f32)
    } else {
        ([0.0; 3], 0.0)
    }
}

/// Per-face unit normals and areas, in parallel.
pub(crate) fn face_normals(p: &[V3], tris: &[[u32; 3]]) -> (Vec<V3>, Vec<f32>) {
    tris.par_iter().with_min_len(4096).map(|&t| tri_normal_area(p, t)).unzip()
}

/// Disjoint-set forest; the root of a set is always its smallest member.
pub(crate) struct UnionFind {
    parent: Vec<u32>,
}

impl UnionFind {
    pub fn new(n: usize) -> Self {
        UnionFind { parent: (0..n as u32).collect() }
    }
    #[inline]
    pub fn find(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let p = self.parent[x as usize];
            let gp = self.parent[p as usize];
            self.parent[x as usize] = gp;
            x = gp;
        }
        x
    }
    #[inline]
    pub fn union(&mut self, a: u32, b: u32) -> bool {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return false;
        }
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        self.parent[hi as usize] = lo;
        true
    }
}

/// Vertex → incident faces (CSR). Each vertex's list is in increasing face
/// order, which lets callers binary-search a face id.
pub(crate) struct VertexFaces {
    pub offsets: Vec<u32>,
    pub faces: Vec<u32>,
}

impl VertexFaces {
    pub fn build(nv: usize, tris: &[[u32; 3]]) -> Self {
        assert!(tris.len() * 3 < u32::MAX as usize, "argent-mesh: too many triangles for u32 adjacency");
        let mut counts = vec![0u32; nv + 1];
        for t in tris {
            for &v in t {
                counts[v as usize + 1] += 1;
            }
        }
        for i in 0..nv {
            counts[i + 1] += counts[i];
        }
        let offsets = counts;
        let mut cursor: Vec<u32> = offsets[..nv].to_vec();
        let mut faces = vec![0u32; tris.len() * 3];
        for (f, t) in tris.iter().enumerate() {
            for &v in t {
                let c = &mut cursor[v as usize];
                faces[*c as usize] = f as u32;
                *c += 1;
            }
        }
        VertexFaces { offsets, faces }
    }
    #[inline]
    pub fn of(&self, v: usize) -> &[u32] {
        &self.faces[self.offsets[v] as usize..self.offsets[v + 1] as usize]
    }
    #[inline]
    pub fn range(&self, v: usize) -> std::ops::Range<usize> {
        self.offsets[v] as usize..self.offsets[v + 1] as usize
    }
}

/// Keep the triangles whose flag is set, updating part ranges (empty parts
/// are dropped). Returns how many triangles were removed.
pub(crate) fn retain_triangles(mesh: &mut Mesh, keep: &[bool]) -> usize {
    debug_assert_eq!(keep.len(), mesh.triangles.len());
    let removed = keep.iter().filter(|k| !**k).count();
    if removed == 0 {
        return 0;
    }
    // Prefix count of kept triangles maps old range bounds to new ones.
    let mut new_index = Vec::with_capacity(keep.len() + 1);
    let mut acc = 0u32;
    new_index.push(0u32);
    for &k in keep {
        acc += k as u32;
        new_index.push(acc);
    }
    let mut out = Vec::with_capacity(mesh.triangles.len() - removed);
    for (t, &k) in mesh.triangles.iter().zip(keep) {
        if k {
            out.push(*t);
        }
    }
    mesh.triangles = out;
    let parts: Vec<Part> = std::mem::take(&mut mesh.parts)
        .into_iter()
        .filter_map(|mut p| {
            let s = new_index[p.triangles.start as usize];
            let e = new_index[p.triangles.end as usize];
            p.triangles = s..e;
            (e > s).then_some(p)
        })
        .collect();
    mesh.parts = parts;
    removed
}

/// Make `parts` a valid partition: if absent or not covering every triangle,
/// fill the gaps with generic parts.
pub(crate) fn normalize_parts(mesh: &mut Mesh, default_name: &str) {
    let n = mesh.triangles.len() as u32;
    let mut parts: Vec<Part> = std::mem::take(&mut mesh.parts);
    parts.retain(|p| p.triangles.end > p.triangles.start && p.triangles.end <= n);
    parts.sort_by_key(|p| p.triangles.start);
    let mut out: Vec<Part> = Vec::with_capacity(parts.len() + 1);
    let mut at = 0u32;
    for mut p in parts {
        if p.triangles.start < at {
            if p.triangles.end <= at {
                continue;
            }
            p.triangles.start = at;
        }
        if p.triangles.start > at {
            out.push(Part { name: default_name.to_string(), triangles: at..p.triangles.start, material: None });
        }
        at = p.triangles.end;
        out.push(p);
    }
    if at < n {
        out.push(Part { name: default_name.to_string(), triangles: at..n, material: None });
    }
    mesh.parts = out;
}

/// Eigen-decomposition of a symmetric 3×3 matrix (cyclic Jacobi). Returns
/// eigenvalues in descending order with matching unit eigenvectors.
pub(crate) fn sym_eigen3(m: [[f64; 3]; 3]) -> ([f64; 3], [D3; 3]) {
    let mut a = m;
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _sweep in 0..64 {
        let off = a[0][1].abs() + a[0][2].abs() + a[1][2].abs();
        let scale = a[0][0].abs() + a[1][1].abs() + a[2][2].abs();
        if off <= 1e-15 * scale.max(1e-300) {
            break;
        }
        for (p, q) in [(0usize, 1usize), (0, 2), (1, 2)] {
            if a[p][q].abs() < 1e-300 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
            let t = if theta == 0.0 { 1.0 } else { t };
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            // A' = Jᵀ A J with the rotation in the (p, q) plane.
            for k in 0..3 {
                let akp = a[k][p];
                let akq = a[k][q];
                a[k][p] = c * akp - s * akq;
                a[k][q] = s * akp + c * akq;
            }
            for k in 0..3 {
                let apk = a[p][k];
                let aqk = a[q][k];
                a[p][k] = c * apk - s * aqk;
                a[q][k] = s * apk + c * aqk;
            }
            for row in v.iter_mut() {
                let vkp = row[p];
                let vkq = row[q];
                row[p] = c * vkp - s * vkq;
                row[q] = s * vkp + c * vkq;
            }
        }
    }
    let mut idx = [0usize, 1, 2];
    idx.sort_by(|&i, &j| a[j][j].partial_cmp(&a[i][i]).unwrap_or(std::cmp::Ordering::Equal));
    let vals = [a[idx[0]][idx[0]], a[idx[1]][idx[1]], a[idx[2]][idx[2]]];
    let col = |c: usize| [v[0][c], v[1][c], v[2][c]];
    (vals, [col(idx[0]), col(idx[1]), col(idx[2])])
}

/// 32-bit integer hash (lowbias32).
#[inline]
pub(crate) fn hash32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// Uniform float in [0, 1) from a hash.
#[inline]
pub(crate) fn unit_f32(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// Parse an ASCII float token (bytes) via the standard library.
#[inline]
pub(crate) fn parse_f32(tok: &[u8]) -> Option<f32> {
    std::str::from_utf8(tok).ok()?.parse::<f32>().ok()
}

#[inline]
pub(crate) fn parse_f64(tok: &[u8]) -> Option<f64> {
    std::str::from_utf8(tok).ok()?.parse::<f64>().ok()
}

#[inline]
pub(crate) fn parse_i64(tok: &[u8]) -> Option<i64> {
    std::str::from_utf8(tok).ok()?.parse::<i64>().ok()
}

/// Whitespace tokenizer over a byte slice (space, tab, CR, LF, FF, VT).
pub(crate) struct Tokens<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Tokens<'a> {
    pub fn new(s: &'a [u8]) -> Self {
        Tokens { s, i: 0 }
    }
    /// Remaining bytes up to the end of the current line (not including the newline).
    pub fn rest_of_line(&mut self) -> &'a [u8] {
        let s = self.s;
        while self.i < s.len() && (s[self.i] == b' ' || s[self.i] == b'\t') {
            self.i += 1;
        }
        let start = self.i;
        while self.i < s.len() && s[self.i] != b'\n' {
            self.i += 1;
        }
        let mut end = self.i;
        while end > start && (s[end - 1] == b'\r' || s[end - 1] == b' ' || s[end - 1] == b'\t') {
            end -= 1;
        }
        &s[start..end]
    }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = &'a [u8];
    #[inline]
    fn next(&mut self) -> Option<&'a [u8]> {
        let s = self.s;
        let mut i = self.i;
        while i < s.len() && s[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= s.len() {
            self.i = i;
            return None;
        }
        let start = i;
        while i < s.len() && !s[i].is_ascii_whitespace() {
            i += 1;
        }
        self.i = i;
        Some(&s[start..i])
    }
}

/// Find `needle` in `hay` starting at `from`.
pub(crate) fn find_bytes(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= hay.len() || hay.len() - from < needle.len() {
        return None;
    }
    let first = needle[0];
    let last_start = hay.len() - needle.len();
    let mut i = from;
    while i <= last_start {
        let j = i + hay[i..=last_start].iter().position(|&b| b == first)?;
        if &hay[j..j + needle.len()] == needle {
            return Some(j);
        }
        i = j + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eigen_diag_and_rotated() {
        let (vals, vecs) = sym_eigen3([[1.0, 0.0, 0.0], [0.0, 5.0, 0.0], [0.0, 0.0, 3.0]]);
        assert!((vals[0] - 5.0).abs() < 1e-12 && (vals[1] - 3.0).abs() < 1e-12 && (vals[2] - 1.0).abs() < 1e-12);
        assert!(vecs[0][1].abs() > 0.999999);
        // Rotated: M = R diag(4,2,1) Rᵀ
        let c = (0.3f64).cos();
        let s = (0.3f64).sin();
        let r = [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]];
        let d = [4.0, 2.0, 1.0];
        let mut m = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                for k in 0..3 {
                    m[i][j] += r[i][k] * d[k] * r[j][k];
                }
            }
        }
        let (vals, vecs) = sym_eigen3(m);
        assert!((vals[0] - 4.0).abs() < 1e-9);
        assert!((dot_d(vecs[0], [c, s, 0.0]).abs() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn find_and_tokens() {
        assert_eq!(find_bytes(b"abc endfacet x", b"endfacet", 0), Some(4));
        assert_eq!(find_bytes(b"abc", b"abcd", 0), None);
        let t: Vec<&[u8]> = Tokens::new(b"  v 1 2\t3\r\n").collect();
        assert_eq!(t, vec![&b"v"[..], b"1", b"2", b"3"]);
    }
}
