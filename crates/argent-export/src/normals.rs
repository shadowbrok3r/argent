//! Shading normals for simplified meshes: plain angle-weighted normals, and
//! a crease split that gives every smooth fan around a vertex its own copy
//! (the same rule argent-mesh's loader uses).

use crate::math::{D3, add, corner_angle, d3, dot, normal_area2, scale, unit_f32};
use crate::parts::PartTable;
use argent_mesh::Mesh;
use rayon::prelude::*;

/// Angle-weighted (Thürmer–Wüthrich) vertex normals, without splitting.
pub(crate) fn smooth_normals(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Vec<[f32; 3]> {
    let mut acc = vec![[0.0f64; 3]; positions.len()];
    for t in triangles {
        let p = t.map(|i| d3(positions[i as usize]));
        let (n, a2) = normal_area2(p[0], p[1], p[2]);
        if a2 <= 0.0 {
            continue;
        }
        for k in 0..3 {
            let w = corner_angle(p[k], p[(k + 1) % 3], p[(k + 2) % 3]);
            let v = t[k] as usize;
            acc[v] = add(acc[v], scale(n, w));
        }
    }
    acc.into_iter().map(unit_f32).collect()
}

/// Split `mesh` along creases sharper than `crease_angle_deg` and compute
/// angle-weighted normals per smooth fan (0° gives facet normals).
///
/// Returns the new mesh (same triangles and parts, possibly more vertices)
/// and, for every new vertex, the input vertex it was copied from, so
/// per-vertex attributes follow with `attr[src[v]]`. Faces of different
/// parts never share a vertex afterwards. Unreferenced input vertices are
/// dropped.
pub fn crease_normals(mesh: &Mesh, crease_angle_deg: f32) -> (Mesh, Vec<u32>) {
    let table = PartTable::new(mesh, "mesh");
    let cos = (crease_angle_deg.clamp(0.0, 180.0) as f64).to_radians().cos();
    crease_split(mesh, &table.tri_part, &vec![cos; table.len()])
}

/// [`crease_normals`] with a crease cosine per part.
pub(crate) fn crease_split(mesh: &Mesh, tri_part: &[u32], part_cos: &[f64]) -> (Mesh, Vec<u32>) {
    let nv = mesh.positions.len();
    let tris = &mesh.triangles;
    let pos: Vec<D3> = mesh.positions.par_iter().map(|p| d3(*p)).collect();
    let fnorm: Vec<(D3, bool)> = tris
        .par_iter()
        .map(|t| {
            let (n, a2) = normal_area2(pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
            (n, a2 > 0.0)
        })
        .collect();

    // Vertex → faces (ascending; a repeated corner is listed once).
    let mut start = vec![0u32; nv + 1];
    for t in tris {
        for k in 0..3 {
            if (0..k).all(|j| t[j] != t[k]) {
                start[t[k] as usize + 1] += 1;
            }
        }
    }
    for v in 0..nv {
        start[v + 1] += start[v];
    }
    let mut refs = vec![0u32; start[nv] as usize];
    let mut cur = start.clone();
    for (f, t) in tris.iter().enumerate() {
        for k in 0..3 {
            if (0..k).all(|j| t[j] != t[k]) {
                let v = t[k] as usize;
                refs[cur[v] as usize] = f as u32;
                cur[v] += 1;
            }
        }
    }
    drop(cur);

    // Per vertex: a fan label per incident face, and one normal per fan.
    let fans: Vec<(Vec<u32>, Vec<[f32; 3]>)> = (0..nv)
        .into_par_iter()
        .with_min_len(256)
        .map_init(FanScratch::default, |s, v| {
            let faces = &refs[start[v] as usize..start[v + 1] as usize];
            fan_split(v as u32, faces, tris, &pos, &fnorm, tri_part, part_cos, s)
        })
        .collect();

    let mut base = vec![0u32; nv + 1];
    for v in 0..nv {
        base[v + 1] = base[v] + fans[v].1.len() as u32;
    }
    let total = base[nv] as usize;
    let mut positions = Vec::with_capacity(total);
    let mut normals = Vec::with_capacity(total);
    let mut src = Vec::with_capacity(total);
    for (v, (_, ns)) in fans.iter().enumerate() {
        for n in ns {
            positions.push(mesh.positions[v]);
            normals.push(*n);
            src.push(v as u32);
        }
    }
    let triangles: Vec<[u32; 3]> = tris
        .par_iter()
        .enumerate()
        .with_min_len(1024)
        .map(|(f, t)| {
            t.map(|v| {
                let v = v as usize;
                let faces = &refs[start[v] as usize..start[v + 1] as usize];
                let j = faces.binary_search(&(f as u32)).expect("face listed at its vertex");
                base[v] + fans[v].0[j]
            })
        })
        .collect();
    (Mesh { positions, normals, triangles, parts: mesh.parts.clone() }, src)
}

#[derive(Default)]
struct FanScratch {
    /// (other vertex, local face, edge leaves v in this face's winding)
    ev: Vec<(u32, u32, bool)>,
    uf: Vec<u32>,
    acc: Vec<D3>,
}

#[allow(clippy::too_many_arguments)]
fn fan_split(
    v: u32,
    faces: &[u32],
    tris: &[[u32; 3]],
    pos: &[D3],
    fnorm: &[(D3, bool)],
    tri_part: &[u32],
    part_cos: &[f64],
    s: &mut FanScratch,
) -> (Vec<u32>, Vec<[f32; 3]>) {
    let k = faces.len();
    if k == 0 {
        return (Vec::new(), Vec::new());
    }
    s.ev.clear();
    for (li, &f) in faces.iter().enumerate() {
        let t = tris[f as usize];
        if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
            continue; // repeated corner: its own fan
        }
        let j = (0..3).find(|&j| t[j] == v).unwrap_or(0);
        s.ev.push((t[(j + 1) % 3], li as u32, true));
        s.ev.push((t[(j + 2) % 3], li as u32, false));
    }
    s.ev.sort_unstable();
    s.uf.clear();
    s.uf.extend(0..k as u32);
    let mut i = 0;
    while i < s.ev.len() {
        let mut j = i + 1;
        while j < s.ev.len() && s.ev[j].0 == s.ev[i].0 {
            j += 1;
        }
        // Join across a manifold, consistently wound edge of one part whose
        // dihedral angle is below that part's crease angle.
        if j - i == 2 && s.ev[i].2 != s.ev[i + 1].2 {
            let (la, lb) = (s.ev[i].1, s.ev[i + 1].1);
            let (fa, fb) = (faces[la as usize] as usize, faces[lb as usize] as usize);
            let part = tri_part[fa];
            if part == tri_part[fb] {
                let (na, va) = fnorm[fa];
                let (nb, vb) = fnorm[fb];
                if !va || !vb || dot(na, nb) >= part_cos[part as usize] {
                    let (ra, rb) = (find(&mut s.uf, la), find(&mut s.uf, lb));
                    if ra != rb {
                        s.uf[ra.max(rb) as usize] = ra.min(rb);
                    }
                }
            }
        }
        i = j;
    }
    // Compact labels in order of first face; accumulate weighted normals.
    let mut labels = vec![u32::MAX; k];
    let mut nfans = 0u32;
    for li in 0..k {
        let r = find(&mut s.uf, li as u32) as usize;
        if labels[r] == u32::MAX {
            labels[r] = nfans;
            nfans += 1;
        }
        labels[li] = labels[r];
    }
    s.acc.clear();
    s.acc.resize(nfans as usize, [0.0; 3]);
    let pv = pos[v as usize];
    for (li, &f) in faces.iter().enumerate() {
        let (n, valid) = fnorm[f as usize];
        if !valid {
            continue;
        }
        let t = tris[f as usize];
        let j = (0..3).find(|&j| t[j] == v).unwrap_or(0);
        let w = corner_angle(pv, pos[t[(j + 1) % 3] as usize], pos[t[(j + 2) % 3] as usize]);
        let l = labels[li] as usize;
        s.acc[l] = add(s.acc[l], scale(n, w));
    }
    let normals = s.acc.iter().map(|n| unit_f32(*n)).collect();
    (labels, normals)
}

fn find(p: &mut [u32], mut x: u32) -> u32 {
    while p[x as usize] != x {
        p[x as usize] = p[p[x as usize] as usize];
        x = p[x as usize];
    }
    x
}
