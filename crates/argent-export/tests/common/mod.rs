//! Test helpers: synthetic meshes, tiny STL/OBJ readers, and a GLB parser
//! that checks the structure the glTF 2.0 spec requires.
#![allow(dead_code)]

use argent_mesh::{Mesh, Part};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

pub fn finish(positions: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>, name: &str, material: Option<&str>) -> Mesh {
    let n = triangles.len() as u32;
    Mesh {
        normals: vec![[0.0, 0.0, 1.0]; positions.len()],
        positions,
        triangles,
        parts: vec![Part { name: name.into(), triangles: 0..n, material: material.map(String::from) }],
    }
}

/// Closed UV sphere, outward winding: `nu` around, `nv` rings pole to pole.
pub fn uv_sphere(r: f32, nu: usize, nv: usize) -> Mesh {
    let mut p = vec![[0.0, 0.0, r]];
    for i in 1..nv {
        let th = std::f32::consts::PI * i as f32 / nv as f32;
        for j in 0..nu {
            let ph = std::f32::consts::TAU * j as f32 / nu as f32;
            p.push([r * th.sin() * ph.cos(), r * th.sin() * ph.sin(), r * th.cos()]);
        }
    }
    p.push([0.0, 0.0, -r]);
    let south = (p.len() - 1) as u32;
    let ring = |i: usize, j: usize| (1 + (i - 1) * nu + j % nu) as u32;
    let mut t = Vec::new();
    for j in 0..nu {
        t.push([0, ring(1, j), ring(1, j + 1)]);
    }
    for i in 1..nv - 1 {
        for j in 0..nu {
            let (a, b, c, d) = (ring(i, j), ring(i, j + 1), ring(i + 1, j + 1), ring(i + 1, j));
            t.push([a, d, b]);
            t.push([b, d, c]);
        }
    }
    for j in 0..nu {
        t.push([ring(nv - 1, j), south, ring(nv - 1, j + 1)]);
    }
    finish(p, t, "sphere", None)
}

/// Closed torus about Z, outward winding.
pub fn torus(big_r: f32, r: f32, nu: usize, nv: usize) -> Mesh {
    let mut p = Vec::with_capacity(nu * nv);
    for i in 0..nu {
        let u = std::f32::consts::TAU * i as f32 / nu as f32;
        for j in 0..nv {
            let v = std::f32::consts::TAU * j as f32 / nv as f32;
            let w = big_r + r * v.cos();
            p.push([w * u.cos(), w * u.sin(), r * v.sin()]);
        }
    }
    let id = |i: usize, j: usize| ((i % nu) * nv + j % nv) as u32;
    let mut t = Vec::with_capacity(2 * nu * nv);
    for i in 0..nu {
        for j in 0..nv {
            let (a, b, c, d) = (id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1));
            t.push([a, b, c]);
            t.push([a, c, d]);
        }
    }
    finish(p, t, "torus", None)
}

/// Open `w × h` plate in XY (normal +Z) with a bump that is zero on the rim.
pub fn plate(w: f32, h: f32, nx: usize, ny: usize, bump: f32) -> Mesh {
    let mut p = Vec::new();
    for j in 0..=ny {
        for i in 0..=nx {
            let (x, y) = (w * i as f32 / nx as f32, h * j as f32 / ny as f32);
            let z = bump * (std::f32::consts::PI * x / w).sin() * (std::f32::consts::PI * y / h).sin();
            p.push([x, y, z]);
        }
    }
    let id = |i: usize, j: usize| (j * (nx + 1) + i) as u32;
    let mut t = Vec::new();
    for j in 0..ny {
        for i in 0..nx {
            let (a, b, c, d) = (id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1));
            t.push([a, b, c]);
            t.push([a, c, d]);
        }
    }
    finish(p, t, "plate", None)
}

/// Unit cube (welded, 8 vertices).
pub fn cube(s: f32) -> Mesh {
    let p: Vec<[f32; 3]> =
        (0..8).map(|i| [(i & 1) as f32 * s, ((i >> 1) & 1) as f32 * s, ((i >> 2) & 1) as f32 * s]).collect();
    let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    let mut t = Vec::new();
    for q in quads {
        t.push([q[0], q[1], q[2]]);
        t.push([q[0], q[2], q[3]]);
    }
    finish(p, t, "cube", None)
}

