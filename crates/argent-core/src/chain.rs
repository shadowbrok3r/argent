//! Procedural chains so a pendant photographs like a pendant: a cable chain
//! of stadium-shaped links, each turned 90° from the last, hanging from the
//! bail in a V and running out of frame.

use crate::geom::V3;
use argent_mesh::{Mesh, Part};

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct ChainSpec {
    /// Wire diameter, mm.
    pub wire: f32,
    /// Outer length and width of one link, mm.
    pub link_len: f32,
    pub link_w: f32,
    /// Half-angle of the V from vertical, degrees.
    pub spread_deg: f32,
    /// Length of each strand, mm.
    pub strand: f32,
}

impl ChainSpec {
    /// A 1.6 mm cable chain, the usual pairing for a cast pendant.
    pub fn cable(scale: f32) -> ChainSpec {
        ChainSpec { wire: 0.45 * scale, link_len: 2.3 * scale, link_w: 1.6 * scale, spread_deg: 14.0, strand: 80.0 * scale }
    }
}

/// Append one link: a tube of radius `r` around a stadium centreline
/// (half-width `a`, straight length `l`) in the plane spanned by `u`
/// (along the chain) and `v`, centred at `c`.
fn link(m: &mut Mesh, c: V3, u: V3, v: V3, a: f32, l: f32, r: f32) {
    let w = u.cross(v).normalize();
    let (n_path, n_ring) = (40u32, 10u32);
    let base = m.positions.len() as u32;
    // Centreline parameterized by arc length around the stadium.
    let half_circ = std::f32::consts::PI * a;
    let total = 2.0 * l + 2.0 * half_circ;
    for i in 0..n_path {
        let s = i as f32 / n_path as f32 * total;
        let (p, t) = if s < l {
            (V3::new(-l / 2.0 + s, a, 0.0), V3::X)
        } else if s < l + half_circ {
            let th = (s - l) / a;
            (V3::new(l / 2.0 + a * th.sin(), a * th.cos(), 0.0), V3::new(th.cos(), -th.sin(), 0.0))
        } else if s < 2.0 * l + half_circ {
            (V3::new(l / 2.0 - (s - l - half_circ), -a, 0.0), -V3::X)
        } else {
            let th = (s - 2.0 * l - half_circ) / a;
            (V3::new(-l / 2.0 - a * th.sin(), -a * th.cos(), 0.0), V3::new(-th.cos(), th.sin(), 0.0))
        };
        // Local frame: n points away from the stadium centre in its plane.
        let n_in = V3::new(-t.y, t.x, 0.0);
        let to_w = |q: V3| u * q.x + v * q.y + w * q.z;
        let pc = c + to_w(p);
        let nn = to_w(-n_in);
        for j in 0..n_ring {
            let ph = j as f32 / n_ring as f32 * std::f32::consts::TAU;
            let dir = nn * ph.cos() + w * ph.sin();
            m.positions.push((pc + dir * r).into());
            m.normals.push(dir.into());
        }
    }
    for i in 0..n_path {
        for j in 0..n_ring {
            let i1 = (i + 1) % n_path;
            let j1 = (j + 1) % n_ring;
            let p00 = base + i * n_ring + j;
            let p10 = base + i1 * n_ring + j;
            let p11 = base + i1 * n_ring + j1;
            let p01 = base + i * n_ring + j1;
            m.triangles.push([p00, p10, p11]);
            m.triangles.push([p00, p11, p01]);
        }
    }
}

/// Two strands of chain hanging from `anchor` (the bail hole), rising in a
/// V in the XY plane. Returns a mesh with one part named `chain`.
pub fn hang(anchor: V3, spec: &ChainSpec) -> Mesh {
    let mut m = Mesh::default();
    let r = spec.wire * 0.5;
    let a = (spec.link_w - spec.wire) * 0.5;
    let l = (spec.link_len - spec.link_w).max(0.0);
    // Consecutive links interlock: centres one inner length apart.
    let pitch = spec.link_len - 2.0 * spec.wire;
    for side in [-1.0f32, 1.0] {
        let ang = spec.spread_deg.to_radians() * side;
        let dir = V3::new(ang.sin(), ang.cos(), 0.0);
        let side_axis = V3::Z.cross(dir).normalize();
        let n = (spec.strand / pitch).ceil() as usize;
        for k in 0..n {
            // The first link of each side straddles the bail; offset half a pitch.
            let c = anchor + dir * (pitch * (k as f32 + 0.5));
            let v = if k % 2 == 0 { side_axis } else { V3::Z };
            link(&mut m, c, dir, v, a, l, r);
        }
    }
    let nt = m.triangles.len() as u32;
    m.parts = vec![Part { name: "chain".into(), triangles: 0..nt, material: None }];
    m
}
