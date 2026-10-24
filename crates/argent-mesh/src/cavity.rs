//! Per-vertex cavity masks for antiquing (oxidised recesses, polished highs).
//!
//! [`patina_mask`] = contrast-shaped blend of
//! - ambient occlusion: fraction of cosine-weighted hemisphere rays (from
//!   the vertex, nudged along its normal) blocked within `ao_distance` —
//!   "how far the polishing cloth can't reach";
//! - concavity: mean curvature × `curvature_radius` from a Gaussian-weighted
//!   neighbourhood of radius `curvature_radius` (positive in recesses);
//!
//! then Laplacian-smoothed over the mesh with crease-split copies of a vertex
//! treated as one, so hard edges never show a seam in the patina.

use crate::util::V3;
use crate::Mesh;
use glam::{DVec3, Vec3};
use rayon::prelude::*;

/// Occlusion query: `(origin, direction, max_distance) -> hit?`.
pub trait Occluder: Sync {
    fn occluded(&self, origin: [f32; 3], dir: [f32; 3], max_dist: f32) -> bool;
}

/// Settings for [`patina_mask`]; distances in mm.
#[derive(Clone, Debug)]
pub struct PatinaSettings {
    /// AO rays per vertex.
    pub ao_samples: u32,
    /// How far a recess "sees" (≈ the polishing cloth's reach), mm.
    pub ao_distance: f32,
    /// Curvature analysis radius, mm.
    pub curvature_radius: f32,
    /// Blend between AO (0) and concavity (1).
    pub concavity_weight: f32,
    /// Laplacian smoothing passes over the final mask.
    pub smooth_iterations: u32,
}