/// Every triangle gets its own three vertices, like an STL file.
pub fn unweld(m: &Mesh) -> Mesh {
    let mut p = Vec::with_capacity(m.triangles.len() * 3);
    let mut t = Vec::with_capacity(m.triangles.len());
    for tri in &m.triangles {
        let b = p.len() as u32;
        for &i in tri {
            p.push(m.positions[i as usize]);
        }
        t.push([b, b + 1, b + 2]);
    }
    Mesh { normals: vec![[0.0, 0.0, 1.0]; p.len()], positions: p, triangles: t, parts: m.parts.clone() }
}

pub fn translate(m: &mut Mesh, d: [f32; 3]) {
    for p in &mut m.positions {
        for k in 0..3 {
            p[k] += d[k];
        }
    }
}

/// Binary STL as a triangle soup (one part named after the file stem).
pub fn read_stl(path: &Path) -> Mesh {
    let b = std::fs::read(path).unwrap();
    let n = u32::from_le_bytes(b[80..84].try_into().unwrap()) as usize;
    assert_eq!(b.len(), 84 + 50 * n, "binary STL size");
    let f = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let mut p = Vec::with_capacity(3 * n);
    for i in 0..n {
        let o = 84 + 50 * i + 12;
        for k in 0..3 {
            p.push([f(o + 12 * k), f(o + 12 * k + 4), f(o + 12 * k + 8)]);
        }
    }
    let t = (0..n as u32).map(|i| [3 * i, 3 * i + 1, 3 * i + 2]).collect();
    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
    finish(p, t, &stem, None)
}

/// Minimal OBJ: `v`, `f` (fan-triangulated, `i/t/n` ok), `o`/`g`, `usemtl`.
pub fn read_obj(path: &Path) -> Mesh {
    let text = std::fs::read_to_string(path).unwrap();
    let mut p = Vec::new();
    let mut t = Vec::new();
    let mut parts: Vec<Part> = Vec::new();
    let mut name = "obj".to_string();
    let mut mat: Option<String> = None;
    fn open(parts: &mut Vec<Part>, start: u32, name: &str, mat: &Option<String>) {
        if let Some(last) = parts.last_mut()
            && last.triangles.start == start
        {
            last.name = name.into();
            last.material = mat.clone();
            return;
        }
        if let Some(last) = parts.last_mut() {
            last.triangles.end = start;
        }
        parts.push(Part { name: name.into(), triangles: start..start, material: mat.clone() });
    }
    for line in text.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let v: Vec<f32> = it.take(3).map(|x| x.parse().unwrap()).collect();
                p.push([v[0], v[1], v[2]]);
            }
            Some("o") | Some("g") => {
                name = it.collect::<Vec<_>>().join(" ");
                open(&mut parts, t.len() as u32, &name, &mat);
            }
            Some("usemtl") => {
                mat = it.next().map(String::from);
                open(&mut parts, t.len() as u32, &name, &mat);
            }
            Some("f") => {
                if parts.is_empty() {
                    open(&mut parts, 0, &name, &mat);
                }
                let idx: Vec<u32> = it
                    .map(|s| {
                        let i: i64 = s.split('/').next().unwrap().parse().unwrap();
                        (if i < 0 { p.len() as i64 + i } else { i - 1 }) as u32
                    })
                    .collect();
                for k in 1..idx.len() - 1 {
                    t.push([idx[0], idx[k], idx[k + 1]]);
                }
            }
            _ => {}
        }
    }
    if let Some(last) = parts.last_mut() {
        last.triangles.end = t.len() as u32;
    }
    parts.retain(|q| !q.triangles.is_empty());
    Mesh { normals: vec![[0.0, 0.0, 1.0]; p.len()], positions: p, triangles: t, parts }
}

/// Every undirected edge is shared by exactly two triangles, traversed in
/// opposite directions (closed, consistently wound).
pub fn closed_and_oriented(m: &Mesh) -> bool {
    let mut e: Vec<(u32, u32, bool)> = Vec::new();
    for t in &m.triangles {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            e.push((a.min(b), a.max(b), a < b));
        }
    }
    e.sort_unstable();
    let mut i = 0;
    while i < e.len() {
        let mut j = i + 1;
        while j < e.len() && (e[j].0, e[j].1) == (e[i].0, e[i].1) {
            j += 1;
        }
        if j - i != 2 || e[i].2 == e[i + 1].2 {
            return false;
        }
        i = j;
    }
    true
}

