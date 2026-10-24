//! Per-hit scattering functions, evaluated in the local shading frame
//! (`+Z` = shading normal) for the wavelengths a path carries.

use crate::geom::V3;
use crate::microfacet::Ggx;
use argent_spectra::{fresnel, Spec};
use std::f32::consts::FRAC_1_PI;

#[derive(Clone, Copy, Debug)]
pub enum Bsdf {
    /// Polished or satin metal: GGX with exact complex Fresnel per wavelength
    /// and multiple-scattering compensation.
    Conductor { ggx: Ggx, eta: Spec, k: Spec },
    /// Dielectric coat over a diffuse base: acrylic floors, tarnish, enamel.
    Coated { ggx: Ggx, eta: f32, base: Spec },
    Diffuse { albedo: Spec },
    /// Smooth gemstone surface; `eta` per wavelength. When the values differ
    /// the stone disperses, and refraction keeps only the hero wavelength.
    Dielectric { eta: Spec },
    /// Absorbs everything (flags, light housings).
    Black,
}

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub wi: V3,
    /// BSDF value (already including Fresnel), *not* multiplied by cosine.
    pub f: Spec,
    pub pdf: f32,
    pub delta: bool,
    /// The direction crossed the surface (refraction).
    pub transmit: bool,
    /// The caller must terminate secondary wavelengths (dispersive refraction).
    pub terminate_secondary: bool,
}

impl Bsdf {
    /// True when the BSDF has no non-delta component (skip light sampling).
    #[inline]
    pub fn is_delta(&self) -> bool {
        match self {
            Bsdf::Dielectric { .. } => true,
            Bsdf::Conductor { ggx, .. } => ggx.is_smooth(),
            _ => false,
        }
    }

    pub fn eval(&self, wo: V3, wi: V3) -> Spec {
        match *self {
            Bsdf::Conductor { ggx, eta, k } => {
                if wo.z <= 0.0 || wi.z <= 0.0 || ggx.is_smooth() {
                    return Spec::ZERO;
                }
                let h = (wo + wi).normalize();
                let f = fresnel::conductor(wo.dot(h), eta, k);
                let base = ggx.d(h) * ggx.g2(wo, wi) / (4.0 * wo.z * wi.z);
                let e = ggx.e(wo.z);
                let f0 = fresnel::conductor(1.0, eta, k);
                f * base * (Spec::ONE + f0 * ((1.0 - e) / e))
            }
            Bsdf::Coated { ggx, eta, base } => {
                if wo.z <= 0.0 || wi.z <= 0.0 {
                    return Spec::ZERO;
                }
                let h = (wo + wi).normalize();
                let fs = fresnel::dielectric(wo.dot(h), eta);
                let spec = ggx.d(h) * ggx.g2(wo, wi) * fs / (4.0 * wo.z * wi.z);
                let fo = fresnel::dielectric(wo.z, eta);
                let fi = fresnel::dielectric(wi.z, eta);
                base * ((1.0 - fo) * (1.0 - fi) * FRAC_1_PI) + Spec::splat(spec)
            }
            Bsdf::Diffuse { albedo } => {
                if wo.z <= 0.0 || wi.z <= 0.0 {
                    return Spec::ZERO;
                }
                albedo * FRAC_1_PI
            }
            Bsdf::Dielectric { .. } | Bsdf::Black => Spec::ZERO,
        }
    }

    pub fn pdf(&self, wo: V3, wi: V3) -> f32 {
        match *self {
            Bsdf::Conductor { ggx, .. } => {
                if wo.z <= 0.0 || wi.z <= 0.0 || ggx.is_smooth() {
                    return 0.0;
                }
                let h = (wo + wi).normalize();
                ggx.d_visible(wo, h) / (4.0 * wo.dot(h).max(1e-8))
            }
            Bsdf::Coated { ggx, eta, base } => {
                if wo.z <= 0.0 || wi.z <= 0.0 {
                    return 0.0;
                }
                let ps = coat_spec_prob(wo.z, eta, base);
                let h = (wo + wi).normalize();
                let p_spec = ggx.d_visible(wo, h) / (4.0 * wo.dot(h).max(1e-8));
                ps * p_spec + (1.0 - ps) * wi.z * FRAC_1_PI
            }
            Bsdf::Diffuse { .. } => {
                if wo.z <= 0.0 || wi.z <= 0.0 {
                    0.0
                } else {
                    wi.z * FRAC_1_PI
                }
            }
            Bsdf::Dielectric { .. } | Bsdf::Black => 0.0,
        }
    }

