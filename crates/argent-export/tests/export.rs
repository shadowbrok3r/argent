//! Simplifier and GLB writer on synthetic meshes.

mod common;

use argent_export::{GlbOptions, PartKind, carry_scalars, classify_part, crease_normals, export_glb_bytes, simplify};
use argent_mesh::{Mesh, Part};
use common::*;

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[test]
fn sphere_reaches_target_closed_round_and_outward() {
    let m = uv_sphere(5.0, 256, 128); // 65,024 triangles
    let (s, map) = simplify(&m, 2_000);
    assert!(s.triangles.len() <= 2_000 && s.triangles.len() >= 1_990, "{}", s.triangles.len());
    assert!(closed_and_oriented(&s), "simplified sphere is closed and consistently wound");
    for p in &s.positions {
        let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        assert!((r - 5.0).abs() < 0.05, "vertex off the sphere: r = {r}");
    }
    for t in &s.triangles {
        let (n, c) = face_normal(&s, *t);
        assert!(dot(n, c) > 0.0, "inward (folded) triangle");
    }
    let v = signed_volume(&s);
    let exact = 4.0 / 3.0 * std::f64::consts::PI * 125.0;
    assert!((v - exact).abs() / exact < 0.01, "volume {v} vs {exact}");
    assert_eq!(map.len(), m.positions.len());
    assert!(map.iter().all(|&i| (i as usize) < s.positions.len()));
    assert_eq!(s.normals.len(), s.positions.len());
    assert_eq!(s.parts.len(), 1);
    assert_eq!(s.parts[0].triangles, 0..s.triangles.len() as u32);
}

#[test]
fn torus_never_folds() {
    // Negative curvature on the inside of the tube is where flips happen.
    let (big_r, r) = (8.0f64, 2.0f64);
    let m = torus(big_r as f32, r as f32, 400, 100); // 80,000 triangles
    for target in [20_000u32, 3_000, 600] {
        let (s, _) = simplify(&m, target);
        assert!(s.triangles.len() <= target as usize);
        assert!(closed_and_oriented(&s));
        for t in &s.triangles {
            let (n, c) = face_normal(&s, *t);
            let rho = (c[0] * c[0] + c[1] * c[1]).sqrt();
            let core = [big_r * c[0] / rho, big_r * c[1] / rho, 0.0];
            let out = [c[0] - core[0], c[1] - core[1], c[2]];
            assert!(dot(n, out) > 0.0, "folded triangle at target {target}");
        }
        // Genus is kept: V - E + F = 0 for a torus.
        let f = s.triangles.len() as i64;
        let v = s.positions.len() as i64;
        assert_eq!(v - 3 * f / 2 + f, 0, "Euler characteristic at target {target}");
    }
}

