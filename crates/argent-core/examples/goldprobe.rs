//! Dev: linear colour of metal spheres in a uniform white environment
//! (normal-incidence centre pixel ≈ F0), to check alloy optics end to end.
use argent_core::camera::{Camera, Lens};
use argent_core::geom::V3;
use argent_core::integrator::PathSettings;
use argent_core::light::Env;
use argent_core::material::Material;
use argent_core::render::{render_pass, Film};
use argent_core::scene::{Piece, Scene};
use argent_mesh::{Mesh, Part};
use argent_spectra::alloys::Alloy;
use argent_spectra::{RgbIlluminant, Spectrum};
use std::sync::Arc;

fn sphere(r: f32, n: u32) -> Mesh {
    let mut m = Mesh::default();
    for i in 0..=n {
        let th = std::f32::consts::PI * i as f32 / n as f32;
        for j in 0..2 * n {
            let ph = std::f32::consts::PI * j as f32 / n as f32;
            let p = [r * th.sin() * ph.cos(), r * th.cos(), r * th.sin() * ph.sin()];
            m.positions.push(p);
            m.normals.push([p[0] / r, p[1] / r, p[2] / r]);
        }
    }
    let w = 2 * n;
    for i in 0..n {
        for j in 0..w {
            let a = i * w + j;
            let b = i * w + (j + 1) % w;
            let c = (i + 1) * w + (j + 1) % w;
            let d = (i + 1) * w + j;
            m.triangles.push([a, c, b]);
            m.triangles.push([a, d, c]);
        }
    }
    let nt = m.triangles.len() as u32;
    m.parts = vec![Part { name: "s".into(), triangles: 0..nt, material: None }];
    m
}

fn main() {
    let mesh = sphere(10.0, 64);
    for a in [Alloy::Sterling, Alloy::Gold24k, Alloy::Gold18kYellow, Alloy::Gold14kYellow, Alloy::Gold14kRose, Alloy::Bronze] {
        let piece = Arc::new(Piece::new(&mesh, vec![0; mesh.triangles.len()]));
        let white: Arc<dyn Spectrum> = Arc::new(RgbIlluminant::new([1.0, 1.0, 1.0]));
        let env = Env::Gradient { top: white.clone(), horizon: white.clone(), bottom: white, power: 1.0 };
        let scene = Scene::new(piece, vec![Material::metal(a, 0.02)], vec![], env, None);
        let cam = Camera::look_at(V3::new(0.0, 0.0, 200.0), V3::ZERO, V3::Y, &Lens { focal_mm: 100.0, f_stop: 0.0 }, 1.0);
        let mut film = Film::new(32, 32);
        render_pass(&scene, &cam, &mut film, &PathSettings::default(), 512, 1);
        let c = film.color().get(16, 16);
        let f0 = a.f0_srgb();
        println!("{:<22} centre {:.3} {:.3} {:.3}   f0_srgb {:.3} {:.3} {:.3}", a.display(), c[0], c[1], c[2], f0[0], f0[1], f0[2]);
    }
}
