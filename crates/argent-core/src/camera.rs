//! Thin-lens perspective camera described like a real one: focal length on a
//! full-frame sensor, f-number, focus distance.

use crate::geom::{Ray, V3};
use crate::sampler::disk;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Lens {
    /// Focal length in mm on a 36 mm (long side) sensor. 100 mm = macro.
    pub focal_mm: f32,
    /// f-number; 0 disables depth of field.
    pub f_stop: f32,
}

impl Default for Lens {
    fn default() -> Self {
        Lens { focal_mm: 100.0, f_stop: 0.0 }
    }
}

#[derive(Clone, Debug)]
pub struct Camera {
    pub pos: V3,
    pub forward: V3,
    pub right: V3,
    pub up: V3,
    /// Half-extents of the image plane at distance 1.
    pub half_w: f32,
    pub half_h: f32,
    pub lens_radius: f32,
    pub focus_dist: f32,
    /// Sub-frame shift (fraction of half-extent), for re-centring.
    pub shift: (f32, f32),
}

impl Camera {
    /// `aspect` = width / height.
    pub fn look_at(pos: V3, target: V3, up_hint: V3, lens: &Lens, aspect: f32) -> Camera {
        let forward = (target - pos).normalize();
        let mut right = forward.cross(up_hint);
        if right.length_squared() < 1e-8 {
            right = forward.cross(V3::Z);
        }
        let right = right.normalize();
        let up = right.cross(forward);
        // 36 mm along the long side of the frame.
        let long_half = 18.0 / lens.focal_mm;
        let (half_w, half_h) = if aspect >= 1.0 { (long_half, long_half / aspect) } else { (long_half * aspect, long_half) };
        let focus_dist = (target - pos).length();
        let lens_radius = if lens.f_stop > 0.0 { 0.5 * lens.focal_mm / lens.f_stop } else { 0.0 };
        Camera { pos, forward, right, up, half_w, half_h, lens_radius, focus_dist, shift: (0.0, 0.0) }
    }

    /// Ray through normalized image coordinates `(sx, sy)` in `[-1, 1]`,
    /// `+sy` up.
    #[inline]
    pub fn ray(&self, sx: f32, sy: f32, lens_u: f32, lens_v: f32) -> Ray {
        let x = (sx + self.shift.0) * self.half_w;
        let y = (sy + self.shift.1) * self.half_h;
        let dir = (self.forward + self.right * x + self.up * y).normalize();
        if self.lens_radius <= 0.0 {
            return Ray::new(self.pos, dir);
        }
        let ft = self.focus_dist / dir.dot(self.forward);
        let p_focus = self.pos + dir * ft;
        let (dx, dy) = disk(lens_u, lens_v);
        let o = self.pos + self.right * (dx * self.lens_radius) + self.up * (dy * self.lens_radius);
        Ray::new(o, (p_focus - o).normalize())
    }

    /// Project a world point to normalized image coordinates (`None` if behind).
    pub fn project(&self, p: V3) -> Option<(f32, f32)> {
        let d = p - self.pos;
        let z = d.dot(self.forward);
        if z <= 1e-6 {
            return None;
        }
        Some((d.dot(self.right) / z / self.half_w - self.shift.0, d.dot(self.up) / z / self.half_h - self.shift.1))
    }
}
