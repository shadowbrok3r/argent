//! Vertex welding.
//!
//! Two stages, both parallel and deterministic:
//! 1. exact: vertices with bit-identical coordinates (what STL exporters
//!    write for shared corners) are merged by sorting packed coordinate bits;
//! 2. tolerance: the survivors are hashed into a grid of cells 8× the
//!    tolerance; only points within `eps` of a cell wall probe the neighbour
//!    cell, so the common case is one binary search per point.
//!
//! Merged vertices keep the position of their first occurrence and new
//! vertex ids follow first-occurrence order (preserves file locality).
//! Triangles that collapse (repeated index) are dropped; unreferenced
//! vertices are removed.

use crate::util::{UnionFind, retain_triangles};
use crate::Mesh;
use rayon::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};

/// What [`weld`] did.
#[derive(Clone, Debug, Default)]
pub struct WeldReport {
    pub input_vertices: usize,
    pub output_vertices: usize,
    /// Vertices merged because their coordinates were bit-identical.
    pub exact_merges: usize,
    /// Further vertices merged by distance (within `tolerance_mm`).
    pub tolerance_merges: usize,
    /// Triangles removed because two of their corners merged.
    pub dropped_triangles: usize,
    /// Absolute weld distance actually used.
    pub tolerance_mm: f32,
}

/// Weld `mesh` in place with tolerance `rel_tolerance × bbox diagonal`
/// (the bounding box of referenced vertices). Clears `normals` (they no
/// longer match); call [`crate::normals::compute`] afterwards.
pub fn weld(mesh: &mut Mesh, rel_tolerance: f32) -> WeldReport {
    let used = referenced(mesh);
    let (lo, hi) = bounds_of(&mesh.positions, &used);
    let diag = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt();
    let eps = (rel_tolerance.max(0.0) as f64 * diag as f64) as f32;
    let (positions, remap, stats) = weld_positions_impl(&mesh.positions, Some(&used), eps);
    let mut report = WeldReport {
        input_vertices: mesh.positions.len(),
        output_vertices: positions.len(),
        exact_merges: stats.0,
        tolerance_merges: stats.1,
        dropped_triangles: 0,
        tolerance_mm: eps,
    };
    mesh.positions = positions;
    mesh.normals.clear();
    mesh.triangles.par_iter_mut().with_min_len(4096).for_each(|t| {
        *t = [remap[t[0] as usize], remap[t[1] as usize], remap[t[2] as usize]];
    });
    report.dropped_triangles = drop_collapsed(mesh);
    report
}

/// Remove unreferenced vertices and triangles with a repeated index, without
/// merging anything. Returns the number of triangles removed.
pub fn compact(mesh: &mut Mesh) -> usize {
    let dropped = drop_collapsed(mesh);
    let used = referenced(mesh);
    if used.iter().all(|u| *u) {
        return dropped;
    }
    let mut remap = vec![u32::MAX; mesh.positions.len()];
    let mut positions = Vec::with_capacity(mesh.positions.len());
    let mut normals = Vec::new();
    let keep_normals = mesh.normals.len() == mesh.positions.len();
    for (i, &u) in used.iter().enumerate() {
        if u {
            remap[i] = positions.len() as u32;
            positions.push(mesh.positions[i]);
            if keep_normals {
                normals.push(mesh.normals[i]);
            }
        }
    }
    mesh.positions = positions;
    mesh.normals = normals;
    mesh.triangles.par_iter_mut().with_min_len(4096).for_each(|t| {
        *t = [remap[t[0] as usize], remap[t[1] as usize], remap[t[2] as usize]];
    });
    dropped
}

/// Weld a bare position list with absolute tolerance `eps` (0 = exact only).
/// Returns the unique positions and, for every input vertex, its new index.
pub fn weld_positions(positions: &[[f32; 3]], eps: f32) -> (Vec<[f32; 3]>, Vec<u32>) {
    let (p, r, _) = weld_positions_impl(positions, None, eps);
    (p, r)
}

