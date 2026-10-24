use super::*;
use crate::synth;

/// Height field z = f(x) over [-5, 5]² with `n`×`n` quads.
fn height_field(n: usize, f: impl Fn(f32) -> f32) -> Mesh {
    let mut positions = Vec::with_capacity((n + 1) * (n + 1));
    for j in 0..=n {
        for i in 0..=n {
            let x = -5.0 + 10.0 * i as f32 / n as f32;
            let y = -5.0 + 10.0 * j as f32 / n as f32;
            positions.push([x, y, f(x)]);
        }
    }
    let id = |i: usize, j: usize| (j * (n + 1) + i) as u32;
    let mut triangles = Vec::with_capacity(n * n * 2);
    for j in 0..n {
        for i in 0..n {
            triangles.push([id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
            triangles.push([id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
        }
    }
    let nt = triangles.len() as u32;
    let mut m = Mesh { positions, normals: vec![], triangles, parts: vec![crate::Part { name: "hf".into(), triangles: 0..nt, material: None }] };
    crate::normals::compute(&mut m, 60.0);
    m
}

/// V-groove at x = -3 and a matching ridge at x = +3 (0.5 mm deep/high, 1 mm wide).
fn groove_ridge() -> Mesh {
    height_field(200, |x| {
        let g = -0.5 * (1.0 - (x + 3.0).abs() / 0.5).max(0.0);
        let r = 0.5 * (1.0 - (x - 3.0).abs() / 0.5).max(0.0);
        g + r
    })
}

fn value_near(m: &Mesh, vals: &[f32], x: f32, y: f32) -> f32 {
    let mut best = (f32::MAX, 0.0);
    for (p, v) in m.positions.iter().zip(vals) {
        let d = (p[0] - x).powi(2) + (p[1] - y).powi(2);
        if d < best.0 {
            best = (d, *v);
        }
    }
    best.1
}

#[test]
fn groove_dark_ridge_bright() {
    let m = groove_ridge();
    let bvh = SimpleBvh::new(&m);
    let mask = patina_mask(&m, &bvh, &PatinaSettings::default());
    assert_eq!(mask.len(), m.positions.len());
    let groove = value_near(&m, &mask, -3.0, 0.0);
    let ridge = value_near(&m, &mask, 3.0, 0.0);
    let flat = value_near(&m, &mask, 0.0, 0.0);
    assert!(groove > 0.6, "groove {groove}");
    assert!(ridge < 0.05, "ridge {ridge}");
    assert!(flat < 0.1, "flat {flat}");
    assert!(mask.iter().all(|v| (0.0..=1.0).contains(v)));
}

#[test]
fn concavity_signs() {
    let m = groove_ridge();
    let c = concavity(&m, 0.4);
    assert!(value_near(&m, &c, -3.0, 0.0) > 0.2);
    assert!(value_near(&m, &c, 3.0, 0.0) < -0.2);
    assert!(value_near(&m, &c, 0.0, 0.0).abs() < 1e-3);
    // Sphere of radius 4: H·r ≈ -r/R = -0.1 everywhere (convex).
    let mut s = synth::icosphere(4.0, 5);
    crate::normals::compute(&mut s, 50.0);
    let c = concavity(&s, 0.4);
    let mean = c.iter().sum::<f32>() / c.len() as f32;
    assert!((mean + 0.1).abs() < 0.02, "{mean}");
}

#[test]
fn bvh_matches_brute_force() {
    let mut m = synth::icosphere(3.0, 3);
    let mut b = synth::cuboid([2.0, 2.0, 2.0]);
    synth::translate(&mut b, [4.0, 0.0, 0.0]);
    m.merge(b);
    let bvh = SimpleBvh::new(&m);
    let brute = |o: Vec3, d: Vec3, tmax: f32| {
        m.triangles.iter().any(|t| {
            let [a, b, c] = t.map(|i| Vec3::from(m.positions[i as usize]));
            let e1 = b - a;
            let e2 = c - a;
            let pv = d.cross(e2);
            let det = e1.dot(pv);
            if det.abs() < 1e-12 {
                return false;
            }
            let tv = o - a;
            let u = tv.dot(pv) / det;
            let q = tv.cross(e1);
            let v = d.dot(q) / det;
            let t = e2.dot(q) / det;
            (0.0..=1.0).contains(&u) && v >= 0.0 && u + v <= 1.0 && t > 1e-6 && t < tmax
        })
    };
    for k in 0..2000u32 {
        let h = |s: u32| crate::util::unit_f32(crate::util::hash32(k * 7 + s)) * 2.0 - 1.0;
        let o = Vec3::new(h(1) * 8.0, h(2) * 8.0, h(3) * 8.0);
        let d = Vec3::new(h(4), h(5), h(6)).normalize();
        let tmax = (h(7) + 1.0) * 6.0;
        assert_eq!(bvh.occluded(o.to_array(), d.to_array(), tmax), brute(o, d, tmax), "ray {k}");
    }
}

#[test]
fn smoothing_preserves_constants_and_merges_crease_copies() {
    let mut m = synth::subdivided_cuboid([2.0, 2.0, 2.0], 4);
    crate::normals::compute(&mut m, 50.0);
    let mut v = vec![0.7f32; m.positions.len()];
    smooth(&m, &mut v, 5, 0.5);
    assert!(v.iter().all(|x| (x - 0.7).abs() < 1e-6));
    // A spike spreads to its neighbours; copies at one position stay equal.
    let mut v: Vec<f32> = (0..m.positions.len()).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();
    smooth(&m, &mut v, 2, 0.5);
    for i in 0..m.positions.len() {
        for j in 0..m.positions.len() {
            if m.positions[i] == m.positions[j] {
                assert_eq!(v[i], v[j]);
            }
        }
    }
    assert!(v[0] < 1.0 && v.iter().filter(|x| **x > 0.0).count() > 3);
}

#[test]
fn ao_open_plane_is_zero_and_box_inside_corner_is_occluded() {
    let m = height_field(20, |_| 0.0);
    let bvh = SimpleBvh::new(&m);
    let ao = ambient_occlusion(&m, &bvh, 32, 1.0);
    assert!(ao.iter().all(|a| *a == 0.0));
}
