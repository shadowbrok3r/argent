//! Gemstone optics: dispersion (Sellmeier) and body colour (absorption).
//!
//! PLACEHOLDER: diamond and sapphire only, with flat or rough absorption. To
//! be replaced by a full table (see `names`), each with published Sellmeier
//! or Cauchy coefficients and an absorption spectrum in 1/mm that gives the
//! stone's familiar colour at a 3–6 mm path. Keep the API.

use crate::spectrum::{Constant, Piecewise, Spectrum};

/// How the renderer should treat the stone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GemKind {
    /// Clear or coloured transparent stone: refraction, dispersion, absorption.
    Transparent,
    /// Opal: milky body with play-of-colour (the renderer adds the flakes).
    Opal,
    /// Effectively opaque polished stone (onyx, obsidian, turquoise).
    Opaque,
}

pub struct GemOptics {
    pub name: &'static str,
    pub kind: GemKind,
    /// Refractive index n(λ).
    pub ior: Box<dyn Spectrum>,
    /// Absorption coefficient σa(λ) in 1/mm (Beer–Lambert inside the stone).
    pub absorption: Box<dyn Spectrum>,
    /// Reflectance colour for `Opaque` stones (diffuse base under a polished coat).
    pub body_albedo: Option<[f32; 3]>,
}

/// Sellmeier equation, `c` in µm², `λ` converted from nm.
#[derive(Clone, Copy, Debug)]
pub struct Sellmeier {
    pub b: [f32; 3],
    pub c: [f32; 3],
}

impl Spectrum for Sellmeier {
    fn eval(&self, lambda_nm: f32) -> f32 {
        let l2 = (lambda_nm * 1e-3).powi(2);
        let mut n2 = 1.0;
        for i in 0..3 {
            n2 += self.b[i] * l2 / (l2 - self.c[i]);
        }
        n2.max(1.0).sqrt()
    }
}

/// Cauchy: `n = a + b / λ²` with `λ` in µm.
#[derive(Clone, Copy, Debug)]
pub struct Cauchy {
    pub a: f32,
    pub b: f32,
}

impl Spectrum for Cauchy {
    fn eval(&self, lambda_nm: f32) -> f32 {
        let l = lambda_nm * 1e-3;
        self.a + self.b / (l * l)
    }
}

/// Look a stone up by name, case-insensitive (`"diamond"`, `"sapphire"`).
pub fn get(name: &str) -> Option<GemOptics> {
    let n = name.trim().to_ascii_lowercase();
    Some(match n.as_str() {
        "diamond" => GemOptics {
            name: "diamond",
            kind: GemKind::Transparent,
            ior: Box::new(Sellmeier { b: [4.3356, 0.3306, 0.0], c: [0.1060 * 0.1060, 0.1750 * 0.1750, 1.0] }),
            absorption: Box::new(Constant(0.0)),
            body_albedo: None,
        },
        "sapphire" | "blue-sapphire" => GemOptics {
            name: "sapphire",
            kind: GemKind::Transparent,
            ior: Box::new(Sellmeier {
                b: [1.431_349_3, 0.650_547_13, 5.341_402_1],
                c: [0.072_663_1f32.powi(2), 0.119_324_2f32.powi(2), 18.028_251f32.powi(2)],
            }),
            absorption: Box::new(Piecewise::new(&[
                (400.0, 0.15),
                (450.0, 0.08),
                (500.0, 0.25),
                (550.0, 0.6),
                (600.0, 0.9),
                (650.0, 1.0),
                (700.0, 0.9),
            ])),
            body_albedo: None,
        },
        _ => return None,
    })
}

/// Every name [`get`] accepts.
pub fn names() -> &'static [&'static str] {
    &["diamond", "sapphire"]
}
