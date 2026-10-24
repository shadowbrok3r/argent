//! White-furnace checks: a convex object in a uniform environment. Every
//! reflected ray escapes straight to the environment, so a diffuse sphere of
//! albedo ρ must render exactly ρ × the environment, and the environment
//! itself (RgbIlluminant white) must come out neutral white. This exercises
//! wavelength sampling, the CIE/D65 normalization, RGB upsampling and the
//! integrator's bookkeeping at once.

use argent_core::camera::{Camera, Lens};
use argent_core::geom::V3;
use argent_core::integrator::PathSettings;
use argent_core::light::Env;
use argent_core::material::Material;
use argent_core::render::{render_pass, Film};
use argent_core::scene::{Piece, Scene};
use argent_mesh::{Mesh, Part};
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
    m.parts = vec![Part { name: "sphere".into(), triangles: 0..nt, material: None }];
    m
}

fn furnace(mat: Material) -> ([f32; 3], [f32; 3]) {
    let mesh = sphere(10.0, 64);
    let piece = Arc::new(Piece::new(&mesh, vec![0; mesh.triangles.len()]));
    let white: Arc<dyn Spectrum> = Arc::new(RgbIlluminant::new([1.0, 1.0, 1.0]));
    let env = Env::Gradient { top: white.clone(), horizon: white.clone(), bottom: white, power: 1.0 };
    let scene = Scene::new(piece, vec![mat], vec![], env, None);
    let cam = Camera::look_at(V3::new(0.0, 0.0, 200.0), V3::ZERO, V3::Y, &Lens { focal_mm: 100.0, f_stop: 0.0 }, 1.0);
    let mut film = Film::new(64, 64);
    render_pass(&scene, &cam, &mut film, &PathSettings::default(), 256, 1);
    let img = film.color();
    let at = |x: u32, y: u32| img.get(x, y);
    (at(32, 32), at(1, 1))
}

#[test]
fn white_environment_is_white() {
    let (_, bg) = furnace(Material::diffuse([0.8, 0.8, 0.8]));
    for c in bg {
        assert!((c - 1.0).abs() < 0.03, "background {bg:?}");
    }
}

#[test]
fn grey_diffuse_sphere_reflects_its_albedo() {
    let (center, _) = furnace(Material::diffuse([0.5, 0.5, 0.5]));
    for c in center {
        assert!((c - 0.5).abs() < 0.04, "centre {center:?}");
    }
}
