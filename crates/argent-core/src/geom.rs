//! Rays, hits and small vector helpers. Units are millimetres.

pub use glam::Vec3A as V3;

#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub o: V3,
    pub d: V3,
}

impl Ray {
    #[inline]
    pub fn new(o: V3, d: V3) -> Ray {
        Ray { o, d }
    }
    #[inline]
    pub fn at(&self, t: f32) -> V3 {
        self.o + self.d * t
    }
}

/// Offset a ray origin off a surface along its geometric normal, scaled to
/// the coordinate magnitude so self-intersection is avoided at any distance
/// from the origin (a 25 mm ring sits ~150 mm from the lights).
#[inline]
pub fn offset_origin(p: V3, ng: V3, dir: V3) -> V3 {
    let scale = 1e-4 * (1.0 + p.abs().max_element());
    let n = if ng.dot(dir) >= 0.0 { ng } else { -ng };
    p + n * scale
}

/// Orthonormal basis around `n` (Duff et al. 2017).
#[inline]
pub fn basis(n: V3) -> (V3, V3) {
    let sign = 1.0f32.copysign(n.z);
    let a = -1.0 / (sign + n.z);
    let b = n.x * n.y * a;
    (
        V3::new(1.0 + sign * n.x * n.x * a, sign * b, -sign * n.x),
        V3::new(b, sign + n.y * n.y * a, -n.y),
    )
}

/// Local shading frame.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub t: V3,
    pub b: V3,
    pub n: V3,
}

impl Frame {
    #[inline]
    pub fn new(n: V3) -> Frame {
        let (t, b) = basis(n);
        Frame { t, b, n }
    }
    #[inline]
    pub fn to_local(&self, v: V3) -> V3 {
        V3::new(v.dot(self.t), v.dot(self.b), v.dot(self.n))
    }
    #[inline]
    pub fn to_world(&self, v: V3) -> V3 {
        self.t * v.x + self.b * v.y + self.n * v.z
    }
}

#[inline]
pub fn reflect(wo: V3, n: V3) -> V3 {
    -wo + n * (2.0 * wo.dot(n))
}

/// Refract `wi` (pointing away from the surface, on the side of `n`) through
/// an interface with relative index `eta` (= n_t / n_i). `None` on TIR.
#[inline]
pub fn refract(wi: V3, n: V3, eta: f32) -> Option<V3> {
    let cos_i = n.dot(wi);
    let sin2_i = (1.0 - cos_i * cos_i).max(0.0);
    let sin2_t = sin2_i / (eta * eta);
    if sin2_t >= 1.0 {
        return None;
    }
    let cos_t = (1.0 - sin2_t).sqrt();
    Some(-wi / eta + n * (cos_i / eta - cos_t))
}

#[inline]
pub fn luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

/// Rotation about +Y by `deg` degrees.
#[inline]
pub fn rot_y(v: V3, deg: f32) -> V3 {
    let (s, c) = deg.to_radians().sin_cos();
    V3::new(c * v.x + s * v.z, v.y, -s * v.x + c * v.z)
}
