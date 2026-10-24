//! Isotropic GGX (Trowbridge–Reitz) with visible-normal sampling (Heitz 2018)
//! and a multiple-scattering energy table (Turquin 2019), so satin and
//! brushed finishes keep their brightness instead of darkening.

use crate::geom::V3;
use std::f32::consts::PI;
use std::sync::OnceLock;

#[inline]
fn tan2(w: V3) -> f32 {
    let c2 = w.z * w.z;
    ((1.0 - c2).max(0.0)) / c2.max(1e-12)
}

#[derive(Clone, Copy, Debug)]
pub struct Ggx {
    pub alpha: f32,
}

impl Ggx {
    /// Perceptual roughness → α.
    pub fn from_roughness(r: f32) -> Ggx {
        let r = r.clamp(0.0, 1.0);
        Ggx { alpha: (r * r).max(1e-4) }
    }

    /// Below this a lobe is treated as a perfect mirror.
    #[inline]
    pub fn is_smooth(&self) -> bool {
        self.alpha < 1.5e-3
    }

    #[inline]
    pub fn d(&self, h: V3) -> f32 {
        if h.z <= 0.0 {
            return 0.0;
        }
        let a2 = self.alpha * self.alpha;
        let c2 = h.z * h.z;
        let k = c2 * (a2 - 1.0) + 1.0;
        a2 / (PI * k * k)
    }

    #[inline]
    pub fn lambda(&self, w: V3) -> f32 {
        let t2 = tan2(w);
        if !t2.is_finite() {
            return 0.0;
        }
        0.5 * (-1.0 + (1.0 + self.alpha * self.alpha * t2).sqrt())
    }

    #[inline]
    pub fn g1(&self, w: V3) -> f32 {
        1.0 / (1.0 + self.lambda(w))
    }

    #[inline]
    pub fn g2(&self, wo: V3, wi: V3) -> f32 {
        1.0 / (1.0 + self.lambda(wo) + self.lambda(wi))
    }

    /// Density of visible normals from `wo` (both local, `wo.z > 0`).
    #[inline]
    pub fn d_visible(&self, wo: V3, h: V3) -> f32 {
        let c = wo.z.abs().max(1e-8);
        self.g1(wo) * wo.dot(h).max(0.0) * self.d(h) / c
    }

    /// Sample a visible microfacet normal (local, `wo.z > 0`).
    pub fn sample_h(&self, wo: V3, u1: f32, u2: f32) -> V3 {
        let a = self.alpha;
        let vh = V3::new(a * wo.x, a * wo.y, wo.z).normalize();
        let lensq = vh.x * vh.x + vh.y * vh.y;
        let t1 = if lensq > 1e-12 { V3::new(-vh.y, vh.x, 0.0) / lensq.sqrt() } else { V3::X };
        let t2 = vh.cross(t1);
        let r = u1.sqrt();
        let phi = 2.0 * PI * u2;
        let p1 = r * phi.cos();
        let mut p2 = r * phi.sin();
        let s = 0.5 * (1.0 + vh.z);
        p2 = (1.0 - s) * (1.0 - p1 * p1).max(0.0).sqrt() + s * p2;
        let nh = t1 * p1 + t2 * p2 + vh * (1.0 - p1 * p1 - p2 * p2).max(0.0).sqrt();
        V3::new(a * nh.x, a * nh.y, nh.z.max(1e-6)).normalize()
    }

    /// Single-scattering directional albedo with F = 1, for energy compensation.
    pub fn e(&self, cos_o: f32) -> f32 {
        energy_table().lookup(cos_o, self.alpha)
    }
}

const EN: usize = 32;

struct EnergyTable {
    e: Vec<f32>,
}

impl EnergyTable {
    fn lookup(&self, mu: f32, alpha: f32) -> f32 {
        let x = (mu.clamp(0.0, 1.0) * (EN - 1) as f32).min((EN - 1) as f32 - 1e-4);
        let y = (alpha.sqrt().clamp(0.0, 1.0) * (EN - 1) as f32).min((EN - 1) as f32 - 1e-4);
        let (xi, yi) = (x as usize, y as usize);
        let (fx, fy) = (x - xi as f32, y - yi as f32);
        let g = |i: usize, j: usize| self.e[j * EN + i];
        let a = g(xi, yi) * (1.0 - fx) + g(xi + 1, yi) * fx;
        let b = g(xi, yi + 1) * (1.0 - fx) + g(xi + 1, yi + 1) * fx;
        (a * (1.0 - fy) + b * fy).clamp(0.05, 1.0)
    }
}

fn energy_table() -> &'static EnergyTable {
    static T: OnceLock<EnergyTable> = OnceLock::new();
    T.get_or_init(|| {
        let mut e = vec![0.0f32; EN * EN];
        for j in 0..EN {
            let r = j as f32 / (EN - 1) as f32; // sqrt(alpha)
            let g = Ggx { alpha: (r * r).max(1e-4) };
            for i in 0..EN {
                let mu = (i as f32 / (EN - 1) as f32).max(1e-3);
                let wo = V3::new((1.0 - mu * mu).sqrt(), 0.0, mu);
                let n = 24;
                let mut sum = 0.0;
                for a in 0..n {
                    for b in 0..n {
                        let u1 = (a as f32 + 0.5) / n as f32;
                        let u2 = (b as f32 + 0.5) / n as f32;
                        let h = g.sample_h(wo, u1, u2);
                        let wi = crate::geom::reflect(wo, h);
                        if wi.z > 0.0 {
                            sum += g.g2(wo, wi) / g.g1(wo);
                        }
                    }
                }
                e[j * EN + i] = sum / (n * n) as f32;
            }
        }
        EnergyTable { e }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_lobes_conserve_energy() {
        let g = Ggx::from_roughness(0.05);
        assert!(g.e(0.8) > 0.97);
        let r = Ggx::from_roughness(0.6);
        let e = r.e(0.5);
        assert!(e < 0.95 && e > 0.5, "{e}");
    }

    #[test]
    fn vndf_samples_upper_hemisphere() {
        let g = Ggx::from_roughness(0.3);
        let wo = V3::new(0.6, 0.0, 0.8);
        for i in 0..64 {
            let h = g.sample_h(wo, (i as f32 + 0.5) / 64.0, 0.37);
            assert!(h.z > 0.0);
            assert!((h.length() - 1.0).abs() < 1e-4);
        }
    }
}
