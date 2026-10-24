//! Named camera views and automatic framing.

use crate::camera::{Camera, Lens};
use crate::geom::V3;

/// A camera direction around the piece, in the canonical frame (+Y up,
/// front +Z): azimuth from +Z toward +X, elevation above the horizon.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct View {
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
    /// Fraction of the frame the piece's larger projected extent fills.
    pub fill: f32,
}

impl View {
    pub fn named(name: &str) -> Option<View> {
        let v = |a, e, f| View { azimuth_deg: a, elevation_deg: e, fill: f };
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "hero" => v(34.0, 26.0, 0.72),
            "front" | "face" => v(0.0, 6.0, 0.70),
            "side" | "profile" => v(90.0, 6.0, 0.72),
            "top" => v(0.0, 62.0, 0.70),
            "three-quarter" | "34" => v(45.0, 24.0, 0.72),
            "back" => v(160.0, 16.0, 0.72),
            "low" => v(22.0, 3.0, 0.72),
            // Framed on the head by `Prepared::shot_region` (see the CLI).
            "detail" | "macro" => v(14.0, 34.0, 0.92),
            _ => return None,
        })
    }

    pub const NAMES: &'static [&'static str] = &["hero", "front", "side", "top", "three-quarter", "back", "low", "detail"];

    /// Unit vector from the piece toward the camera.
    pub fn direction(&self) -> V3 {
        let (sa, ca) = self.azimuth_deg.to_radians().sin_cos();
        let (se, ce) = self.elevation_deg.to_radians().sin_cos();
        V3::new(sa * ce, se, ca * ce)
    }
}

/// Place a camera along `view` so `points` (a subsample of the piece,
/// already in world space) fill `view.fill` of the frame, centred.
pub fn frame(points: &[V3], view: &View, lens: &Lens, aspect: f32) -> Camera {
    let (mut lo, mut hi) = (V3::splat(f32::MAX), V3::splat(f32::MIN));
    for p in points {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    let center = (lo + hi) * 0.5;
    let radius = ((hi - lo) * 0.5).length().max(1e-3);
    let dir = view.direction();
    let up = if dir.y.abs() > 0.98 { V3::Z } else { V3::Y };
    let half = (18.0 / lens.focal_mm).min(18.0 / lens.focal_mm * if aspect < 1.0 { aspect } else { 1.0 / aspect });
    let mut dist = radius / half.atan().sin() / view.fill.max(0.05);
    let mut target = center;
    let mut cam = Camera::look_at(target + dir * dist, target, up, lens, aspect);
    for _ in 0..6 {
        let (mut mnx, mut mxx, mut mny, mut mxy) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in points {
            if let Some((x, y)) = cam.project(*p) {
                mnx = mnx.min(x);
                mxx = mxx.max(x);
                mny = mny.min(y);
                mxy = mxy.max(y);
            }
        }
        if mnx > mxx {
            break;
        }
        let ext = ((mxx - mnx) * 0.5).max((mxy - mny) * 0.5);
        // Re-centre: move the target in the image plane by the offset at
        // the current distance, then scale distance to hit the fill.
        let cx = (mxx + mnx) * 0.5 * cam.half_w;
        let cy = (mxy + mny) * 0.5 * cam.half_h;
        let pd = (center - cam.pos).dot(cam.forward);
        target += cam.right * (cx * pd) + cam.up * (cy * pd);
        dist *= ext / view.fill.max(0.05);
        cam = Camera::look_at(target + dir * dist, target, up, lens, aspect);
    }
    cam
}
