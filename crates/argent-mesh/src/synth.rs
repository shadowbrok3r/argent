//! Synthetic meshes (millimetres, outward winding, welded, no normals) for
//! tests and benchmarks: boxes, spheres, tori, cylinders, plus rigid
//! transforms and "STL-style" unwelding.

use crate::{Mesh, Part};
use std::f32::consts::TAU;

/// Axis-aligned box centred at the origin.
pub fn cuboid(size: [f32; 3]) -> Mesh {
    let [hx, hy, hz] = [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5];
    let positions = vec![
        [-hx, -hy, -hz],
        [hx, -hy, -hz],
        [hx, hy, -hz],
        [-hx, hy, -hz],
        [-hx, -hy, hz],
        [hx, -hy, hz],
        [hx, hy, hz],
        [-hx, hy, hz],
    ];
    let triangles = vec![
        [0, 2, 1],
        [0, 3, 2], // -z
        [4, 5, 6],
        [4, 6, 7], // +z
        [0, 1, 5],
        [0, 5, 4], // -y
        [3, 7, 6],
        [3, 6, 2], // +y
        [0, 4, 7],
        [0, 7, 3], // -x
        [1, 2, 6],
        [1, 6, 5], // +x
    ];
    finish(positions, triangles)
}

/// Box subdivided into `n × n` quads per face (shared edges welded).
pub fn subdivided_cuboid(size: [f32; 3], n: usize) -> Mesh {
    let n = n.max(1);
    let mut out = Mesh::default();
    let h = [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5];
    // For each face: normal axis, sign; build a grid in the other two axes.
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
            let mut m = Mesh::default();
            for i in 0..=n {
                for j in 0..=n {
                    let mut p = [0.0f32; 3];
                    p[axis] = sign * h[axis];
                    p[a] = -h[a] + size[a] * i as f32 / n as f32;
                    p[b] = -h[b] + size[b] * j as f32 / n as f32;
                    m.positions.push(p);
                }
            }
            let id = |i: usize, j: usize| (i * (n + 1) + j) as u32;
            for i in 0..n {
                for j in 0..n {
                    // (a, b, axis) is right-handed, so a×b = +axis.
                    if sign > 0.0 {
                        m.triangles.push([id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
                        m.triangles.push([id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
                    } else {
                        m.triangles.push([id(i, j), id(i + 1, j + 1), id(i + 1, j)]);
                        m.triangles.push([id(i, j), id(i, j + 1), id(i + 1, j + 1)]);
                    }
                }
            }
            out.merge(m);
        }
    }
    out.parts.clear();
    crate::weld::weld(&mut out, 1e-6);
    let n = out.triangles.len() as u32;
    out.parts = vec![Part { name: "box".into(), triangles: 0..n, material: None }];
    out
}

/// Icosphere of radius `r` with `subdiv` loop subdivisions (20·4^subdiv triangles).
pub fn icosphere(r: f32, subdiv: u32) -> Mesh {
    let t = (1.0 + 5.0f32.sqrt()) / 2.0;
    let mut p: Vec<[f32; 3]> = vec![
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ];
    let mut tris: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..subdiv {
        let mut mid = std::collections::HashMap::<(u32, u32), u32>::new();
        let mut next = Vec::with_capacity(tris.len() * 4);
        let mut midpoint = |a: u32, b: u32, p: &mut Vec<[f32; 3]>| -> u32 {
            let key = (a.min(b), a.max(b));
            *mid.entry(key).or_insert_with(|| {
                let (x, y) = (p[a as usize], p[b as usize]);
                p.push([(x[0] + y[0]) * 0.5, (x[1] + y[1]) * 0.5, (x[2] + y[2]) * 0.5]);
                (p.len() - 1) as u32
            })
        };
        for &[a, b, c] in &tris {
            let ab = midpoint(a, b, &mut p);
            let bc = midpoint(b, c, &mut p);
            let ca = midpoint(c, a, &mut p);
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        tris = next;
    }
    for q in &mut p {
        let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
        *q = [q[0] / l * r, q[1] / l * r, q[2] / l * r];
    }
    finish(p, tris)
}

/// Torus around the Z axis: `big_r` to the tube centre, tube radius `r`;
/// `nu` segments around Z, `nv` around the tube.
pub fn torus(big_r: f32, r: f32, nu: usize, nv: usize) -> Mesh {
    torus_profile(nu, nv, |v| (big_r + r * v.cos(), r * v.sin()))
}

/// Surface of revolution around Z from a closed profile `v ∈ [0, 2π) ->
/// (radius, z)` traversed counter-clockwise in the (radius, z) plane.
pub fn torus_profile(nu: usize, nv: usize, profile: impl Fn(f32) -> (f32, f32)) -> Mesh {
    let mut positions = Vec::with_capacity(nu * nv);
    for i in 0..nu {
        let u = TAU * i as f32 / nu as f32;
        for j in 0..nv {
            let v = TAU * j as f32 / nv as f32;
            let (rad, z) = profile(v);
            positions.push([rad * u.cos(), rad * u.sin(), z]);
        }
    }
    let id = |i: usize, j: usize| ((i % nu) * nv + (j % nv)) as u32;
    let mut triangles = Vec::with_capacity(nu * nv * 2);
    for i in 0..nu {
        for j in 0..nv {
            triangles.push([id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
            triangles.push([id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
        }
    }
    finish(positions, triangles)
}

/// Closed cylinder along Z, centred, with fan caps.
pub fn cylinder(r: f32, h: f32, n: usize) -> Mesh {
    let mut p = Vec::with_capacity(2 * n + 2);
    for i in 0..n {
        let a = TAU * i as f32 / n as f32;
        p.push([r * a.cos(), r * a.sin(), -h * 0.5]);
    }
    for i in 0..n {
        let a = TAU * i as f32 / n as f32;
        p.push([r * a.cos(), r * a.sin(), h * 0.5]);
    }
    let cb = p.len() as u32;
    p.push([0.0, 0.0, -h * 0.5]);
    let ct = p.len() as u32;
    p.push([0.0, 0.0, h * 0.5]);
    let mut t = Vec::new();
    let n32 = n as u32;
    for i in 0..n32 {
        let j = (i + 1) % n32;
        t.push([i, j, n32 + j]);
        t.push([i, n32 + j, n32 + i]);
        t.push([cb, j, i]);
        t.push([ct, n32 + i, n32 + j]);
    }
    finish(p, t)
}

/// Row-major rotation from an axis and angle (radians).
pub fn rotation(axis: [f32; 3], angle: f32) -> [[f32; 3]; 3] {
    let l = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    let [x, y, z] = [axis[0] / l, axis[1] / l, axis[2] / l];
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ]
}

/// Multiply row-major 3×3 matrices.
pub fn mat_mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut m = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    m
}

/// Rotate (row-major `rot`) then translate every vertex and normal.
pub fn transform(mesh: &mut Mesh, rot: [[f32; 3]; 3], translate: [f32; 3]) {
    let ap = |v: [f32; 3]| {
        [
            rot[0][0] * v[0] + rot[0][1] * v[1] + rot[0][2] * v[2],
            rot[1][0] * v[0] + rot[1][1] * v[1] + rot[1][2] * v[2],
            rot[2][0] * v[0] + rot[2][1] * v[1] + rot[2][2] * v[2],
        ]
    };
    for p in &mut mesh.positions {
        let q = ap(*p);
        *p = [q[0] + translate[0], q[1] + translate[1], q[2] + translate[2]];
    }
    for n in &mut mesh.normals {
        *n = ap(*n);
    }
}

/// Translate every vertex.
pub fn translate(mesh: &mut Mesh, t: [f32; 3]) {
    transform(mesh, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t);
}

/// Scale about the origin per axis.
pub fn scale3(mesh: &mut Mesh, s: [f32; 3]) {
    for p in &mut mesh.positions {
        *p = [p[0] * s[0], p[1] * s[1], p[2] * s[2]];
    }
    mesh.normals.clear();
}

/// Expand to a triangle soup (3 private vertices per triangle), like STL.
pub fn unweld(mesh: &mut Mesh) {
    let mut p = Vec::with_capacity(mesh.triangles.len() * 3);
    for (i, t) in mesh.triangles.iter_mut().enumerate() {
        for k in 0..3 {
            p.push(mesh.positions[t[k] as usize]);
        }
        let b = (3 * i) as u32;
        *t = [b, b + 1, b + 2];
    }
    mesh.positions = p;
    mesh.normals.clear();
}

/// Combine meshes into one (parts named `name`).
pub fn combine(meshes: Vec<Mesh>, name: &str) -> Mesh {
    let mut out = Mesh::default();
    for m in meshes {
        out.merge(m);
    }
    let n = out.triangles.len() as u32;
    out.parts = vec![Part { name: name.into(), triangles: 0..n, material: None }];
    out
}

/// Serialize as binary STL bytes (header starts with "solid" on purpose:
/// many exporters do that, and readers must not be fooled).
pub fn to_binary_stl(mesh: &Mesh) -> Vec<u8> {
    let mut out = Vec::with_capacity(84 + 50 * mesh.triangles.len());
    let mut header = [b' '; 80];
    header[..21].copy_from_slice(b"solid argent-synth   ");
    out.extend_from_slice(&header);
    out.extend_from_slice(&(mesh.triangles.len() as u32).to_le_bytes());
    for t in &mesh.triangles {
        let (n, _) = crate::util::tri_normal_area(&mesh.positions, *t);
        for v in n {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for &i in t {
            for v in mesh.positions[i as usize] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out.extend_from_slice(&[0, 0]);
    }
    out
}

/// Signed enclosed volume (positive for outward winding), mm³.
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

fn finish(positions: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>) -> Mesh {
    let n = triangles.len() as u32;
    Mesh { positions, normals: vec![], triangles, parts: vec![Part { name: "synth".into(), triangles: 0..n, material: None }] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volumes_are_positive_and_right() {
        let c = cuboid([1.0, 2.0, 3.0]);
        assert!((signed_volume(&c) - 6.0).abs() < 1e-5);
        let s = icosphere(1.0, 4);
        let v = signed_volume(&s);
        assert!((v - 4.18879).abs() < 0.02, "{v}");
        let t = torus(10.0, 1.0, 256, 64);
        let v = signed_volume(&t);
        let exact = 2.0 * std::f64::consts::PI.powi(2) * 10.0;
        assert!((v - exact).abs() / exact < 0.01, "{v} vs {exact}");
        let cy = cylinder(2.0, 3.0, 128);
        assert!(signed_volume(&cy) > 0.0);
        let sc = subdivided_cuboid([1.0, 2.0, 3.0], 4);
        assert!((signed_volume(&sc) - 6.0).abs() < 1e-4);
        assert_eq!(sc.triangles.len(), 6 * 4 * 4 * 2);
    }

}