pub fn face_normal(m: &Mesh, t: [u32; 3]) -> ([f64; 3], [f64; 3]) {
    let p = t.map(|i| m.positions[i as usize].map(|x| x as f64));
    let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
    let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
    let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
    let c = [0, 1, 2].map(|k| (p[0][k] + p[1][k] + p[2][k]) / 3.0);
    (n, c)
}

pub fn signed_volume(m: &Mesh) -> f64 {
    m.triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| m.positions[i as usize].map(|v| v as f64));
            (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.0
        })
        .sum()
}

/// What a GLB holds, after [`check_glb`] verified its structure.
pub struct Glb {
    pub json: Value,
    pub bin: Vec<u8>,
    pub triangles: usize,
    pub vertices: usize,
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

pub fn f32s(bin: &[u8], json: &Value, accessor: usize) -> Vec<f32> {
    let a = &json["accessors"][accessor];
    let v = &json["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
    let off = v["byteOffset"].as_u64().unwrap_or(0) as usize + a["byteOffset"].as_u64().unwrap_or(0) as usize;
    let comps = match a["type"].as_str().unwrap() {
        "SCALAR" => 1,
        "VEC3" => 3,
        "VEC4" => 4,
        t => panic!("type {t}"),
    };
    let n = a["count"].as_u64().unwrap() as usize * comps;
    (0..n).map(|i| f32::from_le_bytes(bin[off + 4 * i..off + 4 * i + 4].try_into().unwrap())).collect()
}

pub fn u32s(bin: &[u8], json: &Value, accessor: usize) -> Vec<u32> {
    let a = &json["accessors"][accessor];
    assert_eq!(a["componentType"], 5125, "u32 indices");
    let v = &json["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
    let off = v["byteOffset"].as_u64().unwrap_or(0) as usize + a["byteOffset"].as_u64().unwrap_or(0) as usize;
    let n = a["count"].as_u64().unwrap() as usize;
    (0..n).map(|i| u32_at(bin, off + 4 * i)).collect()
}

/// Parse a GLB and check what the glTF 2.0 spec requires of it: header and
/// chunk layout, 4-byte alignment, byte lengths, accessor bounds, POSITION
/// min/max, unit normals, index ranges and `extensionsUsed`.
pub fn check_glb(bytes: &[u8]) -> Glb {
    assert!(bytes.len() >= 28);
    assert_eq!(&bytes[0..4], b"glTF", "magic");
    assert_eq!(u32_at(bytes, 4), 2, "version");
    assert_eq!(u32_at(bytes, 8) as usize, bytes.len(), "header length");
    let jlen = u32_at(bytes, 12) as usize;
    assert_eq!(u32_at(bytes, 16), 0x4E4F_534A, "JSON chunk type");
    assert_eq!(jlen % 4, 0, "JSON chunk padded");
    let json: Value = serde_json::from_slice(&bytes[20..20 + jlen]).expect("JSON chunk parses");
    let bo = 20 + jlen;
    let blen = u32_at(bytes, bo) as usize;
    assert_eq!(u32_at(bytes, bo + 4), 0x004E_4942, "BIN chunk type");
    assert_eq!(blen % 4, 0, "BIN chunk padded");
    assert_eq!(bo + 8 + blen, bytes.len(), "chunks fill the file");
    let bin = bytes[bo + 8..bo + 8 + blen].to_vec();

    assert_eq!(json["asset"]["version"], "2.0");
    let buffers = json["buffers"].as_array().unwrap();
    assert_eq!(buffers.len(), 1);
    let byte_length = buffers[0]["byteLength"].as_u64().unwrap() as usize;
    assert!(byte_length <= blen && blen - byte_length < 4, "buffer.byteLength {byte_length} vs chunk {blen}");
    for v in json["bufferViews"].as_array().unwrap() {
        let off = v["byteOffset"].as_u64().unwrap_or(0) as usize;
        let len = v["byteLength"].as_u64().unwrap() as usize;
        assert_eq!(off % 4, 0, "bufferView aligned");
        assert!(off + len <= byte_length, "bufferView inside buffer");
        assert!(matches!(v["target"].as_u64(), Some(34962) | Some(34963)));
    }
    for a in json["accessors"].as_array().unwrap() {
        let v = &json["bufferViews"][a["bufferView"].as_u64().unwrap() as usize];
        let comps = match a["type"].as_str().unwrap() {
            "SCALAR" => 1,
            "VEC3" => 3,
            "VEC4" => 4,
            t => panic!("type {t}"),
        };
        let size = match a["componentType"].as_u64().unwrap() {
            5126 | 5125 => 4,
            c => panic!("componentType {c}"),
        };
        let count = a["count"].as_u64().unwrap() as usize;
        assert!(count >= 1);
        assert!(count * comps * size <= v["byteLength"].as_u64().unwrap() as usize, "accessor fits its view");
    }
    let nmat = json["materials"].as_array().map_or(0, |m| m.len());
    let (mut triangles, mut vertices) = (0, 0);
    for m in json["meshes"].as_array().unwrap() {
        for prim in m["primitives"].as_array().unwrap() {
            assert_eq!(prim["mode"].as_u64().unwrap_or(4), 4);
            assert!((prim["material"].as_u64().unwrap() as usize) < nmat);
            let attrs = &prim["attributes"];
            let pa = attrs["POSITION"].as_u64().unwrap() as usize;
            let na = attrs["NORMAL"].as_u64().unwrap() as usize;
            let count = json["accessors"][pa]["count"].as_u64().unwrap();
            assert_eq!(json["accessors"][na]["count"].as_u64().unwrap(), count);
            if let Some(ca) = attrs["COLOR_0"].as_u64() {
                assert_eq!(json["accessors"][ca as usize]["count"].as_u64().unwrap(), count);
                for c in f32s(&bin, &json, ca as usize) {
                    assert!((0.0..=1.0).contains(&c), "colour {c}");
                }
            }
            // min/max must equal the data exactly.
            let pos = f32s(&bin, &json, pa);
            let (lo, hi) = (&json["accessors"][pa]["min"], &json["accessors"][pa]["max"]);
            for k in 0..3 {
                let mn = pos.iter().skip(k).step_by(3).fold(f32::INFINITY, |a, &b| a.min(b));
                let mx = pos.iter().skip(k).step_by(3).fold(f32::NEG_INFINITY, |a, &b| a.max(b));
                // serde_json's default parser may be 1 f64-ulp off; as f32 it is exact.
                assert_eq!(lo[k].as_f64().unwrap() as f32, mn, "min[{k}]");
                assert_eq!(hi[k].as_f64().unwrap() as f32, mx, "max[{k}]");
            }
            for n in f32s(&bin, &json, na).chunks(3) {
                let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                assert!((l - 1.0).abs() < 1e-4, "normal length {l}");
            }
            let idx = u32s(&bin, &json, prim["indices"].as_u64().unwrap() as usize);
            assert_eq!(idx.len() % 3, 0);
            assert!(idx.iter().all(|&i| (i as u64) < count), "indices in range");
            assert!(
                idx.chunks(3).all(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]),
                "no index-degenerate triangles"
            );
            // Shading normals point to the same side as their triangle.
            let nrm = f32s(&bin, &json, na);
            let (mut backwards, mut faces) = (0usize, 0usize);
            for t in idx.chunks(3) {
                let p = |i: u32| [0, 1, 2].map(|k| pos[3 * i as usize + k] as f64);
                let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
                let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let g = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                if g.iter().map(|x| x * x).sum::<f64>() == 0.0 {
                    continue;
                }
                faces += 1;
                for &i in t {
                    let n = [0, 1, 2].map(|k| nrm[3 * i as usize + k] as f64);
                    if n[0] * g[0] + n[1] * g[1] + n[2] * g[2] <= 0.0 {
                        backwards += 1;
                    }
                }
            }
            assert!(
                backwards * 10_000 <= faces * 3,
                "{backwards} corner normals face away from their triangle ({faces} faces)"
            );
            triangles += idx.len() / 3;
            vertices += count as usize;
        }
    }
    let nmesh = json["meshes"].as_array().unwrap().len();
    for n in json["nodes"].as_array().unwrap() {
        if let Some(m) = n["mesh"].as_u64() {
            assert!((m as usize) < nmesh);
        }
    }
    // extensionsUsed lists exactly the extensions the materials use.
    let mut used = BTreeSet::new();
    for m in json["materials"].as_array().into_iter().flatten() {
        if let Some(e) = m["extensions"].as_object() {
            used.extend(e.keys().cloned());
        }
    }
    let declared: BTreeSet<String> =
        json["extensionsUsed"].as_array().into_iter().flatten().map(|v| v.as_str().unwrap().to_string()).collect();
    assert_eq!(used, declared, "extensionsUsed");
    Glb { json, bin, triangles, vertices }
}