impl Default for PatinaSettings {
    fn default() -> Self {
        PatinaSettings { ao_samples: 48, ao_distance: 1.2, curvature_radius: 0.4, concavity_weight: 0.35, smooth_iterations: 3 }
    }
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One value per vertex in `0..=1`: 1 = deep recess (oxidised black),
/// 0 = exposed high point (polished bright).
pub fn patina_mask(mesh: &crate::Mesh, occ: &dyn Occluder, s: &PatinaSettings) -> Vec<f32> {
    let nv = mesh.positions.len();
    if nv == 0 || mesh.triangles.is_empty() {
        return vec![0.0; nv];
    }
    let ao = if s.ao_samples > 0 && s.ao_distance > 0.0 {
        ambient_occlusion(mesh, occ, s.ao_samples, s.ao_distance)
    } else {
        vec![0.0; nv]
    };
    let w = s.concavity_weight.clamp(0.0, 1.0);
    let cav = if w > 0.0 && s.curvature_radius > 0.0 { concavity(mesh, s.curvature_radius) } else { vec![0.0; nv] };
    let mut m: Vec<f32> = ao
        .par_iter()
        .zip(cav.par_iter())
        .map(|(&a, &c)| {
            let at = smoothstep(0.04, 0.55, a);
            let ct = smoothstep(0.02, 0.5, c);
            smoothstep(0.05, 0.9, (1.0 - w) * at + w * ct)
        })
        .collect();
    smooth(mesh, &mut m, s.smooth_iterations, 0.5);
    for v in &mut m {
        *v = v.clamp(0.0, 1.0);
    }
    m
}

fn vertex_normals(mesh: &Mesh) -> Vec<V3> {
    if mesh.normals.len() == mesh.positions.len() {
        mesh.normals.clone()
    } else {
        crate::normals::smooth_vertex_normals(mesh)
    }
}

fn diag(mesh: &Mesh) -> f32 {
    let (lo, hi) = mesh.bounds();
    ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt()
}

/// Fraction (0..1) of `samples` cosine-weighted hemisphere rays per vertex
/// that hit something within `distance` mm. Stratified per vertex and
/// rotated by a vertex hash (deterministic, low noise).
pub fn ambient_occlusion(mesh: &Mesh, occ: &dyn Occluder, samples: u32, distance: f32) -> Vec<f32> {
    let normals = vertex_normals(mesh);
    let eps = (2e-4 * diag(mesh)).max(1e-3);
    let n = samples.max(1);
    mesh.positions
        .par_iter()
        .zip(normals.par_iter())
        .enumerate()
        .with_min_len(256)
        .map(|(v, (p, nrm))| {
            let nn = Vec3::from(*nrm).normalize_or_zero();
            if nn == Vec3::ZERO {
                return 0.0;
            }
            let (t, b) = nn.any_orthonormal_pair();
            let o = Vec3::from(*p) + nn * eps;
            let h = crate::util::hash32(v as u32 ^ 0x5bd1_e995);
            let rot = crate::util::unit_f32(h);
            let jit = crate::util::unit_f32(crate::util::hash32(h));
            let mut hits = 0u32;
            for k in 0..n {
                let u1 = ((k as f32 + jit) / n as f32).min(0.999_999);
                let u2 = (k as f32 * 0.618_034 + rot).fract();
                let r = u1.sqrt();
                let phi = std::f32::consts::TAU * u2;
                let d = t * (r * phi.cos()) + b * (r * phi.sin()) + nn * (1.0 - u1).sqrt();
                if occ.occluded(o.to_array(), d.to_array(), distance) {
                    hits += 1;
                }
            }
            hits as f32 / n as f32
        })
        .collect()
}

/// Signed concavity per vertex: mean curvature × `radius` measured over a
/// Gaussian neighbourhood (σ = radius/2, cut at `radius`). Positive in
/// recesses (the surrounding surface rises along the normal), negative on
/// ridges; ≈ `radius / R` at the bottom of a spherical dimple of radius R.
/// Points on the far side of thin walls (normals facing away) are ignored.
pub fn concavity(mesh: &Mesh, radius: f32) -> Vec<f32> {
    let nv = mesh.positions.len();
    if nv == 0 || radius.is_nan() || radius <= 0.0 {
        return vec![0.0; nv];
    }
    let normals = vertex_normals(mesh);
    // Vertex areas (one third of incident triangle areas).
    let mut varea = vec![0.0f64; nv];
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|i| Vec3::from(mesh.positions[i as usize]).as_dvec3());
        let ar = 0.5 * (b - a).cross(c - a).length() / 3.0;
        for &i in t {
            varea[i as usize] += ar;
        }
    }
    let (lo, _) = mesh.bounds();
    let lo = Vec3::from(lo).as_dvec3();
    let h = radius as f64 / 2.0;
    let key_of = |p: [f32; 3]| -> u64 {
        let q = (Vec3::from(p).as_dvec3() - lo) / h;
        let c = [q.x.max(0.0) as u64, q.y.max(0.0) as u64, q.z.max(0.0) as u64];
        (c[0].min((1 << 21) - 1) << 42) | (c[1].min((1 << 21) - 1) << 21) | c[2].min((1 << 21) - 1)
    };
    let mut order: Vec<(u64, u32)> = mesh.positions.par_iter().enumerate().map(|(i, p)| (key_of(*p), i as u32)).collect();
    order.par_sort_unstable();
    // Cells: area-weighted centroid and normal.
    struct Cell {
        key: u64,
        m: f64,
        p: DVec3,
        n: DVec3,
    }
    let mut cells: Vec<Cell> = Vec::new();
    let mut cell_of = vec![0u32; nv];
    let mut i = 0;
    while i < order.len() {
        let k = order[i].0;
        let (mut m, mut p, mut n) = (0.0, DVec3::ZERO, DVec3::ZERO);
        let mut e = i;
        while e < order.len() && order[e].0 == k {
            let v = order[e].1 as usize;
            let w = varea[v].max(1e-12);
            m += w;
            p += Vec3::from(mesh.positions[v]).as_dvec3() * w;
            n += Vec3::from(normals[v]).as_dvec3() * w;
            cell_of[v] = cells.len() as u32;
            e += 1;
        }
        cells.push(Cell { key: k, m, p: p / m, n: n.normalize_or_zero() });
        i = e;
    }
    drop(order);
    let unpack = |k: u64| [(k >> 42) as i64, ((k >> 21) & ((1 << 21) - 1)) as i64, (k & ((1 << 21) - 1)) as i64];
    let pack = |c: [i64; 3]| ((c[0] as u64) << 42) | ((c[1] as u64) << 21) | c[2] as u64;
    let reach = (radius as f64 / h).ceil() as i64 + 1;
    let lim = (1i64 << 21) - 1;
    let max_d = radius as f64 + 2.0 * h;
    let neighbours: Vec<Vec<u32>> = cells
        .par_iter()
        .map(|c| {
            let base = unpack(c.key);
            let mut out = Vec::new();
            for dx in -reach..=reach {
                for dy in -reach..=reach {
                    for dz in -reach..=reach {
                        let q = [base[0] + dx, base[1] + dy, base[2] + dz];
                        if q.iter().any(|&x| x < 0 || x > lim) {
                            continue;
                        }
                        let key = pack(q);
                        if let Ok(j) = cells.binary_search_by_key(&key, |c| c.key)
                            && (cells[j].p - c.p).length() <= max_d
                        {
                            out.push(j as u32);
                        }
                    }
                }
            }
            out
        })
        .collect();
    let r2 = (radius as f64) * (radius as f64);
    let inv2s2 = 1.0 / (2.0 * (0.5 * radius as f64).powi(2));
    (0..nv)
        .into_par_iter()
        .with_min_len(1024)
        .map(|v| {
            let p = Vec3::from(mesh.positions[v]).as_dvec3();
            let n = Vec3::from(normals[v]).as_dvec3().normalize_or_zero();
            if n == DVec3::ZERO {
                return 0.0;
            }
            let (mut sw, mut sd, mut st) = (0.0f64, DVec3::ZERO, 0.0f64);
            for &j in &neighbours[cell_of[v] as usize] {
                let c = &cells[j as usize];
                let d = c.p - p;
                let dist2 = d.length_squared();
                if dist2 > r2 || c.n.dot(n) < -0.5 {
                    continue;
                }
                let w = c.m * (-dist2 * inv2s2).exp();
                sw += w;
                sd += d * w;
                let dn = d.dot(n);
                st += w * (dist2 - dn * dn);
            }
            if sw <= 0.0 || st <= 1e-18 {
                return 0.0;
            }
            let hgt = (sd / sw).dot(n);
            let dt2 = st / sw;
            let mean_curv = 2.0 * hgt / dt2;
            (mean_curv * radius as f64).clamp(-4.0, 4.0) as f32
        })
        .collect()
}

