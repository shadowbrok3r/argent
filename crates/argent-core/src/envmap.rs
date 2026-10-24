//! Image-based lighting: an equirectangular HDR environment with importance
//! sampling (luminance × sin θ), so a photographed studio lights the piece
//! with few samples.

use crate::geom::V3;
use argent_spectra::{cie, Spec, Spectrum, Wavelengths};
use std::f32::consts::PI;
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// RGB → spectrum as a fixed linear combination of three smooth basis
/// spectra under D65, corrected by a 3×3 matrix so every linear sRGB colour
/// round-trips exactly through the CIE observer. Linear, so an HDR texel
/// costs three multiply-adds per wavelength (no per-texel fit).
pub struct LinearRgb {
    basis: [Vec<f32>; 3],
    inv: [[f32; 3]; 3],
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl LinearRgb {
    pub fn get() -> &'static LinearRgb {
        static L: OnceLock<LinearRgb> = OnceLock::new();
        L.get_or_init(LinearRgb::build)
    }

    fn build() -> LinearRgb {
        let n = 471usize;
        // Illuminant shape for the basis; the 3×3 correction below makes any
        // RGB round-trip exactly, so a 6504 K blackbody is as good as D65 here.
        let bb = argent_spectra::Blackbody::new(6504.0);
        let d65 = |l: f32| bb.eval(l);
        let mut basis = [vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]];
        for i in 0..n {
            let l = 360.0 + i as f32;
            let wb = 1.0 - smoothstep(470.0, 515.0, l);
            let wr = smoothstep(565.0, 610.0, l);
            let wg = (1.0 - wb - wr).max(0.0);
            let e = d65(l);
            basis[0][i] = wr * e;
            basis[1][i] = wg * e;
            basis[2][i] = wb * e;
        }
        // M[i][j] = linear sRGB channel i of basis j (through the observer).
        let mut m = [[0.0f32; 3]; 3];
        for (j, b) in basis.iter().enumerate() {
            let mut xyz = [0.0f32; 3];
            for (i, v) in b.iter().enumerate() {
                let c = cie::xyz_bar(360.0 + i as f32);
                for k in 0..3 {
                    xyz[k] += c[k] * v;
                }
            }
            let rgb = cie::xyz_to_linear_srgb(xyz.map(|v| v / cie::CIE_Y_INTEGRAL));
            for i in 0..3 {
                m[i][j] = rgb[i];
            }
        }
        LinearRgb { basis, inv: inv3(m) }
    }

    /// Basis weights for a linear sRGB colour.
    #[inline]
    pub fn weights(&self, rgb: [f32; 3]) -> [f32; 3] {
        let m = &self.inv;
        [
            m[0][0] * rgb[0] + m[0][1] * rgb[1] + m[0][2] * rgb[2],
            m[1][0] * rgb[0] + m[1][1] * rgb[1] + m[1][2] * rgb[2],
            m[2][0] * rgb[0] + m[2][1] * rgb[1] + m[2][2] * rgb[2],
        ]
    }

    #[inline]
    pub fn eval(&self, w: [f32; 3], l: f32) -> f32 {
        let x = (l - 360.0).clamp(0.0, 469.999);
        let i = x as usize;
        let f = x - i as f32;
        let mut v = 0.0;
        for c in 0..3 {
            let b = &self.basis[c];
            v += w[c] * (b[i] * (1.0 - f) + b[i + 1] * f);
        }
        v.max(0.0)
    }

    #[inline]
    pub fn sample(&self, w: [f32; 3], wl: &Wavelengths) -> Spec {
        Spec(std::array::from_fn(|k| self.eval(w, wl.lambda[k])))
    }
}

fn inv3(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let [[a, b, c], [d, e, f], [g, h, i]] = m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    let k = 1.0 / det;
    [
        [(e * i - f * h) * k, (c * h - b * i) * k, (b * f - c * e) * k],
        [(f * g - d * i) * k, (a * i - c * g) * k, (c * d - a * f) * k],
        [(d * h - e * g) * k, (b * g - a * h) * k, (a * e - b * d) * k],
    ]
}

/// The image data and its sampling tables (independent of rotation).
pub struct EnvData {
    pub width: u32,
    pub height: u32,
    weights: Vec<[f32; 3]>,
    /// Marginal CDF over rows and conditional CDFs per row (each `w+1` long).
    marginal: Vec<f32>,
    conditional: Vec<f32>,
    /// Integral of the sampling function, for pdfs.
    total: f32,
}

/// An equirectangular environment, pre-converted to basis weights, with a
/// 2D sampling distribution, turned `rotation_deg` about +Y.
#[derive(Clone)]
pub struct EnvMap {
    pub data: Arc<EnvData>,
    pub rotation_deg: f32,
}

impl std::ops::Deref for EnvMap {
    type Target = EnvData;
    fn deref(&self) -> &EnvData {
        &self.data
    }
}

impl EnvMap {
    pub fn load(path: &Path, exposure_ev: f32, rotation_deg: f32) -> anyhow::Result<Arc<EnvMap>> {
        Ok(Arc::new(EnvMap { data: EnvData::load(path, exposure_ev)?, rotation_deg }))
    }
}