    /// `u0` picks a lobe, `(u1, u2)` the direction.
    pub fn sample(&self, wo: V3, u0: f32, u1: f32, u2: f32) -> Option<Sample> {
        match *self {
            Bsdf::Conductor { ggx, eta, k } => {
                if wo.z <= 0.0 {
                    return None;
                }
                if ggx.is_smooth() {
                    let wi = V3::new(-wo.x, -wo.y, wo.z);
                    let f = fresnel::conductor(wo.z, eta, k) / wi.z.max(1e-8);
                    return Some(Sample { wi, f, pdf: 1.0, delta: true, transmit: false, terminate_secondary: false });
                }
                let h = ggx.sample_h(wo, u1, u2);
                let wi = crate::geom::reflect(wo, h);
                if wi.z <= 0.0 {
                    return None;
                }
                let pdf = ggx.d_visible(wo, h) / (4.0 * wo.dot(h).max(1e-8));
                let f = self.eval(wo, wi);
                Some(Sample { wi, f, pdf, delta: false, transmit: false, terminate_secondary: false })
            }
            Bsdf::Coated { ggx, eta, base } => {
                if wo.z <= 0.0 {
                    return None;
                }
                let ps = coat_spec_prob(wo.z, eta, base);
                let wi = if u0 < ps {
                    let h = ggx.sample_h(wo, u1, u2);
                    crate::geom::reflect(wo, h)
                } else {
                    crate::sampler::cosine_hemisphere(u1, u2)
                };
                if wi.z <= 0.0 {
                    return None;
                }
                let pdf = self.pdf(wo, wi);
                if pdf <= 0.0 {
                    return None;
                }
                Some(Sample { wi, f: self.eval(wo, wi), pdf, delta: false, transmit: false, terminate_secondary: false })
            }
            Bsdf::Diffuse { albedo } => {
                if wo.z <= 0.0 {
                    return None;
                }
                let wi = crate::sampler::cosine_hemisphere(u1, u2);
                let pdf = wi.z * FRAC_1_PI;
                if pdf <= 0.0 {
                    return None;
                }
                Some(Sample { wi, f: albedo * FRAC_1_PI, pdf, delta: false, transmit: false, terminate_secondary: false })
            }
            Bsdf::Dielectric { eta } => {
                let dispersive = (eta.max_value() - eta.min_value()) > 1e-5;
                let e0 = eta[0];
                let cos_o = wo.z;
                let f0 = fresnel::dielectric(cos_o, e0);
                if u0 < f0 {
                    let wi = V3::new(-wo.x, -wo.y, wo.z);
                    let c = wi.z.abs().max(1e-8);
                    // Reflection direction is the same for every wavelength;
                    // weight each by its own Fresnel over the hero's.
                    let f = Spec(std::array::from_fn(|i| fresnel::dielectric(cos_o, eta[i]))) / c;
                    return Some(Sample { wi, f, pdf: f0, delta: true, transmit: false, terminate_secondary: false });
                }
                let entering = cos_o > 0.0;
                let etap = if entering { e0 } else { 1.0 / e0 };
                let n = if entering { V3::Z } else { -V3::Z };
                let wi = crate::geom::refract(wo, n, etap)?;
                let c = wi.z.abs().max(1e-8);
                // Radiance is compressed by 1/η'² crossing into the denser side.
                let ft = (1.0 - f0) / (c * etap * etap);
                Some(Sample {
                    wi,
                    f: Spec::splat(ft),
                    pdf: 1.0 - f0,
                    delta: true,
                    transmit: true,
                    terminate_secondary: dispersive,
                })
            }
            Bsdf::Black => None,
        }
    }

    /// Albedo-like colour for the denoiser's guide buffer.
    pub fn guide_albedo(&self) -> Spec {
        match *self {
            Bsdf::Conductor { eta, k, .. } => fresnel::conductor(1.0, eta, k),
            Bsdf::Coated { base, eta, .. } => base * (1.0 - fresnel::dielectric(1.0, eta)) + Spec::splat(fresnel::dielectric(1.0, eta)),
            Bsdf::Diffuse { albedo } => albedo,
            Bsdf::Dielectric { .. } => Spec::ONE,
            Bsdf::Black => Spec::ZERO,
        }
    }
}

#[inline]
fn coat_spec_prob(cos_o: f32, eta: f32, base: Spec) -> f32 {
    let f = fresnel::dielectric(cos_o, eta);
    let d = base.average() * (1.0 - f);
    (f / (f + d).max(1e-6)).clamp(0.15, 0.95)
}
