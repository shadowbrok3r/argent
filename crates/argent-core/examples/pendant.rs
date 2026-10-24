//! Dev: a procedural medallion with a bail, hung on a cable chain.
//! `cargo run --release --example pendant -- out.png [metal] [studio] [spp]`

use argent_core::camera::Lens;
use argent_core::chain::ChainSpec;
use argent_core::geom::V3;
use argent_core::shot::{grade, render_image, Look, Prepared, Quality};
use argent_core::studio::Studio;
use argent_core::views::View;
use argent_mesh::{Mesh, Part, Pose};

fn medallion() -> Mesh {
    // Disc of radius 12 mm in the XY plane, 3 mm thick, with a relief of
    // concentric rings and rays on the front (+Z), plus a torus bail on top.
    let mut m = Mesh::default();
    let (nr, nt) = (160u32, 360u32);
    let rad = 12.0f32;
    let front = |r: f32, th: f32| -> f32 {
        let rel = 0.5 * (r * 1.6).sin().powi(2) * (r / rad) + 0.35 * ((th * 12.0).cos().max(0.0)).powf(4.0) * (1.0 - r / rad);
        1.5 + rel * (1.0 - (r / rad).powf(8.0))
    };
    // Front and back grids (polar), joined at the rim.
    for side in [1.0f32, -1.0] {
        for i in 0..=nr {
            let r = rad * i as f32 / nr as f32;
            for j in 0..nt {
                let th = j as f32 / nt as f32 * std::f32::consts::TAU;
                let z = if side > 0.0 { front(r, th) } else { -1.5 };
                m.positions.push([r * th.cos(), r * th.sin(), z]);
            }
        }
    }
    let row = nt;
    let grid = (nr + 1) * nt;
    for s in 0..2u32 {
        let o = s * grid;
        for i in 0..nr {
            for j in 0..nt {
                let a = o + i * row + j;
                let b = o + i * row + (j + 1) % nt;
                let c = o + (i + 1) * row + (j + 1) % nt;
                let d = o + (i + 1) * row + j;
                if s == 0 {
                    m.triangles.push([a, d, c]);
                    m.triangles.push([a, c, b]);
                } else {
                    m.triangles.push([a, b, c]);
                    m.triangles.push([a, c, d]);
                }
            }
        }
    }
    // Rim: connect outer rows.
    for j in 0..nt {
        let f0 = nr * row + j;
        let f1 = nr * row + (j + 1) % nt;
        let b0 = grid + nr * row + j;
        let b1 = grid + nr * row + (j + 1) % nt;
        m.triangles.push([f0, f1, b1]);
        m.triangles.push([f0, b1, b0]);
    }
    // Bail: torus around Z at the top.
    let base = m.positions.len() as u32;
    let (cu, cv) = (64u32, 24u32);
    let (big, small) = (2.6f32, 0.9f32);
    let cy = rad + big - 0.4;
    for i in 0..cu {
        let u = i as f32 / cu as f32 * std::f32::consts::TAU;
        for j in 0..cv {
            let v = j as f32 / cv as f32 * std::f32::consts::TAU;
            let rr = big + small * v.cos();
            m.positions.push([rr * u.cos(), cy + rr * u.sin(), small * v.sin()]);
        }
    }
    for i in 0..cu {
        for j in 0..cv {
            let a = base + i * cv + j;
            let b = base + ((i + 1) % cu) * cv + j;
            let c = base + ((i + 1) % cu) * cv + (j + 1) % cv;
            let d = base + i * cv + (j + 1) % cv;
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
    m.normals = n.iter().map(|v| v.normalize_or_zero().into()).collect();
    let tn = m.triangles.len() as u32;
    m.parts = vec![Part { name: "medallion".into(), triangles: 0..tn, material: None }];
    m
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let out = a.get(1).cloned().unwrap_or("pendant.png".into());
    let metal = a.get(2).cloned().unwrap_or("sterling".into());
    let studio = a.get(3).and_then(|s| Studio::from_name(s)).unwrap_or(Studio::Noir);
    let spp: u32 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(64);
    let mesh = medallion();
    let p = Prepared::build(mesh, Some(Pose::IDENTITY), Some(ChainSpec::cable(1.0)), true);
    let look = Look { metal, studio, ..Default::default() };
    let scene = p.scene(&look);
    let mut v = View::named("front").unwrap();
    v.azimuth_deg = 18.0;
    v.elevation_deg = 10.0;
    let cam = p.camera(&v, &Lens::default(), 540.0 / 960.0);
    let q = Quality { width: 540, height: 960, spp, ..Default::default() };
    let img = render_image(&scene, &cam, &q, 1)?;
    argent_post::io::write_png(std::path::Path::new(&out), &argent_post::develop(&img, &grade(&look)))?;
    println!("wrote {out} ({} tris)", p.mesh.triangles.len());
    Ok(())
}
