//! Dev-only test piece: the smoke example's procedural ring (rippled band,
//! head toward +Y, finger axis Z) plus a faceted stone on the head, so the
//! studio can be exercised before real meshes load. Listed only when
//! `ARGENT_STUDIO_DEMO=1` or when the library holds no meshes.

use argent_core::geom::V3;
use argent_mesh::{Mesh, Part, Pose};

pub const ID: &str = "demo:ring";
pub const NAME: &str = "Demo ring (procedural)";

pub fn mesh() -> Mesh {
    let mut m = band();
    let stone = stone(V3::new(0.0, 13.75, 0.0), 2.3);
    m.merge(stone);
    m
}

/// Rippled band, as in `argent-core/examples/smoke.rs`.
fn band() -> Mesh {
    let (nu, nv) = (720u32, 160u32);
    let r_in = 9.0f32;
    let mut m = Mesh::default();
    for i in 0..nu {
        let u = i as f32 / nu as f32 * std::f32::consts::TAU;
        let head = (u.sin().max(0.0)).powf(3.0);
        let thick = 1.6 + 2.6 * head;
        let width = 3.2 + 4.0 * head;
        for j in 0..nv {
            let v = j as f32 / nv as f32 * std::f32::consts::TAU;
            let ripple = 0.22 * head * ((u * 40.0).sin() * (v * 6.0).cos()).max(-0.3);
            let rr = r_in + thick * 0.5 + (thick * 0.5 + ripple) * v.cos();
            let z = width * 0.5 * v.sin();
            m.positions.push([rr * u.cos(), rr * u.sin(), z]);
        }
    }
    for i in 0..nu {
        for j in 0..nv {
            let a = i * nv + j;
            let b = ((i + 1) % nu) * nv + j;
            let c = ((i + 1) % nu) * nv + (j + 1) % nv;
            let d = i * nv + (j + 1) % nv;
            m.triangles.push([a, b, c]);
            m.triangles.push([a, c, d]);
        }
    }
    let mut n = vec![V3::ZERO; m.positions.len()];
    for t in &m.triangles {
        let [a, b, c] = t.map(|i| V3::from(m.positions[i as usize]));
        let f = (b - a).cross(c - a);
        for &i in t {
            n[i as usize] += f;
        }
    }
    m.normals = n.iter().map(|v| (-v.normalize_or_zero()).into()).collect();
    let outward = V3::from(m.positions[0]).dot(V3::from(m.normals[0])) > 0.0;
    if !outward {
        for nn in &mut m.normals {
            *nn = (-V3::from(*nn)).into();
        }
    }
    let nt = m.triangles.len() as u32;
    m.parts = vec![Part { name: "band".into(), triangles: 0..nt, material: None }];
    m
}

/// A 16-sided brilliant-ish stone (table, crown, pavilion), flat-faceted,
/// girdle centred at `c` with radius `r`, table up (+Y).
fn stone(c: V3, r: f32) -> Mesh {
    let k = 16usize;
    let ring = |rad: f32, y: f32, phase: f32| -> Vec<V3> {
        (0..k)
            .map(|i| {
                let a = (i as f32 + phase) / k as f32 * std::f32::consts::TAU;
                c + V3::new(rad * a.cos(), y, rad * a.sin())
            })
            .collect()
    };
    let girdle = ring(r, 0.0, 0.0);
    let table = ring(r * 0.56, r * 0.33, 0.5);
    let top = c + V3::new(0.0, r * 0.33, 0.0);
    let culet = c - V3::new(0.0, r * 0.86, 0.0);
    let mut tris: Vec<[V3; 3]> = vec![];
    for i in 0..k {
        let j = (i + 1) % k;
        tris.push([top, table[i], table[j]]);
        tris.push([girdle[i], girdle[j], table[i]]);
        tris.push([table[i], girdle[j], table[j]]);
        tris.push([girdle[i], culet, girdle[j]]);
    }
    let mut m = Mesh::default();
    for t in tris {
        let mut t = t;
        let mut n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
        let centroid = (t[0] + t[1] + t[2]) / 3.0;
        if n.dot(centroid - c) < 0.0 {
            t.swap(1, 2);
            n = -n;
        }
        let base = m.positions.len() as u32;
        for p in t {
            m.positions.push(p.into());
            m.normals.push(n.into());
        }
        m.triangles.push([base, base + 1, base + 2]);
    }
    let nt = m.triangles.len() as u32;
    m.parts = vec![Part { name: "stone_00".into(), triangles: 0..nt, material: Some("gem".into()) }];
    m
}

/// The demo is already in the canonical frame: only centre it.
pub fn pose(m: &Mesh) -> Pose {
    let (lo, hi) = m.bounds();
    Pose { center: [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0], ..Pose::IDENTITY }
}
