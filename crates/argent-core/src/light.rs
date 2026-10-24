//! Studio lighting: rectangular softboxes and flags, a backdrop environment,
//! and the floor the piece rests on.

use crate::geom::{Ray, V3};
use argent_spectra::{Spec, Spectrum, Wavelengths};
use std::sync::Arc;

/// A rectangle: a softbox when it emits, a flag (black card) when it doesn't.
#[derive(Clone)]
pub struct Panel {
    pub name: String,
    pub center: V3,
    /// Half-extent vectors along the two sides.
    pub hu: V3,
    pub hv: V3,
    /// Emitting side (`hu × hv` normalized).
    pub n: V3,
    pub area: f32,
    /// Radiance leaving the emitting face; `None` for a flag.
    pub emission: Option<Arc<dyn Spectrum>>,
    /// Softboxes are usually outside the frame; when false, camera rays pass through.
    pub camera_visible: bool,
}

impl Panel {
    pub fn new(name: &str, center: V3, hu: V3, hv: V3, emission: Option<Arc<dyn Spectrum>>) -> Panel {
        let c = hu.cross(hv);
        Panel { name: name.into(), center, hu, hv, n: c.normalize(), area: 4.0 * c.length(), emission, camera_visible: false }
    }

    /// A softbox of size `w × h` mm at `pos`, facing `target`; `h` runs
    /// along `up_hint` as far as possible.
    pub fn facing(name: &str, pos: V3, target: V3, w: f32, h: f32, up_hint: V3, emission: Option<Arc<dyn Spectrum>>) -> Panel {
        let n = (target - pos).normalize();
        let mut right = up_hint.cross(n);
        if right.length_squared() < 1e-6 {
            right = V3::X.cross(n);
        }
        let right = right.normalize();
        let up = n.cross(right);
        // hu × hv must point along n: right × up = n.
        Panel::new(name, pos, right * (0.5 * w), up * (0.5 * h), emission)
    }

    #[inline]
    pub fn intersect(&self, ray: &Ray, t_max: f32) -> Option<f32> {
        let denom = ray.d.dot(self.n);
        if denom.abs() < 1e-9 {
            return None;
        }
        let t = (self.center - ray.o).dot(self.n) / denom;
        if t <= 1e-6 || t >= t_max {
            return None;
        }
        let p = ray.at(t) - self.center;
        let a = p.dot(self.hu) / self.hu.length_squared();
        let b = p.dot(self.hv) / self.hv.length_squared();
        if a.abs() <= 1.0 && b.abs() <= 1.0 { Some(t) } else { None }
    }

    /// Radiance seen along `dir` arriving at the panel (front face only).
    #[inline]
    pub fn le(&self, dir: V3, wl: &Wavelengths) -> Spec {
        match &self.emission {
            Some(e) if dir.dot(self.n) < 0.0 => e.sample(wl),
            _ => Spec::ZERO,
        }
    }

    #[inline]
    pub fn sample_point(&self, u: f32, v: f32) -> V3 {
        self.center + self.hu * (2.0 * u - 1.0) + self.hv * (2.0 * v - 1.0)
    }
}

/// The backdrop and everything beyond the floor.
#[derive(Clone)]
pub enum Env {
    /// Vertical gradient: `bottom` below the horizon, `horizon` at it, `top`
    /// overhead; `power` shapes the blend.
    Gradient { top: Arc<dyn Spectrum>, horizon: Arc<dyn Spectrum>, bottom: Arc<dyn Spectrum>, power: f32 },
    /// Equirectangular HDR image (linear sRGB), +Y up, rotated by `rotation_deg`.
    Image { width: u32, height: u32, texels: Arc<Vec<[f32; 3]>>, rotation_deg: f32, intensity: f32 },
}

impl Env {
    pub fn eval(&self, dir: V3, wl: &Wavelengths) -> Spec {
        match self {
            Env::Gradient { top, horizon, bottom, power } => {
                let y = dir.y.clamp(-1.0, 1.0);
                if y >= 0.0 {
                    let t = y.powf(*power);
                    Spec::lerp(t, horizon.sample(wl), top.sample(wl))
                } else {
                    let t = (-y).powf(0.5);
                    Spec::lerp(t, horizon.sample(wl), bottom.sample(wl))
                }
            }
            Env::Image { width, height, texels, rotation_deg, intensity } => {
                let d = crate::geom::rot_y(dir, -*rotation_deg);
                let u = 0.5 + d.x.atan2(-d.z) / (2.0 * std::f32::consts::PI);
                let v = d.y.clamp(-1.0, 1.0).acos() / std::f32::consts::PI;
                let x = ((u * *width as f32) as u32).min(width - 1);
                let y = ((v * *height as f32) as u32).min(height - 1);
                let rgb = texels[(y * width + x) as usize];
                let s = argent_spectra::RgbIlluminant::new([rgb[0] * intensity, rgb[1] * intensity, rgb[2] * intensity]);
                s.sample(wl)
            }
        }
    }
}

/// An infinite floor at height `y` that dissolves into the backdrop between
/// `fade_start` and `fade_end` (mm from the vertical axis), so there is no
/// horizon line.
#[derive(Clone, Debug)]
pub struct Floor {
    pub y: f32,
    pub material: usize,
    pub fade_start: f32,
    pub fade_end: f32,
}

impl Floor {
    #[inline]
    pub fn intersect(&self, ray: &Ray, t_max: f32) -> Option<f32> {
        if ray.d.y.abs() < 1e-9 {
            return None;
        }
        let t = (self.y - ray.o.y) / ray.d.y;
        if t > 1e-5 && t < t_max { Some(t) } else { None }
    }

    /// Probability the floor is really there at `p` (1 near the piece).
    #[inline]
    pub fn presence(&self, p: V3) -> f32 {
        let r = (p.x * p.x + p.z * p.z).sqrt();
        if r <= self.fade_start {
            1.0
        } else if r >= self.fade_end {
            0.0
        } else {
            let t = (r - self.fade_start) / (self.fade_end - self.fade_start);
            1.0 - t * t * (3.0 - 2.0 * t)
        }
    }
}