/// Laplacian smoothing of per-vertex `values` over mesh edges:
/// `x ← x + λ·(mean(neighbours) − x)`, `iterations` times. Vertices sharing
/// a position (crease splits) are smoothed as one and end up equal.
pub fn smooth(mesh: &Mesh, values: &mut [f32], iterations: u32, lambda: f32) {
    let nv = mesh.positions.len();
    assert_eq!(values.len(), nv, "smooth: one value per vertex");
    if nv == 0 {
        return;
    }
    let (_, canon) = crate::weld::weld_positions(&mesh.positions, 0.0);
    let nc = canon.iter().copied().filter(|&c| c != u32::MAX).max().map_or(0, |m| m as usize + 1);
    let mut sum = vec![0.0f64; nc];
    let mut cnt = vec![0u32; nc];
    for (v, &c) in canon.iter().enumerate() {
        sum[c as usize] += values[v] as f64;
        cnt[c as usize] += 1;
    }
    let mut x: Vec<f32> = sum.iter().zip(&cnt).map(|(s, &n)| (s / n.max(1) as f64) as f32).collect();
    if iterations > 0 && !mesh.triangles.is_empty() {
        let mut edges: Vec<u64> = mesh
            .triangles
            .par_iter()
            .flat_map_iter(|t| {
                let c = t.map(|i| canon[i as usize] as u64);
                [(c[0] << 32) | c[1], (c[1] << 32) | c[0], (c[1] << 32) | c[2], (c[2] << 32) | c[1], (c[2] << 32) | c[0], (c[0] << 32) | c[2]]
            })
            .collect();
        edges.par_sort_unstable();
        edges.dedup();
        let mut off = vec![0u32; nc + 1];
        for e in &edges {
            off[(e >> 32) as usize + 1] += 1;
        }
        for i in 0..nc {
            off[i + 1] += off[i];
        }
        let nbr: Vec<u32> = edges.iter().map(|e| *e as u32).collect();
        drop(edges);
        let mut y = x.clone();
        for _ in 0..iterations {
            y.par_iter_mut().enumerate().with_min_len(4096).for_each(|(i, out)| {
                let (a, b) = (off[i] as usize, off[i + 1] as usize);
                let xi = x[i];
                *out = if b > a {
                    let m: f32 = nbr[a..b].iter().filter(|&&j| j as usize != i).map(|&j| x[j as usize]).sum::<f32>()
                        / (b - a) as f32;
                    xi + lambda * (m - xi)
                } else {
                    xi
                };
            });
            std::mem::swap(&mut x, &mut y);
        }
    }
    for (v, &c) in canon.iter().enumerate() {
        values[v] = x[c as usize];
    }
}

