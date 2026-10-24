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
    /// Precious opal (crushed-opal inlay or cabochon): Voronoi domains of
    /// diffraction lattices over a dark (`black`) or milky body.
    Opal { body: Arc<dyn Spectrum>, cell_mm: f32, strength: f32 },
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
        let lname = name.to_ascii_lowercase();
        if lname.contains("opal") {
            return Some(Material::opal(lname.contains("black") || lname == "opal", 0.35));
        }
        let g = gems::get(name)?;
        let opaque_base: Option<Arc<dyn Spectrum>> = g.body_albedo.map(|c| Arc::new(RgbAlbedo::new(c)) as Arc<dyn Spectrum>);
        Some(Material::Gem { ior: Arc::from(g.ior), absorption: Arc::from(g.absorption), kind: g.kind, opaque_base })
    }

    /// Black opal (dark body, vivid flashes) or white opal (milky body).
    pub fn opal(black: bool, cell_mm: f32) -> Material {
        let body = if black { [0.012, 0.012, 0.018] } else { [0.55, 0.58, 0.62] };
        Material::Opal { body: Arc::new(RgbAlbedo::new(body)), cell_mm, strength: if black { 1.0 } else { 0.7 } }
    }

    /// True when rays pass through (two-sided shading must not flip normals).
    pub fn is_transmissive(&self) -> bool {
        matches!(self, Material::Gem { kind: GemKind::Transparent, .. })
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
    pub fn bsdf(&self, wl: &Wavelengths, mask: f32, u: f32, ctx: &HitCtx) -> Bsdf {
        match self {
            Material::Opal { body, cell_mm, strength } => {
                let d = opal_domain(ctx.p_obj / *cell_mm);
                // Lattice normals lean toward the surface normal: chips are
                // polished flat, so most domains face roughly outward.
                let nw = (ctx.n_world * 1.2 + crate::geom::rot_y(d.dir, ctx.turn_deg)).normalize();
                let n = ctx.frame.to_local(nw);
                let n = if n.z < 0.0 { -n } else { n };
                Bsdf::Opal {
                    n,
                    ggx: Ggx::from_roughness(0.22),
                    spacing_nm: d.spacing,
                    strength: *strength * d.brightness,
                    lambda: Spec(wl.lambda),
                    body: body.sample(wl),
                    w: 0.6,
                }
            }
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
        Material::Opal { body, .. } => L.map(|l| body.eval(l)),
        Material::Plastic { base, .. } => L.map(|l| base.eval(l)),
        Material::Diffuse { albedo } => L.map(|l| albedo.eval(l)),
        Material::Black => [0.0; 3],
    }
}

/// What a material may need to know about the hit beyond wavelengths.
pub struct HitCtx<'a> {
    /// Position in the piece's own frame (moves with the turntable).
    pub p_obj: crate::geom::V3,
    pub n_world: crate::geom::V3,
    pub frame: &'a crate::geom::Frame,
    /// Turntable angle, to carry object-space directions into the world.
    pub turn_deg: f32,
}

struct OpalDomain {
    dir: crate::geom::V3,
    spacing: f32,
    brightness: f32,
}

#[inline]
fn hash3(i: i32, j: i32, k: i32, salt: u32) -> u32 {
    let mut h = (i as u32).wrapping_mul(0x8da6_b343) ^ (j as u32).wrapping_mul(0xd816_3841) ^ (k as u32).wrapping_mul(0xcb1a_b31f) ^ salt;
    h = crate::sampler::hash32(h);
    h
}

#[inline]
fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Jittered-grid Voronoi: the domain containing `p` (in cell units).
fn opal_domain(p: crate::geom::V3) -> OpalDomain {
    let (fx, fy, fz) = (p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32);
    let mut best = (f32::MAX, 0i32, 0i32, 0i32);
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (i, j, k) = (fx + dx, fy + dy, fz + dz);
                let c = crate::geom::V3::new(
                    i as f32 + unit(hash3(i, j, k, 1)),
                    j as f32 + unit(hash3(i, j, k, 2)),
                    k as f32 + unit(hash3(i, j, k, 3)),
                );
                let d2 = (c - p).length_squared();
                if d2 < best.0 {
                    best = (d2, i, j, k);
                }
            }
        }
    }
    let (_, i, j, k) = best;
    let z = 2.0 * unit(hash3(i, j, k, 4)) - 1.0;
    let phi = std::f32::consts::TAU * unit(hash3(i, j, k, 5));
    let r = (1.0 - z * z).max(0.0).sqrt();
    OpalDomain {
        dir: crate::geom::V3::new(r * phi.cos(), r * phi.sin(), z),
        // 190–265 nm planes put first-order peaks across the visible.
        spacing: 190.0 + 75.0 * unit(hash3(i, j, k, 6)),
        brightness: 0.35 + 0.65 * unit(hash3(i, j, k, 7)).powf(0.7),
    }
}
