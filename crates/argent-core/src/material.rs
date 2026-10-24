//! Scene materials: wavelength-independent descriptions that produce a
//! [`Bsdf`] at each hit.

use crate::bsdf::Bsdf;
use crate::microfacet::Ggx;
use argent_spectra::alloys::Alloy;
use argent_spectra::gems::{self, GemKind};
use argent_spectra::metals::ComplexIor;
use argent_spectra::{Constant, RgbAlbedo, Spec, Spectrum, Wavelengths};
use std::sync::Arc;

/// Oxidised recesses (liver of sulfur on silver, darkened recesses on gold):
/// a dark, softly glossy tarnish blended in where the cavity mask is high.
#[derive(Clone)]
pub struct Patina {
    /// 0 = bright polish everywhere, 1 = the full antique.
    pub strength: f32,
    /// Mask values below `lo` stay polished, above `hi` are fully dark.
    pub lo: f32,
    pub hi: f32,
    pub base: Arc<dyn Spectrum>,
    pub ggx: Ggx,
    pub eta: f32,
}

#[derive(Clone)]
pub enum Material {
    Metal { ior: Arc<ComplexIor>, ggx: Ggx, patina: Option<Patina> },
    Gem { ior: Arc<dyn Spectrum>, absorption: Arc<dyn Spectrum>, kind: GemKind, opaque_base: Option<Arc<dyn Spectrum>> },
    Plastic { base: Arc<dyn Spectrum>, eta: f32, ggx: Ggx },
    Diffuse { albedo: Arc<dyn Spectrum> },
    Black,
}

impl Material {
    pub fn metal(alloy: Alloy, roughness: f32) -> Material {
        Material::Metal { ior: Arc::new(alloy.ior()), ggx: Ggx::from_roughness(roughness), patina: None }
    }

    pub fn with_patina(self, p: Patina) -> Material {
        match self {
            Material::Metal { ior, ggx, .. } => Material::Metal { ior, ggx, patina: Some(p) },
            m => m,
        }
    }

    pub fn gem(name: &str) -> Option<Material> {
        let g = gems::get(name)?;
        let opaque_base: Option<Arc<dyn Spectrum>> = g.body_albedo.map(|c| Arc::new(RgbAlbedo::new(c)) as Arc<dyn Spectrum>);
        Some(Material::Gem { ior: Arc::from(g.ior), absorption: Arc::from(g.absorption), kind: g.kind, opaque_base })
    }

    pub fn plastic(rgb: [f32; 3], eta: f32, roughness: f32) -> Material {
        Material::Plastic { base: Arc::new(RgbAlbedo::new(rgb)), eta, ggx: Ggx::from_roughness(roughness) }
    }

    pub fn diffuse(rgb: [f32; 3]) -> Material {
        Material::Diffuse { albedo: Arc::new(RgbAlbedo::new(rgb)) }
    }

    /// True for gems: shade with facet (geometric) normals so facets stay crisp.
    pub fn wants_flat_normals(&self) -> bool {
        matches!(self, Material::Gem { kind: GemKind::Transparent, .. })
    }

    /// Absorption inside the material (1/mm), if it is a transmissive medium.
    pub fn medium(&self) -> Option<&Arc<dyn Spectrum>> {
        match self {
            Material::Gem { absorption, kind: GemKind::Transparent | GemKind::Opal, .. } => Some(absorption),
            _ => None,
        }
    }

    /// The BSDF at a hit. `mask` is the interpolated cavity value, `u` picks
    /// between stochastic layers (polish vs tarnish).
    pub fn bsdf(&self, wl: &Wavelengths, mask: f32, u: f32) -> Bsdf {
        match self {
            Material::Metal { ior, ggx, patina } => {
                if let Some(p) = patina {
                    let t = ((mask - p.lo) / (p.hi - p.lo).max(1e-4)).clamp(0.0, 1.0);
                    let w = p.strength * t * t * (3.0 - 2.0 * t);
                    if u < w {
                        return Bsdf::Coated { ggx: p.ggx, eta: p.eta, base: p.base.sample(wl) };
                    }
                }
                let (eta, k) = ior.sample(wl);
                Bsdf::Conductor { ggx: *ggx, eta, k }
            }
            Material::Gem { ior, kind, opaque_base, .. } => match kind {
                GemKind::Opaque => Bsdf::Coated {
                    ggx: Ggx::from_roughness(0.06),
                    eta: ior.eval(550.0),
                    base: opaque_base.as_ref().map(|b| b.sample(wl)).unwrap_or(Spec::splat(0.01)),
                },
                _ => Bsdf::Dielectric { eta: ior.sample(wl) },
            },
            Material::Plastic { base, eta, ggx } => Bsdf::Coated { ggx: *ggx, eta: *eta, base: base.sample(wl) },
            Material::Diffuse { albedo } => Bsdf::Diffuse { albedo: albedo.sample(wl) },
            Material::Black => Bsdf::Black,
        }
    }
}

/// Default tarnish for a metal: near-black, softly glossy.
pub fn default_patina(strength: f32, alloy: Alloy) -> Patina {
    let base: Arc<dyn Spectrum> = match alloy {
        Alloy::Sterling | Alloy::FineSilver => Arc::new(RgbAlbedo::new([0.018, 0.018, 0.02])),
        Alloy::Bronze | Alloy::Brass | Alloy::Copper => Arc::new(RgbAlbedo::new([0.03, 0.016, 0.008])),
        _ => Arc::new(RgbAlbedo::new([0.03, 0.02, 0.01])),
    };
    Patina { strength, lo: 0.18, hi: 0.62, base, ggx: Ggx::from_roughness(0.42), eta: 1.9 }
}

pub fn constant(v: f32) -> Arc<dyn Spectrum> {
    Arc::new(Constant(v))
}

/// Approximate first-hit albedo in linear sRGB for the denoiser's guide
/// buffer, evaluated at three representative wavelengths so it is noise-free.
pub fn guide_rgb(mat: &Material, cavity: f32) -> [f32; 3] {
    const L: [f32; 3] = [610.0, 550.0, 465.0];
    match mat {
        Material::Metal { ior, patina, .. } => {
            let mut c = L.map(|l| ior.r0(l));
            if let Some(p) = patina {
                let t = ((cavity - p.lo) / (p.hi - p.lo).max(1e-4)).clamp(0.0, 1.0);
                let w = p.strength * t * t * (3.0 - 2.0 * t);
                for i in 0..3 {
                    c[i] = c[i] * (1.0 - w) + p.base.eval(L[i]) * w;
                }
            }
            c
        }
        Material::Gem { .. } => [1.0; 3],
        Material::Plastic { base, .. } => L.map(|l| base.eval(l)),
        Material::Diffuse { albedo } => L.map(|l| albedo.eval(l)),
        Material::Black => [0.0; 3],
    }
}
