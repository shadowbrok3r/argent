//! Small f64 vector helpers shared by the simplifier and the normals pass.

pub(crate) type D3 = [f64; 3];

#[inline]
pub(crate) fn d3(p: [f32; 3]) -> D3 {
    [p[0] as f64, p[1] as f64, p[2] as f64]
}

#[inline]
pub(crate) fn add(a: D3, b: D3) -> D3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
pub(crate) fn sub(a: D3, b: D3) -> D3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
pub(crate) fn scale(a: D3, s: f64) -> D3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
pub(crate) fn dot(a: D3, b: D3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
pub(crate) fn cross(a: D3, b: D3) -> D3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

#[inline]
pub(crate) fn len2(a: D3) -> f64 {
    dot(a, a)
}

#[inline]
pub(crate) fn len(a: D3) -> f64 {
    dot(a, a).sqrt()
}

/// Unit normal and twice the area of triangle `(a, b, c)`; a zero normal
/// and zero area when degenerate.
#[inline]
pub(crate) fn normal_area2(a: D3, b: D3, c: D3) -> (D3, f64) {
    let n = cross(sub(b, a), sub(c, a));
    let l = len(n);
    if l > 0.0 && l.is_finite() { (scale(n, 1.0 / l), l) } else { ([0.0; 3], 0.0) }
}

/// Interior angle at `p` of the triangle corner whose other vertices are
/// `a` and `b`.
#[inline]
pub(crate) fn corner_angle(p: D3, a: D3, b: D3) -> f64 {
    let (e1, e2) = (sub(a, p), sub(b, p));
    len(cross(e1, e2)).atan2(dot(e1, e2))
}

/// Normalise to f32, falling back to +Z for a zero or non-finite vector.
#[inline]
pub(crate) fn unit_f32(n: D3) -> [f32; 3] {
    let l = len(n);
    if l > 0.0 && l.is_finite() { [(n[0] / l) as f32, (n[1] / l) as f32, (n[2] / l) as f32] } else { [0.0, 0.0, 1.0] }
}

/// Squared length of the longest edge of a triangle.
#[inline]
pub(crate) fn max_edge2(v: &[D3; 3]) -> f64 {
    len2(sub(v[1], v[0])).max(len2(sub(v[2], v[1]))).max(len2(sub(v[0], v[2])))
}