// ---------------------------------------------------------------------------
// A small BVH occluder (median split, 4 triangles per leaf). Good for tests
// and tools up to a few million triangles; the renderer supplies its own.

/// Bounding-volume hierarchy over a mesh's triangles implementing [`Occluder`].
pub struct SimpleBvh {
    nodes: Vec<Node>,
    tris: Vec<[Vec3; 3]>,
}

#[derive(Clone, Copy)]
struct Node {
    lo: Vec3,
    hi: Vec3,
    /// Leaf: first triangle; interior: index of the right child.
    start: u32,
    /// Leaf triangle count (0 = interior; left child is the next node).
    count: u32,
}

impl SimpleBvh {
    pub fn new(mesh: &Mesh) -> SimpleBvh {
        let tris: Vec<[Vec3; 3]> = mesh.triangles.iter().map(|t| t.map(|i| Vec3::from(mesh.positions[i as usize]))).collect();
        let mut idx: Vec<u32> = (0..tris.len() as u32).collect();
        let cent: Vec<Vec3> = tris.iter().map(|t| (t[0] + t[1] + t[2]) / 3.0).collect();
        let mut nodes = Vec::with_capacity(2 * tris.len() / 4 + 1);
        if !tris.is_empty() {
            build(&tris, &cent, &mut idx, 0, &mut nodes);
        }
        let tris = idx.iter().map(|&i| tris[i as usize]).collect();
        SimpleBvh { nodes, tris }
    }
}

fn build(tris: &[[Vec3; 3]], cent: &[Vec3], idx: &mut [u32], first: u32, nodes: &mut Vec<Node>) -> usize {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    let mut clo = Vec3::splat(f32::MAX);
    let mut chi = Vec3::splat(f32::MIN);
    for &i in idx.iter() {
        for p in tris[i as usize] {
            lo = lo.min(p);
            hi = hi.max(p);
        }
        clo = clo.min(cent[i as usize]);
        chi = chi.max(cent[i as usize]);
    }
    let me = nodes.len();
    nodes.push(Node { lo, hi, start: first, count: idx.len() as u32 });
    if idx.len() <= 4 {
        return me;
    }
    let ext = chi - clo;
    let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
    let mid = idx.len() / 2;
    idx.select_nth_unstable_by(mid, |a, b| cent[*a as usize][axis].total_cmp(&cent[*b as usize][axis]));
    let (l, r) = idx.split_at_mut(mid);
    build(tris, cent, l, first, nodes);
    let right = build(tris, cent, r, first + mid as u32, nodes);
    nodes[me].count = 0;
    nodes[me].start = right as u32;
    me
}

#[inline]
fn slab(lo: Vec3, hi: Vec3, o: Vec3, inv: Vec3, tmax: f32) -> bool {
    let t0 = (lo - o) * inv;
    let t1 = (hi - o) * inv;
    let tn = t0.min(t1).max_element().max(0.0);
    let tf = t0.max(t1).min_element().min(tmax);
    tn <= tf
}

impl Occluder for SimpleBvh {
    fn occluded(&self, origin: [f32; 3], dir: [f32; 3], max_dist: f32) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        let o = Vec3::from(origin);
        let d = Vec3::from(dir);
        let inv = Vec3::ONE / d;
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let ni = stack[sp];
            let n = self.nodes[ni as usize];
            if !slab(n.lo, n.hi, o, inv, max_dist) {
                continue;
            }
            if n.count > 0 {
                for t in &self.tris[n.start as usize..(n.start + n.count) as usize] {
                    let e1 = t[1] - t[0];
                    let e2 = t[2] - t[0];
                    let pv = d.cross(e2);
                    let det = e1.dot(pv);
                    if det.abs() < 1e-12 {
                        continue;
                    }
                    let inv_det = 1.0 / det;
                    let tv = o - t[0];
                    let u = tv.dot(pv) * inv_det;
                    if !(0.0..=1.0).contains(&u) {
                        continue;
                    }
                    let q = tv.cross(e1);
                    let v = d.dot(q) * inv_det;
                    if v < 0.0 || u + v > 1.0 {
                        continue;
                    }
                    let tt = e2.dot(q) * inv_det;
                    if tt > 1e-6 && tt < max_dist {
                        return true;
                    }
                }
            } else if sp + 2 <= stack.len() {
                // Left child follows its parent; `start` is the right child.
                stack[sp] = ni + 1;
                stack[sp + 1] = n.start;
                sp += 2;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests;
