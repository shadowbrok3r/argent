//! Film accumulation and parallel rendering passes.

use crate::camera::Camera;
use crate::integrator::{trace, Guides, PathSettings};
use crate::sampler::{hash32, tent, Sampler};
use crate::scene::Scene;
use argent_post::Image;
use argent_spectra::{cie, Wavelengths};
use rayon::prelude::*;

/// Running sums for a progressive render.
#[derive(Clone)]
pub struct Film {
    pub width: u32,
    pub height: u32,
    color: Vec<[f32; 3]>,
    albedo: Vec<[f32; 3]>,
    normal: Vec<[f32; 3]>,
    /// Samples accumulated per pixel (uniform across the film).
    pub samples: u32,
}

impl Film {
    pub fn new(width: u32, height: u32) -> Film {
        let n = (width * height) as usize;
        Film { width, height, color: vec![[0.0; 3]; n], albedo: vec![[0.0; 3]; n], normal: vec![[0.0; 3]; n], samples: 0 }
    }

    pub fn clear(&mut self) {
        for v in self.color.iter_mut().chain(self.albedo.iter_mut()).chain(self.normal.iter_mut()) {
            *v = [0.0; 3];
        }
        self.samples = 0;
    }

    fn average(&self, buf: &[[f32; 3]]) -> Image {
        let k = 1.0 / self.samples.max(1) as f32;
        Image { width: self.width, height: self.height, pixels: buf.par_iter().map(|p| [p[0] * k, p[1] * k, p[2] * k]).collect() }
    }

    pub fn color(&self) -> Image {
        self.average(&self.color)
    }
    pub fn albedo(&self) -> Image {
        self.average(&self.albedo)
    }
    pub fn normal(&self) -> Image {
        let k = 1.0 / self.samples.max(1) as f32;
        Image {
            width: self.width,
            height: self.height,
            pixels: self
                .normal
                .par_iter()
                .map(|n| {
                    let v = [n[0] * k, n[1] * k, n[2] * k];
                    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                    if l > 1e-6 { [v[0] / l, v[1] / l, v[2] / l] } else { [0.0, 0.0, 0.0] }
                })
                .collect(),
        }
    }
}

/// Render `count` more samples per pixel into `film`. `seed` decorrelates
/// frames of an animation.
pub fn render_pass(scene: &Scene, cam: &Camera, film: &mut Film, ps: &PathSettings, count: u32, seed: u32) {
    let w = film.width;
    let h = film.height;
    let first = film.samples;
    let aspect_sx = 2.0 / w as f32;
    let aspect_sy = 2.0 / h as f32;
    let seed_h = hash32(seed ^ 0xa511_e9b3);
    film.color
        .par_chunks_mut(w as usize)
        .zip(film.albedo.par_chunks_mut(w as usize))
        .zip(film.normal.par_chunks_mut(w as usize))
        .enumerate()
        .for_each(|(y, ((crow, arow), nrow))| {
            for x in 0..w as usize {
                let pix_seed = hash32((y as u32 * w + x as u32) ^ seed_h);
                let mut acc = [0.0f32; 3];
                let mut aacc = [0.0f32; 3];
                let mut nacc = [0.0f32; 3];
                for si in first..first + count {
                    let mut s = Sampler::new(pix_seed, si);
                    let (jx, jy) = (tent(s.next(), 1.0), tent(s.next(), 1.0));
                    let (lu, lv) = s.next2();
                    let mut wl = Wavelengths::sample_visible(s.next());
                    let sx = ((x as f32 + 0.5 + jx) * aspect_sx) - 1.0;
                    let sy = 1.0 - ((y as f32 + 0.5 + jy) * aspect_sy);
                    let ray = cam.ray(sx, sy, lu, lv);
                    let mut g = Guides::default();
                    let l = trace(scene, ray, &mut wl, &mut s, ps, &mut g);
                    let rgb = cie::xyz_to_linear_srgb(cie::to_xyz(l, &wl));
                    if rgb.iter().all(|v| v.is_finite()) {
                        for c in 0..3 {
                            acc[c] += rgb[c];
                        }
                    }
                    for c in 0..3 {
                        aacc[c] += g.albedo[c];
                        nacc[c] += g.normal[c];
                    }
                }
                for c in 0..3 {
                    crow[x][c] += acc[c];
                    arow[x][c] += aacc[c];
                    nrow[x][c] += nacc[c];
                }
            }
        });
    film.samples += count;
}
