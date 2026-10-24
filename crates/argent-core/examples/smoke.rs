//! Render a procedural ring (band + domed head + cabochon) to check the
//! pipeline end to end: `cargo run --release --example smoke -- out.png`.

use argent_core::camera::Lens;
use argent_core::geom::V3;
use argent_core::integrator::PathSettings;
use argent_core::material::{default_patina, Material};
use argent_core::render::{render_pass, Film};
use argent_core::scene::{Piece, Scene};
use argent_core::studio::{floor_at, Studio};
use argent_core::views::{frame, View};
use argent_mesh::Mesh;
use argent_spectra::alloys::Alloy;
use std::time::Instant;

fn ring() -> Mesh {
    // Torus band around Z (finger axis), thickened toward +Y (the head),
    // with a ripple pattern so the patina has recesses to find.
    let (nu, nv) = (720u32, 160u32);
    let r_in = 9.0f32;
    let mut m = Mesh::default();
    for i in 0..nu {
        let u = i as f32 / nu as f32 * std::f32::consts::TAU;
        let head = (u.sin().max(0.0)).powf(3.0); // +Y when sin(u)=1
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
    // Normals by accumulation.
    let mut n = vec![V3::ZERO; m.positions.len()];
    for t in &m.triangles {
        let [a, b, c] = t.map(|i| V3::from(m.positions[i as usize]));
        let f = (b - a).cross(c - a);
        for &i in t {
            n[i as usize] += f;
        }
    }
    m.normals = n.iter().map(|v| (-v.normalize_or_zero()).into()).collect();
    // Flip winding to match outward normals.
    let outward = {
        let p = V3::from(m.positions[0]);
        let nn = V3::from(m.normals[0]);
        p.dot(nn) > 0.0
    };
    if !outward {
        for nn in &mut m.normals {
            *nn = (-V3::from(*nn)).into();
        }
    }
    let nt = m.triangles.len() as u32;
    m.parts = vec![argent_mesh::Part { name: "band".into(), triangles: 0..nt, material: None }];
    m
}

fn main() -> anyhow::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "smoke.png".into());
    let metal_arg = std::env::args().nth(2).unwrap_or_default();
    let alloy = Alloy::from_name(&metal_arg).unwrap_or(Alloy::Sterling);
    let studio = std::env::args().nth(3).and_then(|s| Studio::from_name(&s)).unwrap_or(Studio::Noir);
    let spp: u32 = std::env::args().nth(4).and_then(|s| s.parse().ok()).unwrap_or(64);
    let mesh = ring();
    let t0 = Instant::now();
    let tri_mat = vec![0u16; mesh.triangles.len()];
    let mut piece = Piece::new(&mesh, tri_mat);
    let (lo, hi) = piece.bounds();
    let radius = ((hi - lo) * 0.5).length();
    // Cavity mask from the mesh crate (stub returns zeros until implemented).
    let mask = argent_mesh::cavity::patina_mask(&mesh, &piece, &Default::default());
    piece.cavity = Some(mask);
    let rig = studio.build(radius);
    let mut materials = vec![if metal_arg.contains("opal") {
        Material::gem(&metal_arg).unwrap()
    } else {
        Material::metal(alloy, 0.07).with_patina(default_patina(0.85, alloy))
    }];
    let floor = rig.floor.map(|(m, a, b)| {
        materials.push(m);
        floor_at(lo.y, materials.len() - 1, (a, b), radius)
    });
    let scene = Scene::new(std::sync::Arc::new(piece), materials, rig.panels, rig.env, floor);
    println!("scene built in {:.2?}", t0.elapsed());

    let (w, h) = (540u32, 960u32);
    let pts: Vec<V3> = mesh.positions.iter().step_by(37).map(|&p| V3::from(p)).collect();
    let cam = frame(&pts, &View::named("hero").unwrap(), &Lens::default(), w as f32 / h as f32);
    let mut film = Film::new(w, h);
    let t1 = Instant::now();
    render_pass(&scene, &cam, &mut film, &PathSettings::default(), spp, 1);
    let dt = t1.elapsed();
    println!("{} spp at {}x{} in {:.2?} ({:.1} Msamples/s)", spp, w, h, dt, (w * h * spp) as f64 / dt.as_secs_f64() / 1e6);
    let img = film.color();
    let den = argent_post::denoise(
        argent_post::DenoiseInput { color: &img, albedo: Some(&film.albedo()), normal: Some(&film.normal()) },
        argent_post::DenoiseBackend::Auto,
    )?;
    let png = argent_post::develop(&den, &argent_post::Grade::default());
    argent_post::io::write_png(std::path::Path::new(&out), &png)?;
    println!("wrote {out}");
    Ok(())
}