#[test]
fn boundary_stays_on_the_rim() {
    let (w, h) = (20.0f32, 10.0f32);
    let m = plate(w, h, 160, 80, 1.5); // 25,600 triangles, open
    let (s, _) = simplify(&m, 1_000);
    assert!(s.triangles.len() <= 1_000);
    // Boundary edges of the result (one incident triangle) lie on the rim.
    let mut edges = std::collections::HashMap::new();
    for t in &s.triangles {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    // Weighted boundary planes hold the rim to a few µm (measured: < 0.4 µm
    // in plane, < 4 µm in z, on a 20 mm plate).
    let on_rim = |p: [f32; 3]| {
        let e = 2e-3;
        (p[0].abs() < e || (p[0] - w).abs() < e || p[1].abs() < e || (p[1] - h).abs() < e) && p[2].abs() < 1e-2
    };
    let mut boundary = 0;
    for ((a, b), n) in edges {
        assert!(n <= 2);
        if n == 1 {
            boundary += 1;
            assert!(on_rim(s.positions[a as usize]) && on_rim(s.positions[b as usize]), "boundary vertex left the rim");
        }
    }
    assert!(boundary >= 4);
    // All four corners survive.
    for c in [[0.0, 0.0], [w, 0.0], [0.0, h], [w, h]] {
        assert!(
            s.positions.iter().any(|p| (p[0] - c[0]).abs() < 2e-3 && (p[1] - c[1]).abs() < 2e-3 && p[2].abs() < 1e-2),
            "corner {c:?} lost"
        );
    }
    // Projected area is the rectangle's (to the µm drift of the rim).
    let area: f64 = s.triangles.iter().map(|t| face_normal(&s, *t).0[2] * 0.5).sum();
    assert!((area / (w * h) as f64 - 1.0).abs() < 1e-4, "area {area}");
}

fn ring_with_stone() -> Mesh {
    let mut ring = torus(9.0, 1.5, 600, 120); // 144,000 triangles of metal
    ring.parts[0] = Part { name: "band".into(), triangles: 0..ring.triangles.len() as u32, material: None };
    let mut stone = uv_sphere(1.2, 24, 12); // 528 triangles
    translate(&mut stone, [0.0, 10.5, 0.0]);
    stone.parts[0] =
        Part { name: "stone_00".into(), triangles: 0..stone.triangles.len() as u32, material: Some("gem".into()) };
    ring.merge(stone);
    ring
}

#[test]
fn parts_stay_apart_and_small_stones_are_kept() {
    let m = ring_with_stone();
    let (s, map) = simplify(&m, 10_000);
    assert_eq!(s.parts.len(), 2);
    assert_eq!(s.parts[0].name, "band");
    assert_eq!(s.parts[1].name, "stone_00");
    assert_eq!(s.parts[1].material.as_deref(), Some("gem"));
    assert_eq!(s.parts[0].triangles.start, 0);
    assert_eq!(s.parts[0].triangles.end, s.parts[1].triangles.start);
    assert_eq!(s.parts[1].triangles.end as usize, s.triangles.len());
    // The stone is untouched; the metal took the cut.
    assert_eq!(s.parts[1].triangles.len(), 528);
    assert!(s.triangles.len() <= 10_000);
    // No vertex is shared between the parts.
    let verts = |r: std::ops::Range<u32>| {
        let mut v: Vec<u32> = s.triangles[r.start as usize..r.end as usize].iter().flatten().copied().collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let (a, b) = (verts(s.parts[0].triangles.clone()), verts(s.parts[1].triangles.clone()));
    assert!(a.last().unwrap() < b.first().unwrap(), "parts own contiguous, disjoint vertex runs");
    assert_eq!(map.len(), m.positions.len());
}

#[test]
fn scalars_follow_the_collapses() {
    let m = uv_sphere(5.0, 200, 100);
    // A linear field: averaging nearby vertices keeps it close to linear.
    let f = |p: [f32; 3]| 0.5 + 0.08 * p[0] + 0.02 * p[2];
    let values: Vec<f32> = m.positions.iter().map(|p| f(*p)).collect();
    let (s, map) = simplify(&m, 1_500);
    let carried = carry_scalars(&map, &values, s.positions.len());
    assert_eq!(carried.len(), s.positions.len());
    let worst = s.positions.iter().zip(&carried).map(|(p, c)| (f(*p) - c).abs()).fold(0.0f32, f32::max);
    assert!(worst < 0.06, "carried value strays {worst} from the field");
    // Every output vertex received at least one input value.
    let mut hit = vec![false; s.positions.len()];
    for &o in &map {
        hit[o as usize] = true;
    }
    assert!(hit.iter().all(|h| *h));
}

#[test]
fn stl_soup_welds_like_the_indexed_mesh() {
    let m = torus(8.0, 2.0, 200, 50);
    let (a, _) = simplify(&m, 2_000);
    let soup = unweld(&m);
    let (b, map) = simplify(&soup, 2_000);
    assert_eq!(a.triangles.len(), b.triangles.len());
    assert_eq!(a.positions.len(), b.positions.len());
    assert!(closed_and_oriented(&b));
    assert_eq!(map.len(), soup.positions.len());
}

#[test]
fn crease_normals_split_a_cube() {
    let (c, src) = crease_normals(&cube(2.0), 50.0);
    assert_eq!(c.positions.len(), 24, "each corner splits into three facet copies");
    assert_eq!(src.len(), 24);
    for n in &c.normals {
        let axis = n.iter().filter(|x| (x.abs() - 1.0).abs() < 1e-6).count();
        assert_eq!(axis, 1, "{n:?}");
    }
    let (smooth, _) = crease_normals(&cube(2.0), 120.0);
    assert_eq!(smooth.positions.len(), 8);
}

#[test]
fn classification_guards_the_main_body() {
    // A casting named after its stone is still metal when it is the bulk.
    let mut m = ring_with_stone();
    m.parts[0].name = "ophidian-amethyst-serpent_size8.6".into();
    let opts = GlbOptions { target_triangles: 20_000, ..Default::default() };
    let (bytes, _) = export_glb_bytes(&m, &opts).unwrap();
    let g = check_glb(&bytes);
    let mats = g.json["materials"].as_array().unwrap();
    let metal = mats.iter().find(|x| x["name"] == "metal").expect("metal material");
    assert_eq!(metal["pbrMetallicRoughness"]["metallicFactor"], 1.0);
    assert!(matches!(classify_part("ophidian-amethyst-serpent", None), PartKind::Gem(_)));
}

#[test]
fn glb_round_trip() {
    let m = ring_with_stone();
    let mask: Vec<f32> = m.positions.iter().map(|p| ((p[2] + 1.5) / 3.0).clamp(0.0, 1.0)).collect();
    let opts = GlbOptions {
        target_triangles: 20_000,
        max_bytes: Some(15_000_000),
        base_color: [0.95, 0.64, 0.54],
        roughness: 0.3,
        patina: Some((mask, [0.04, 0.035, 0.03], 0.8)),
        name: "test-ring".into(),
    };
    let (bytes, rep) = export_glb_bytes(&m, &opts).unwrap();
    assert_eq!(rep.bytes as usize, bytes.len());
    assert_eq!(rep.triangles_in, m.triangles.len());
    assert!(rep.triangles_out <= 20_000);
    let g = check_glb(&bytes);
    let j = &g.json;
    assert_eq!(g.triangles, rep.triangles_out);
    assert_eq!(j["scenes"][0]["name"], "test-ring");
    assert_eq!(j["nodes"][0]["name"], "test-ring");
    assert_eq!(j["nodes"][0]["children"].as_array().unwrap().len(), 2);
    let meshes = j["meshes"].as_array().unwrap();
    assert_eq!(meshes.len(), 2, "one mesh per part");
    assert_eq!(meshes[0]["name"], "band");
    assert_eq!(meshes[1]["name"], "stone_00");
    // Metal: PBR metal with vertex-colour patina.
    let band = &meshes[0]["primitives"][0];
    assert!(band["attributes"]["COLOR_0"].is_u64());
    let metal = &j["materials"][band["material"].as_u64().unwrap() as usize];
    assert_eq!(metal["pbrMetallicRoughness"]["metallicFactor"], 1.0);
    assert!((metal["pbrMetallicRoughness"]["roughnessFactor"].as_f64().unwrap() - 0.3).abs() < 1e-6);
    // Stone: glass-like, no patina.
    let stone = &meshes[1]["primitives"][0];
    assert!(stone["attributes"]["COLOR_0"].is_null());
    let gem = &j["materials"][stone["material"].as_u64().unwrap() as usize];
    assert_eq!(gem["pbrMetallicRoughness"]["metallicFactor"], 0.0);
    assert_eq!(gem["extensions"]["KHR_materials_transmission"]["transmissionFactor"], 1.0);
    assert!((gem["extensions"]["KHR_materials_ior"]["ior"].as_f64().unwrap() - 2.42).abs() < 1e-6);
    let used: Vec<&str> = j["extensionsUsed"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(used.contains(&"KHR_materials_transmission") && used.contains(&"KHR_materials_ior"));
    // Metres: the ring is ~21 mm across.
    let pa = band["attributes"]["POSITION"].as_u64().unwrap() as usize;
    let (lo, hi) = (&j["accessors"][pa]["min"], &j["accessors"][pa]["max"]);
    let width = hi[0].as_f64().unwrap() - lo[0].as_f64().unwrap();
    assert!((width - 0.021).abs() < 2e-4, "width {width} m");
    // The patina darkens: some vertices get a strong multiplier, some none.
    let colors = f32s(&g.bin, j, band["attributes"]["COLOR_0"].as_u64().unwrap() as usize);
    let lo_c = colors.iter().copied().fold(1.0f32, f32::min);
    let hi_c = colors.iter().copied().fold(0.0f32, f32::max);
    assert!(lo_c < 0.4 && hi_c > 0.99, "colour range {lo_c}..{hi_c}");
    assert!(rep.max_deviation_mm < 0.05, "deviation {}", rep.max_deviation_mm);
}

#[test]
fn max_bytes_shrinks_the_mesh() {
    let m = uv_sphere(5.0, 256, 128);
    let free = GlbOptions { target_triangles: 40_000, max_bytes: None, ..Default::default() };
    let (big, r0) = export_glb_bytes(&m, &free).unwrap();
    let capped = GlbOptions { max_bytes: Some(200_000), ..free.clone() };
    let (small, r1) = export_glb_bytes(&m, &capped).unwrap();
    assert!(big.len() > 200_000);
    assert!(small.len() <= 200_000, "{} bytes", small.len());
    assert!(r1.triangles_out < r0.triangles_out);
    check_glb(&small);
    let impossible = GlbOptions { max_bytes: Some(300), ..free };
    assert!(export_glb_bytes(&m, &impossible).is_err());
}

#[test]
fn no_reduction_means_no_deviation() {
    let m = torus(8.0, 2.0, 60, 20);
    let opts = GlbOptions { target_triangles: 1_000_000, ..Default::default() };
    let (bytes, rep) = export_glb_bytes(&m, &opts).unwrap();
    assert_eq!(rep.triangles_out, m.triangles.len());
    assert!(rep.max_deviation_mm < 1e-5, "{}", rep.max_deviation_mm);
    check_glb(&bytes);
}

#[test]
fn rejects_bad_input() {
    let m = uv_sphere(1.0, 16, 8);
    let opts = GlbOptions { patina: Some((vec![0.0; 3], [0.0; 3], 1.0)), ..Default::default() };
    assert!(export_glb_bytes(&m, &opts).is_err(), "mask length must match the vertices");
    assert!(export_glb_bytes(&Mesh::default(), &GlbOptions::default()).is_err());
}
