//! Fresnel reflectance: exact unpolarized forms for dielectrics and conductors.

use crate::spec::{Spec, N};

/// Unpolarized Fresnel reflectance of a dielectric interface.
/// `cos_theta_i` is signed: negative means the ray arrives from inside, and
/// the relative index is inverted. Returns 1 under total internal reflection.
#[inline]
pub fn dielectric(cos_theta_i: f32, eta: f32) -> f32 {
    let mut cos_i = cos_theta_i.clamp(-1.0, 1.0);
    let mut eta = eta;
    if cos_i < 0.0 {
        eta = 1.0 / eta;
        cos_i = -cos_i;
    }
    let sin2_i = 1.0 - cos_i * cos_i;
    let sin2_t = sin2_i / (eta * eta);
    if sin2_t >= 1.0 {
        return 1.0;
    }
    let cos_t = (1.0 - sin2_t).max(0.0).sqrt();
    let r_parl = (eta * cos_i - cos_t) / (eta * cos_i + cos_t);
    let r_perp = (cos_i - eta * cos_t) / (cos_i + eta * cos_t);
    0.5 * (r_parl * r_parl + r_perp * r_perp)
}

#[derive(Clone, Copy, Debug)]
struct C {
    re: f32,
    im: f32,
}
impl C {
    #[inline]
    fn new(re: f32, im: f32) -> C {
        C { re, im }
    }
    #[inline]
    fn add(self, o: C) -> C {
        C::new(self.re + o.re, self.im + o.im)
    }
    #[inline]
    fn sub(self, o: C) -> C {
        C::new(self.re - o.re, self.im - o.im)
    }
    #[inline]
    fn mul(self, o: C) -> C {
        C::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
    #[inline]
    fn scale(self, s: f32) -> C {
        C::new(self.re * s, self.im * s)
    }
    #[inline]
    fn div(self, o: C) -> C {
        let d = o.re * o.re + o.im * o.im;
        C::new((self.re * o.re + self.im * o.im) / d, (self.im * o.re - self.re * o.im) / d)
    }
    #[inline]
    fn norm(self) -> f32 {
        self.re * self.re + self.im * self.im
    }
    #[inline]
    fn sqrt(self) -> C {
        let n = self.norm().sqrt();
        if n == 0.0 {
            return C::new(0.0, 0.0);
        }
        let t1 = (0.5 * (n + self.re.abs())).sqrt();
        let t2 = 0.5 * self.im / t1;
        if self.re >= 0.0 {
            C::new(t1, t2)
        } else {
            C::new(t2.abs(), t1.copysign(self.im))
        }
    }
}

/// Exact unpolarized Fresnel reflectance of a conductor with complex index
/// `eta + i k` (relative to the outside medium), for one wavelength.
#[inline]
pub fn conductor1(cos_theta_i: f32, eta: f32, k: f32) -> f32 {
    let cos_i = cos_theta_i.clamp(0.0, 1.0);
    let sin2_i = 1.0 - cos_i * cos_i;
    let e = C::new(eta, k);
    let sin2_t = C::new(sin2_i, 0.0).div(e.mul(e));
    let cos_t = C::new(1.0, 0.0).sub(sin2_t).sqrt();
    let ec = e.scale(cos_i);
    let r_parl = ec.sub(cos_t).div(ec.add(cos_t));
    let ci = C::new(cos_i, 0.0);
    let ect = e.mul(cos_t);
    let r_perp = ci.sub(ect).div(ci.add(ect));
    0.5 * (r_parl.norm() + r_perp.norm())
}

/// [`conductor1`] for every carried wavelength.
#[inline]
pub fn conductor(cos_theta_i: f32, eta: Spec, k: Spec) -> Spec {
    let mut r = [0.0; N];
    for i in 0..N {
        r[i] = conductor1(cos_theta_i, eta.0[i], k.0[i]);
    }
    Spec(r)
}

/// Gulbrandsen 2014, "Artist Friendly Metallic Fresnel": recover `(n, k)`
/// from a normal-incidence reflectance `r` and an edge tint `g`, both in
/// `[0, 1)`. Lets an alloy be specified by its measured reflectance.
pub fn gulbrandsen(r: f32, g: f32) -> (f32, f32) {
    let r = r.clamp(0.0, 0.9999);
    let sr = r.sqrt();
    let n_min = (1.0 - r) / (1.0 + r);
    let n_max = (1.0 + sr) / (1.0 - sr);
    let n = g * n_min + (1.0 - g) * n_max;
    let k2 = ((n + 1.0) * (n + 1.0) * r - (n - 1.0) * (n - 1.0)) / (1.0 - r);
    (n, k2.max(0.0).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glass_normal_incidence_is_four_percent() {
        assert!((dielectric(1.0, 1.5) - 0.04).abs() < 1e-4);
    }

    #[test]
    fn total_internal_reflection() {
        assert_eq!(dielectric(-0.1, 1.5), 1.0);
    }

    #[test]
    fn conductor_matches_normal_incidence_formula() {
        let (n, k) = (0.2f32, 3.0f32);
        let expect = ((n - 1.0).powi(2) + k * k) / ((n + 1.0).powi(2) + k * k);
        assert!((conductor1(1.0, n, k) - expect).abs() < 1e-5);
        // grazing incidence reflects everything
        assert!(conductor1(0.0, n, k) > 0.999);
    }

    #[test]
    fn gulbrandsen_round_trips_normal_incidence() {
        for &(r, g) in &[(0.95f32, 0.9f32), (0.6, 0.7), (0.3, 0.2)] {
            let (n, k) = gulbrandsen(r, g);
            let back = conductor1(1.0, n, k);
            assert!((back - r).abs() < 1e-3, "{r} {g} -> {n} {k} -> {back}");
        }
    }
}