impl EnvData {
    pub fn load(path: &Path, exposure_ev: f32) -> anyhow::Result<Arc<EnvData>> {
        let img = image::open(path)?.to_rgb32f();
        let (w, h) = (img.width(), img.height());
        let k = 2f32.powf(exposure_ev);
        let lin = LinearRgb::get();
        let px: Vec<[f32; 3]> = img.pixels().map(|p| [p.0[0] * k, p.0[1] * k, p.0[2] * k]).collect();
        let weights: Vec<[f32; 3]> = px.iter().map(|&c| lin.weights(c)).collect();
        // Sampling function: luminance × sin θ (equal-area correction).
        let mut conditional = vec![0.0f32; (w as usize + 1) * h as usize];
        let mut row_sum = vec![0.0f32; h as usize];
        for y in 0..h as usize {
            let s = (PI * (y as f32 + 0.5) / h as f32).sin();
            let base = y * (w as usize + 1);
            let mut acc = 0.0;
            conditional[base] = 0.0;
            for x in 0..w as usize {
                let c = px[y * w as usize + x];
                acc += (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]).max(0.0) * s + 1e-6;
                conditional[base + x + 1] = acc;
            }
            row_sum[y] = acc;
            for x in 0..=w as usize {
                conditional[base + x] /= acc;
            }
        }
        let mut marginal = vec![0.0f32; h as usize + 1];
        let mut acc = 0.0;
        for y in 0..h as usize {
            acc += row_sum[y];
            marginal[y + 1] = acc;
        }
        let total = acc / (w as f32 * h as f32);
        for v in &mut marginal {
            *v /= acc;
        }
        Ok(Arc::new(EnvData { width: w, height: h, weights, marginal, conditional, total }))
    }
}

impl EnvMap {

    #[inline]
    fn uv(&self, dir: V3) -> (f32, f32) {
        let d = crate::geom::rot_y(dir, -self.rotation_deg);
        let u = 0.5 + d.x.atan2(-d.z) / (2.0 * PI);
        let v = d.y.clamp(-1.0, 1.0).acos() / PI;
        (u.rem_euclid(1.0), v)
    }

    #[inline]
    fn dir(&self, u: f32, v: f32) -> V3 {
        let phi = (u - 0.5) * 2.0 * PI;
        let theta = v * PI;
        let (st, ct) = theta.sin_cos();
        let d = V3::new(st * phi.sin(), ct, -st * phi.cos());
        crate::geom::rot_y(d, self.rotation_deg)
    }

    #[inline]
    fn texel(&self, u: f32, v: f32) -> usize {
        let x = ((u * self.width as f32) as u32).min(self.width - 1);
        let y = ((v * self.height as f32) as u32).min(self.height - 1);
        (y * self.width + x) as usize
    }

    pub fn eval(&self, dir: V3, wl: &Wavelengths) -> Spec {
        let (u, v) = self.uv(dir);
        LinearRgb::get().sample(self.weights[self.texel(u, v)], wl)
    }

    /// Solid-angle density of [`EnvMap::sample`] choosing `dir`.
    pub fn pdf(&self, dir: V3) -> f32 {
        let (u, v) = self.uv(dir);
        let st = (PI * v).sin();
        if st <= 1e-6 {
            return 0.0;
        }
        let x = ((u * self.width as f32) as usize).min(self.width as usize - 1);
        let y = ((v * self.height as f32) as usize).min(self.height as usize - 1);
        let row = y * (self.width as usize + 1);
        let pc = (self.conditional[row + x + 1] - self.conditional[row + x]) * self.width as f32;
        let pm = (self.marginal[y + 1] - self.marginal[y]) * self.height as f32;
        pc * pm / (2.0 * PI * PI * st)
    }

    /// Importance-sample a direction; returns (direction, solid-angle pdf).
    pub fn sample(&self, u1: f32, u2: f32) -> (V3, f32) {
        let y = self.marginal.partition_point(|&c| c <= u1).clamp(1, self.height as usize) - 1;
        let row = y * (self.width as usize + 1);
        let cond = &self.conditional[row..row + self.width as usize + 1];
        let x = cond.partition_point(|&c| c <= u2).clamp(1, self.width as usize) - 1;
        // Jitter within the texel.
        let fy = (u1 - self.marginal[y]) / (self.marginal[y + 1] - self.marginal[y]).max(1e-12);
        let fx = (u2 - cond[x]) / (cond[x + 1] - cond[x]).max(1e-12);
        let u = (x as f32 + fx.clamp(0.0, 0.999)) / self.width as f32;
        let v = (y as f32 + fy.clamp(0.0, 0.999)) / self.height as f32;
        let d = self.dir(u, v);
        (d, self.pdf(d))
    }

    /// Mean radiance (luminance), to balance light-picking against panels.
    pub fn average_luminance(&self) -> f32 {
        self.total * 4.0 * PI / (2.0 * PI * PI) * 0.5
    }
}