fn drop_collapsed(mesh: &mut Mesh) -> usize {
    let keep: Vec<bool> = mesh
        .triangles
        .par_iter()
        .with_min_len(4096)
        .map(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
        .collect();
    retain_triangles(mesh, &keep)
}

fn referenced(mesh: &Mesh) -> Vec<bool> {
    let mut used = vec![false; mesh.positions.len()];
    for t in &mesh.triangles {
        for &v in t {
            if let Some(u) = used.get_mut(v as usize) {
                *u = true;
            }
        }
    }
    used
}

fn bounds_of(p: &[[f32; 3]], used: &[bool]) -> ([f32; 3], [f32; 3]) {
    let (lo, hi) = p
        .par_iter()
        .zip(used.par_iter())
        .with_min_len(8192)
        .filter(|(_, u)| **u)
        .fold(
            || ([f32::MAX; 3], [f32::MIN; 3]),
            |(mut lo, mut hi), (q, _)| {
                for k in 0..3 {
                    lo[k] = lo[k].min(q[k]);
                    hi[k] = hi[k].max(q[k]);
                }
                (lo, hi)
            },
        )
        .reduce(
            || ([f32::MAX; 3], [f32::MIN; 3]),
            |a, b| {
                let mut lo = a.0;
                let mut hi = a.1;
                for k in 0..3 {
                    lo[k] = lo[k].min(b.0[k]);
                    hi[k] = hi[k].max(b.1[k]);
                }
                (lo, hi)
            },
        );
    if lo[0] > hi[0] { ([0.0; 3], [0.0; 3]) } else { (lo, hi) }
}

#[inline]
fn canon_bits(x: f32) -> u32 {
    // -0.0 and +0.0 must collide.
    (x + 0.0).to_bits()
}

/// Returns (unique positions, remap per input vertex (u32::MAX if unused),
/// (exact merges, tolerance merges)).
fn weld_positions_impl(
    positions: &[[f32; 3]],
    used: Option<&[bool]>,
    eps: f32,
) -> (Vec<[f32; 3]>, Vec<u32>, (usize, usize)) {
    let n = positions.len();
    assert!(n < u32::MAX as usize, "argent-mesh: too many vertices");
    let is_used = |i: usize| used.is_none_or(|u| u[i]);

    // ---- stage 1: exact duplicates --------------------------------------
    let mut keys: Vec<u128> = (0..n)
        .into_par_iter()
        .with_min_len(8192)
        .filter(|&i| is_used(i))
        .map(|i| {
            let p = positions[i];
            ((canon_bits(p[0]) as u128) << 96)
                | ((canon_bits(p[1]) as u128) << 64)
                | ((canon_bits(p[2]) as u128) << 32)
                | i as u128
        })
        .collect();
    keys.par_sort_unstable();
    let used_count = keys.len();

    // rep[i] = smallest index with identical coordinates (run head).
    let rep: Vec<AtomicU32> = (0..n).map(|_| AtomicU32::new(u32::MAX)).collect();
    // Split sorted keys into chunks at run boundaries, fill in parallel.
    let nchunks = (used_count / 65536).clamp(1, 1024);
    let mut bounds = Vec::with_capacity(nchunks + 1);
    bounds.push(0usize);
    for c in 1..nchunks {
        let mut b = used_count * c / nchunks;
        let prev = *bounds.last().unwrap();
        if b <= prev {
            continue;
        }
        while b < used_count && (keys[b] >> 32) == (keys[b - 1] >> 32) {
            b += 1;
        }
        if b < used_count && b > prev {
            bounds.push(b);
        }
    }
    bounds.push(used_count);
    bounds.par_windows(2).for_each(|w| {
        let mut head = u32::MAX;
        let mut head_key = u128::MAX;
        for &k in &keys[w[0]..w[1]] {
            let coord = k >> 32;
            let idx = k as u32;
            if coord != head_key {
                head_key = coord;
                head = idx;
            }
            rep[idx as usize].store(head, Ordering::Relaxed);
        }
    });
    drop(keys);
    let rep: Vec<u32> = rep.into_iter().map(|a| a.into_inner()).collect();

    // Unique ids in first-occurrence order.
    let mut uid = vec![u32::MAX; n];
    let mut uniq: Vec<[f32; 3]> = Vec::new();
    for i in 0..n {
        let r = rep[i];
        if r == u32::MAX {
            continue;
        }
        if r as usize == i {
            uid[i] = uniq.len() as u32;
            uniq.push(positions[i]);
        } else {
            uid[i] = uid[r as usize];
        }
    }
    drop(rep);
    let exact_merges = used_count - uniq.len();

    // ---- stage 2: tolerance ---------------------------------------------
    let m = uniq.len();
    let mut root: Option<Vec<u32>> = None;
    if eps > 0.0 && m > 1 {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in &uniq {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let ext = (0..3).map(|k| (hi[k] - lo[k]) as f64).fold(0.0, f64::max);
        let h = (8.0 * eps as f64).max(ext / ((1u64 << 20) as f64)).max(1e-30);
        let cell_of = |p: &[f32; 3]| -> [i64; 3] {
            [
                ((p[0] as f64 - lo[0] as f64) / h).floor() as i64,
                ((p[1] as f64 - lo[1] as f64) / h).floor() as i64,
                ((p[2] as f64 - lo[2] as f64) / h).floor() as i64,
            ]
        };
        let pack = |c: [i64; 3]| -> u64 { ((c[0] as u64) << 42) | ((c[1] as u64) << 21) | (c[2] as u64) };
        let mut cells: Vec<(u64, u32)> =
            uniq.par_iter().enumerate().with_min_len(8192).map(|(i, p)| (pack(cell_of(p)), i as u32)).collect();
        cells.par_sort_unstable();
        let eps2 = (eps as f64) * (eps as f64);
        let pairs: Vec<(u32, u32)> = (0..m)
            .into_par_iter()
            .with_min_len(4096)
            .flat_map_iter(|i| {
                let p = uniq[i];
                let c = cell_of(&p);
                let mut offs: [[i64; 2]; 3] = [[0, 0]; 3];
                let mut nofs = [1usize; 3];
                for k in 0..3 {
                    let base = lo[k] as f64 + c[k] as f64 * h;
                    let f = p[k] as f64 - base;
                    offs[k][0] = 0;
                    if f < eps as f64 && c[k] > 0 {
                        offs[k][nofs[k]] = -1;
                        nofs[k] += 1;
                    } else if h - f < eps as f64 && c[k] < (1 << 21) - 1 {
                        offs[k][nofs[k]] = 1;
                        nofs[k] += 1;
                    }
                }
                let mut out: Vec<(u32, u32)> = Vec::new();
                for a in 0..nofs[0] {
                    for b in 0..nofs[1] {
                        for d in 0..nofs[2] {
                            let cc = [c[0] + offs[0][a], c[1] + offs[1][b], c[2] + offs[2][d]];
                            let key = pack(cc);
                            let start = cells.partition_point(|e| e.0 < key);
                            for &(k2, j) in &cells[start..] {
                                if k2 != key {
                                    break;
                                }
                                if (j as usize) < i {
                                    let q = uniq[j as usize];
                                    let dx = p[0] as f64 - q[0] as f64;
                                    let dy = p[1] as f64 - q[1] as f64;
                                    let dz = p[2] as f64 - q[2] as f64;
                                    if dx * dx + dy * dy + dz * dz <= eps2 {
                                        out.push((i as u32, j));
                                    }
                                }
                            }
                        }
                    }
                }
                out
            })
            .collect();
        if !pairs.is_empty() {
            let mut uf = UnionFind::new(m);
            for (a, b) in pairs {
                uf.union(a, b);
            }
            root = Some((0..m as u32).map(|u| uf.find(u)).collect());
        }
    }

    let (final_pos, uid_to_final, tol_merges) = match root {
        None => (uniq, None, 0),
        Some(root) => {
            let mut cid = vec![0u32; m];
            let mut out = Vec::with_capacity(m);
            for u in 0..m {
                let r = root[u] as usize;
                if r == u {
                    cid[u] = out.len() as u32;
                    out.push(uniq[u]);
                } else {
                    cid[u] = cid[r];
                }
            }
            let merged = m - out.len();
            (out, Some(cid), merged)
        }
    };
    let remap: Vec<u32> = match uid_to_final {
        None => uid,
        Some(cid) => uid.par_iter().map(|&u| if u == u32::MAX { u32::MAX } else { cid[u as usize] }).collect(),
    };
    (final_pos, remap, (exact_merges, tol_merges))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth;

    #[test]
    fn cube_soup_welds_to_8() {
        let mut m = synth::cuboid([2.0, 2.0, 2.0]);
        synth::unweld(&mut m);
        assert_eq!(m.positions.len(), 36);
        let r = weld(&mut m, 1e-6);
        assert_eq!(m.positions.len(), 8);
        assert_eq!(m.triangles.len(), 12);
        assert_eq!(r.exact_merges, 28);
        assert!((m.volume() - 8.0).abs() < 1e-5);
    }

    #[test]
    fn near_duplicates_merge_within_tolerance_only() {
        let mut m = synth::cuboid([2.0, 2.0, 2.0]);
        synth::unweld(&mut m);
        // Jitter every vertex by up to 1e-7 mm: tolerance (1e-6 × diag ≈ 3.5e-6) catches it.
        for (i, p) in m.positions.iter_mut().enumerate() {
            let j = (crate::util::unit_f32(crate::util::hash32(i as u32)) - 0.5) * 2e-7;
            p[0] += j;
        }
        let r = weld(&mut m, 1e-6);
        assert_eq!(m.positions.len(), 8, "{r:?}");
        assert!(r.tolerance_merges > 0);
        // With zero tolerance only exact matches merge.
        let mut m2 = synth::cuboid([2.0, 2.0, 2.0]);
        synth::unweld(&mut m2);
        m2.positions[0][0] += 1e-4;
        weld(&mut m2, 0.0);
        assert_eq!(m2.positions.len(), 9);
    }

    #[test]
    fn collapsed_triangles_dropped_and_parts_updated() {
        let mut m = Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1e-9, 0.0], [5.0, 5.0, 5.0]],
            normals: vec![],
            triangles: vec![[0, 1, 2], [0, 1, 3], [1, 3, 2]],
            parts: vec![
                crate::Part { name: "a".into(), triangles: 0..1, material: None },
                crate::Part { name: "b".into(), triangles: 1..3, material: Some("gold".into()) },
            ],
        };
        let r = weld(&mut m, 1e-6);
        // vertex 3 merges into 1: triangles 1 and 2 collapse; vertex 4 is unreferenced.
        assert_eq!(r.dropped_triangles, 2);
        assert_eq!(m.triangles.len(), 1);
        assert_eq!(m.positions.len(), 3);
        assert_eq!(m.parts.len(), 1);
        assert_eq!(m.parts[0].name, "a");
    }

    #[test]
    fn welding_scales_and_is_deterministic() {
        let mut a = synth::torus(10.0, 2.0, 400, 200);
        synth::unweld(&mut a);
        let mut b = a.clone();
        weld(&mut a, 1e-6);
        weld(&mut b, 1e-6);
        assert_eq!(a.positions.len(), 400 * 200);
        assert_eq!(a.triangles, b.triangles);
    }
}
