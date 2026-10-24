//! Gemstone optics: dispersion (Sellmeier / Cauchy) and body colour
//! (absorption).
//!
//! # Dispersion
//!
//! Stones with a published dispersion formula use it (λ in µm, formulas as in
//! the refractiveindex.info database; birefringent stones use the ordinary
//! ray, the one a face-up stone mostly shows):
//!
//! | stone | formula | n_D | B–G |
//! |---|---|---|---|
//! | diamond | Peter 1923, Sellmeier | 2.417 | 0.044 |
//! | cubic zirconia | Wood & Nassau 1982 (12 mol % Y₂O₃), Sellmeier | 2.159 | 0.060 |
//! | moissanite | Wang et al. 2013, 4H-SiC ordinary, Sellmeier | 2.646 | 0.111 |
//! | sapphire, ruby, pink sapphire | Malitson & Dodge 1972, corundum ordinary | 1.768 | 0.018 |
//! | amethyst, citrine, smoky quartz | Ghosh 1999, α-quartz ordinary | 1.544 | 0.013 |
//! | spinel | Tropf & Thomas 1991 (Handbook of Optics 3rd ed.) | 1.716 | 0.020 |
//! | glass | Schott N-BK7 | 1.5168 (V_d 64.17) | 0.014 |
//!
//! Gem moissanite is grown as the 4H or 6H polytype; their ordinary indices
//! (both measured by Wang et al.) differ by < 0.002 in the visible, and the 4H
//! formula is a plain three-term Sellmeier valid down to 405 nm. The others
//! (beryl, olivine, topaz, garnets, zoisite, tourmaline, opal, the opaque
//! stones) have no Sellmeier fit in the literature, so each is a Cauchy
//! `n = A + B/λ²` through its gemological refractive index n_D (589.3 nm) and
//! "dispersion" n_G − n_B (430.8 nm and 686.7 nm, the gemologist's B–G
//! convention): [`Cauchy::from_gemological`]. Values are standard
//! gemological ones: refractive indices from the GIA/IGS tables (mid-range,
//! ordinary ray), B–G from the LibreTexts Gemology dispersion table (garnets
//! 0.027, peridot 0.020, tourmaline 0.017, topaz and beryl 0.014) and
//! GemSelect (tanzanite 0.030; the literature also gives 0.012–0.021). Opal
//! takes fused silica's B–G (Malitson 1965: 0.0115), being hydrated amorphous
//! silica; the opaque stones' dispersion is nominal (only their surface coat
//! sees it).
//!
//! # Absorption
//!
//! σa(λ) in 1/mm is a sum of absorption bands at their literature positions,
//! each a Gaussian in photon energy (the shape of crystal-field and
//! charge-transfer bands), optionally with different widths on its low- and
//! high-energy sides (Cr³⁺ vibronic bands rise steeply on the red side and
//! tail into the blue; a symmetric Gaussian turns ruby magenta), plus an
//! Urbach tail for the UV edge and a flat floor for broadband loss. Band
//! amplitudes and floors were then tuned, within ×0.25–×4 of physical
//! starting values, so that D65 light through a 4 mm path has each stone's
//! familiar colour (CIELAB L\*, C\*, h at 4 mm):
//!
//! | stone | bands (nm) | L\*, C\*, h° |
//! |---|---|---|
//! | sapphire | Fe²⁺–Ti⁴⁺ IVCT 565, Fe²⁺–Fe³⁺ ~700, Fe³⁺ 450/388/377 | 35, 72, 292 |
//! | ruby | Cr³⁺ ⁴T₂ 556, ⁴T₁ 405 | 36, 75, 5 |
//! | pink sapphire | ruby's Cr³⁺ bands at 1/7 strength | 76, 34, 343 |
//! | emerald | Cr³⁺/V³⁺ 610 and 430, Fe²⁺ 830 | 57, 50, 160 |
//! | aquamarine | Fe²⁺ 820, Fe³⁺ 427/370 | 89, 21, 207 |
//! | morganite | Mn³⁺ 495/560 | 89, 14, 30 |
//! | amethyst | Fe colour centre 545, 357 | 55, 55, 313 |
//! | citrine | Fe³⁺ charge-transfer edge | 83, 56, 90 |
//! | smoky quartz | Al-hole centre 460/620 | 42, 11, 62 |
//! | peridot | Fe²⁺ 850/1050/1250, weak 453/473/493 | 86, 55, 113 |
//! | blue topaz | irradiation colour centre 640 | 78, 34, 254 |
//! | garnet (almandine) | Fe²⁺ 505/527/576, 460, 615, 430 | 28, 45, 22 |
//! | tsavorite | V³⁺/Cr³⁺ 605 and 430 | 52, 58, 145 |
//! | spinel (red) | Cr³⁺ 545 and 395 | 41, 84, 12 |
//! | tanzanite | V³⁺ 595/530 | 38, 72, 293 |
//! | tourmaline (green) | Fe²⁺ 720, 470 | 55, 40, 148 |
//!
//! Colourless stones (diamond, cubic zirconia, moissanite, white topaz,
//! glass) stay neutral: diamond is a D-colour type IIa stone (no absorption),
//! moissanite keeps a faint Urbach tail of its 3.2 eV band gap, and glass uses
//! Schott's measured N-BK7 internal transmittance. Opaque stones absorb
//! 20/mm (nothing leaks through a volume path) and are rendered from
//! `body_albedo` under their dielectric coat; opals carry their body colour
//! there too (white: milky; black: dark body tone), matching argent-core's
//! opal material.
//!
//! These are colour models, not measurements: real stones vary with
//! concentration and cut far more than these tolerances.

// Published dispersion coefficients are kept verbatim, digits beyond f32 included.
#![allow(clippy::excessive_precision)]

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
    /// Reflectance colour for `Opaque` stones (diffuse base under a polished
    /// coat), and the body colour of opals; linear sRGB.
    pub body_albedo: Option<[f32; 3]>,
}

/// Fraunhofer lines, nm: F (H β), d (He), C (H α); B (O₂) and G (Fe/Ca) for
/// the gemological dispersion.
const LINE_F: f32 = 486.13;
const LINE_D: f32 = 587.56;
const LINE_C: f32 = 656.27;
const LINE_B: f32 = 686.7;
const LINE_G: f32 = 430.8;
/// Sodium D, the wavelength gemological refractive indices refer to.
const LINE_NA_D: f32 = 589.3;

impl GemOptics {
    /// Refractive index at the helium d line (587.56 nm).
    pub fn n_d(&self) -> f32 {
        self.ior.eval(LINE_D)
    }

    /// `n_F − n_C` (486.1 nm − 656.3 nm), the optical "fire" (mean
    /// dispersion). Diamond: 0.0256.
    pub fn fire(&self) -> f32 {
        self.ior.eval(LINE_F) - self.ior.eval(LINE_C)
    }

    /// `n_G − n_B` (430.8 nm − 686.7 nm), the figure gem references quote as
    /// "dispersion" (diamond 0.044, cubic zirconia 0.060).
    pub fn dispersion_bg(&self) -> f32 {
        self.ior.eval(LINE_G) - self.ior.eval(LINE_B)
    }

    /// Abbe number `V_d = (n_d − 1)/(n_F − n_C)`: lower means more fire
    /// (diamond 55, cubic zirconia 34, N-BK7 64).
    pub fn dispersion_abbe(&self) -> f32 {
        (self.n_d() - 1.0) / self.fire()
    }

    /// Colour of D65 light after `path_mm` through the stone (Beer–Lambert,
    /// no surface reflections), linear sRGB; white light stays (1, 1, 1).
    pub fn transmittance_srgb(&self, path_mm: f32) -> [f32; 3] {
        let d = path_mm.max(0.0);
        crate::cie::reflectance_srgb(|l| (-self.absorption.eval(l) * d).exp())
    }
}

/// Sellmeier equation, `n² = 1 + Σ bᵢ λ²/(λ² − cᵢ)`, `c` in µm², `λ`
/// converted from nm. A term with `c = 0` is a constant offset.
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

impl Cauchy {
    /// The Cauchy curve through a gemological refractive index `n_d` (sodium
    /// D, 589.3 nm) with gemological dispersion `bg = n_G − n_B`
    /// (430.8 nm − 686.7 nm).
    pub fn from_gemological(n_d: f32, bg: f32) -> Cauchy {
        let inv2 = |nm: f32| 1.0 / (nm * 1e-3).powi(2);
        let b = bg / (inv2(LINE_G) - inv2(LINE_B));
        Cauchy { a: n_d - b * inv2(LINE_NA_D), b }
    }
}

impl Spectrum for Cauchy {
    fn eval(&self, lambda_nm: f32) -> f32 {
        let l = lambda_nm * 1e-3;
        self.a + self.b / (l * l)
    }
}

/// hc in eV·nm.
const HC: f32 = 1_239.841_9;
/// FWHM → standard deviation.
const FWHM_TO_SIGMA: f32 = 1.0 / 2.354_82;

/// One absorption band: a Gaussian in photon energy centred at `center_nm`,
/// `fwhm_low_ev` wide on its low-energy (long-wavelength) side and
/// `fwhm_high_ev` on the other, peaking at `peak` 1/mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub center_nm: f32,
    pub fwhm_low_ev: f32,
    pub fwhm_high_ev: f32,
    pub peak: f32,
}

impl Band {
    /// A symmetric band.
    pub const fn new(center_nm: f32, fwhm_ev: f32, peak: f32) -> Band {
        Band { center_nm, fwhm_low_ev: fwhm_ev, fwhm_high_ev: fwhm_ev, peak }
    }
    /// An asymmetric (split-Gaussian) band.
    pub const fn split(center_nm: f32, fwhm_low_ev: f32, fwhm_high_ev: f32, peak: f32) -> Band {
        Band { center_nm, fwhm_low_ev, fwhm_high_ev, peak }
    }

    #[inline]
    fn eval_ev(&self, e: f32) -> f32 {
        let e0 = HC / self.center_nm;
        let w = if e < e0 { self.fwhm_low_ev } else { self.fwhm_high_ev } * FWHM_TO_SIGMA;
        let z = (e - e0) / w;
        self.peak * (-0.5 * z * z).exp()
    }
}

/// Urbach absorption edge: `at_edge · exp((E − E_edge)/slope_ev)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Urbach {
    pub edge_nm: f32,
    pub at_edge: f32,
    pub slope_ev: f32,
}

/// σa(λ) = `floor` + Σ `bands` + `urbach`, in 1/mm.
#[derive(Clone, Copy, Debug)]
pub struct Bands {
    pub floor: f32,
    pub bands: &'static [Band],
    pub urbach: Option<Urbach>,
}

impl Spectrum for Bands {
    fn eval(&self, lambda_nm: f32) -> f32 {
        let e = HC / lambda_nm.max(1.0);
        let mut s = self.floor;
        for b in self.bands {
            s += b.eval_ev(e);
        }
        if let Some(u) = self.urbach {
            // capped so the far UV cannot overflow
            s += u.at_edge * ((e - HC / u.edge_nm) / u.slope_ev).min(60.0).exp();
        }
        s.max(0.0)
    }
}

const OPAQUE_ABSORPTION: f32 = 20.0;

// Dispersion formulas.
const DIAMOND: Sellmeier = Sellmeier { b: [4.3356, 0.3306, 0.0], c: [0.011_236, 0.030_625, 0.0] };
const CUBIC_ZIRCONIA: Sellmeier =
    Sellmeier { b: [1.347_091, 2.117_788, 9.452_943], c: [0.003_911_627, 0.027_801_894, 591.490_1] };
const MOISSANITE: Sellmeier = Sellmeier { b: [0.200_75, 5.548_61, 35.650_66], c: [-12.072_24, 0.026_41, 1268.247] };
const CORUNDUM_O: Sellmeier =
    Sellmeier { b: [1.431_349_3, 0.650_547_13, 5.341_402_1], c: [0.005_279_926, 0.014_238_265, 325.017_8] };
const QUARTZ_O: Sellmeier = Sellmeier { b: [0.286_041_41, 1.070_440_8, 1.102_022_4], c: [0.0, 0.010_058_6, 100.0] };
const SPINEL: Sellmeier = Sellmeier { b: [1.8938, 3.0755, 0.0], c: [0.009_884_336, 250.462_28, 0.0] };
const N_BK7: Sellmeier =
    Sellmeier { b: [1.039_612_1, 0.231_792_34, 1.010_469_5], c: [0.006_000_698_7, 0.020_017_914, 103.560_65] };

// Absorption spectra (see the module docs).
const SAPPHIRE: Bands = Bands {
    floor: 0.0706,
    bands: &[
        Band::new(565.0, 0.62, 0.5615),
        Band::new(700.0, 0.50, 0.9784),
        Band::new(450.0, 0.08, 0.15),
        Band::new(388.0, 0.06, 0.20),
        Band::new(377.0, 0.06, 0.25),
    ],
    urbach: Some(Urbach { edge_nm: 360.0, at_edge: 0.3, slope_ev: 0.12 }),
};
const RUBY: Bands = Bands {
    floor: 0.005,
    bands: &[Band::split(556.0, 0.24, 0.50, 1.9751), Band::split(405.0, 0.40, 0.60, 2.9627)],
    urbach: Some(Urbach { edge_nm: 360.0, at_edge: 0.5, slope_ev: 0.15 }),
};
const PINK_SAPPHIRE: Bands = Bands {
    floor: 0.003,
    bands: &[Band::split(556.0, 0.24, 0.50, 0.2785), Band::split(405.0, 0.40, 0.60, 0.4177)],
    urbach: None,
};
const EMERALD: Bands = Bands {
    floor: 0.1395,
    bands: &[Band::split(610.0, 0.36, 0.44, 0.40), Band::split(430.0, 0.40, 0.50, 0.6177), Band::new(830.0, 0.55, 0.171)],
    urbach: None,
};
const AQUAMARINE: Bands = Bands {
    floor: 0.0324,
    bands: &[Band::new(820.0, 0.65, 0.764), Band::new(427.0, 0.10, 0.03), Band::new(370.0, 0.12, 0.06)],
    urbach: None,
};
const MORGANITE: Bands = Bands {
    floor: 0.0067,
    bands: &[Band::new(495.0, 0.45, 0.0976), Band::new(560.0, 0.35, 0.0329)],
    urbach: Some(Urbach { edge_nm: 400.0, at_edge: 0.18, slope_ev: 0.25 }),
};
const AMETHYST: Bands =
    Bands { floor: 0.0471, bands: &[Band::new(545.0, 0.65, 0.3908), Band::new(357.0, 0.50, 0.035)], urbach: None };
const CITRINE: Bands =
    Bands { floor: 0.0008, bands: &[], urbach: Some(Urbach { edge_nm: 420.0, at_edge: 0.8236, slope_ev: 0.36 }) };
const SMOKY_QUARTZ: Bands = Bands {
    floor: 0.0791,
    bands: &[Band::new(460.0, 0.90, 0.4317), Band::new(620.0, 0.75, 0.30)],
    urbach: Some(Urbach { edge_nm: 380.0, at_edge: 0.6, slope_ev: 0.25 }),
};
const PERIDOT: Bands = Bands {
    floor: 0.035,
    bands: &[
        Band::new(850.0, 0.55, 1.0),
        Band::new(1050.0, 0.55, 6.0),
        Band::new(1250.0, 0.55, 3.0),
        Band::new(453.0, 0.10, 0.20),
        Band::new(473.0, 0.10, 0.12),
        Band::new(493.0, 0.10, 0.18),
    ],
    urbach: Some(Urbach { edge_nm: 400.0, at_edge: 0.9, slope_ev: 0.25 }),
};
const BLUE_TOPAZ: Bands = Bands { floor: 0.0083, bands: &[Band::new(640.0, 0.65, 0.2855)], urbach: None };
const GARNET: Bands = Bands {
    floor: 0.0815,
    bands: &[
        Band::new(505.0, 0.10, 0.6),
        Band::new(527.0, 0.10, 1.6124),
        Band::new(576.0, 0.12, 2.25),
        Band::new(460.0, 0.35, 0.6986),
        Band::new(615.0, 0.15, 0.30),
        Band::new(430.0, 0.12, 0.20),
    ],
    urbach: Some(Urbach { edge_nm: 400.0, at_edge: 1.0, slope_ev: 0.25 }),
};
const TSAVORITE: Bands =
    Bands { floor: 0.1083, bands: &[Band::split(605.0, 0.42, 0.48, 0.478), Band::split(430.0, 0.42, 0.48, 1.192)], urbach: None };
const SPINEL_RED: Bands =
    Bands { floor: 0.008, bands: &[Band::split(545.0, 0.24, 0.50, 2.5937), Band::split(395.0, 0.40, 0.60, 3.8905)], urbach: None };
const TANZANITE: Bands =
    Bands { floor: 0.146, bands: &[Band::new(595.0, 0.38, 0.7046), Band::new(530.0, 0.35, 0.208)], urbach: None };
const TOURMALINE: Bands =
    Bands { floor: 0.0365, bands: &[Band::new(720.0, 0.55, 1.5), Band::new(470.0, 0.45, 0.72)], urbach: None };
/// 4H-SiC's indirect gap (3.26 eV, 380 nm): a faint violet tail.
const MOISSANITE_ABS: Bands =
    Bands { floor: 0.0005, bands: &[], urbach: Some(Urbach { edge_nm: 380.0, at_edge: 0.05, slope_ev: 0.08 }) };

/// Schott N-BK7 internal transmittance, as absorption `4πk/λ` (1/mm) from
/// the catalogue's tabulated k (refractiveindex.info `specs/schott`).
fn bk7_absorption() -> Piecewise {
    Piecewise::new(&[
        (350.0, 3.335e-3),
        (365.0, 1.177e-3),
        (370.0, 9.308e-4),
        (380.0, 6.859e-4),
        (390.0, 4.424e-4),
        (400.0, 3.213e-4),
        (405.0, 2.810e-4),
        (420.0, 2.810e-4),
        (436.0, 3.213e-4),
        (460.0, 2.810e-4),
        (500.0, 2.407e-4),
        (546.0, 1.603e-4),
        (580.0, 2.005e-4),
        (620.0, 2.407e-4),
        (660.0, 2.407e-4),
        (700.0, 1.603e-4),
        (1060.0, 1.202e-4),
    ])
}

fn gem(name: &'static str, kind: GemKind, ior: impl Spectrum + 'static, absorption: impl Spectrum + 'static) -> GemOptics {
    GemOptics { name, kind, ior: Box::new(ior), absorption: Box::new(absorption), body_albedo: None }
}

fn transparent(name: &'static str, ior: impl Spectrum + 'static, absorption: impl Spectrum + 'static) -> GemOptics {
    gem(name, GemKind::Transparent, ior, absorption)
}

fn opaque(name: &'static str, n_d: f32, bg: f32, albedo: [f32; 3]) -> GemOptics {
    GemOptics { body_albedo: Some(albedo), ..gem(name, GemKind::Opaque, Cauchy::from_gemological(n_d, bg), Constant(OPAQUE_ABSORPTION)) }
}

/// Look a stone up by name, case-insensitive; `-` and spaces count as `_`
/// (`"diamond"`, `"pink sapphire"`, `"Smoky-Quartz"`). Common aliases work
/// too (`cz`, `almandine`, `bk7`, `lapis_lazuli`, `white_opal`, …).
pub fn get(name: &str) -> Option<GemOptics> {
    let n = name.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    let g = |n_d, bg| Cauchy::from_gemological(n_d, bg);
    Some(match n.as_str() {
        "diamond" => transparent("diamond", DIAMOND, Constant(0.0)),
        "cubic_zirconia" | "cz" | "zirconia" => transparent("cubic_zirconia", CUBIC_ZIRCONIA, Constant(0.0005)),
        "moissanite" => transparent("moissanite", MOISSANITE, MOISSANITE_ABS),
        "sapphire" | "blue_sapphire" => transparent("sapphire", CORUNDUM_O, SAPPHIRE),
        "ruby" => transparent("ruby", CORUNDUM_O, RUBY),
        "pink_sapphire" => transparent("pink_sapphire", CORUNDUM_O, PINK_SAPPHIRE),
        "emerald" => transparent("emerald", g(1.580, 0.014), EMERALD),
        "aquamarine" => transparent("aquamarine", g(1.577, 0.014), AQUAMARINE),
        "morganite" => transparent("morganite", g(1.585, 0.014), MORGANITE),
        "amethyst" => transparent("amethyst", QUARTZ_O, AMETHYST),
        "citrine" => transparent("citrine", QUARTZ_O, CITRINE),
        "smoky_quartz" | "smoky" => transparent("smoky_quartz", QUARTZ_O, SMOKY_QUARTZ),
        "peridot" | "olivine" => transparent("peridot", g(1.672, 0.020), PERIDOT),
        "blue_topaz" => transparent("blue_topaz", g(1.613, 0.014), BLUE_TOPAZ),
        "white_topaz" | "topaz" => transparent("white_topaz", g(1.613, 0.014), Constant(0.0005)),
        "garnet" | "almandine" | "almandite" => transparent("garnet", g(1.790, 0.027), GARNET),
        "tsavorite" => transparent("tsavorite", g(1.740, 0.027), TSAVORITE),
        "spinel" | "red_spinel" => transparent("spinel", SPINEL, SPINEL_RED),
        "tanzanite" => transparent("tanzanite", g(1.694, 0.030), TANZANITE),
        "tourmaline" | "green_tourmaline" => transparent("tourmaline", g(1.635, 0.017), TOURMALINE),
        // Black onyx (dyed chalcedony), n 1.535–1.539.
        "onyx" | "black_onyx" => opaque("onyx", 1.537, 0.013, [0.012, 0.012, 0.013]),
        // Volcanic glass, n 1.48–1.51.
        "obsidian" => opaque("obsidian", 1.490, 0.010, [0.016, 0.015, 0.015]),
        // Robin's-egg turquoise, n 1.61–1.65: sRGB (80, 180, 175).
        "turquoise" => opaque("turquoise", 1.620, 0.010, [0.080, 0.456, 0.429]),
        // Lazurite-rich lapis, n ≈ 1.50: sRGB #26619C ("lapis lazuli").
        "lapis" | "lapis_lazuli" => opaque("lapis", 1.500, 0.010, [0.019, 0.120, 0.332]),
        // Hydrated amorphous silica, n 1.37–1.47: milky white body.
        "opal" | "white_opal" => GemOptics {
            body_albedo: Some([0.55, 0.58, 0.62]),
            ..gem("opal", GemKind::Opal, g(1.450, 0.0115), Constant(0.05))
        },
        // Dark body tone (N1–N2) under the play-of-colour.
        "black_opal" => GemOptics {
            body_albedo: Some([0.012, 0.012, 0.018]),
            ..gem("black_opal", GemKind::Opal, g(1.450, 0.0115), Constant(1.5))
        },
        "glass" | "bk7" | "n_bk7" | "crown_glass" => transparent("glass", N_BK7, bk7_absorption()),
        _ => return None,
    })
}

/// Every name [`get`] accepts (canonical names; aliases also resolve).
pub fn names() -> &'static [&'static str] {
    &[
        "diamond",
        "cubic_zirconia",
        "moissanite",
        "sapphire",
        "ruby",
        "pink_sapphire",
        "emerald",
        "aquamarine",
        "morganite",
        "amethyst",
        "citrine",
        "smoky_quartz",
        "peridot",
        "blue_topaz",
        "white_topaz",
        "garnet",
        "tsavorite",
        "spinel",
        "tanzanite",
        "tourmaline",
        "onyx",
        "obsidian",
        "turquoise",
        "lapis",
        "opal",
        "black_opal",
        "glass",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cie;

    fn lch(rgb: [f32; 3]) -> (f32, f32, f32) {
        let l = cie::xyz_to_lab(cie::linear_srgb_to_xyz(rgb));
        (l[0], l[1].hypot(l[2]), l[2].atan2(l[1]).to_degrees().rem_euclid(360.0))
    }

    /// Hue within `[lo, hi]` degrees, wrapping through 0.
    fn hue_in(h: f32, lo: f32, hi: f32) -> bool {
        if lo <= hi { (lo..=hi).contains(&h) } else { h >= lo || h <= hi }
    }

    #[test]
    fn every_name_resolves_and_is_sane() {
        for &n in names() {
            let g = get(n).unwrap_or_else(|| panic!("{n}"));
            assert_eq!(g.name, n);
            assert_eq!(get(&n.to_ascii_uppercase().replace('_', " ")).map(|g| g.name), Some(n));
            for l in (360..=830).step_by(10) {
                let l = l as f32;
                let ior = g.ior.eval(l);
                assert!((1.4..2.9).contains(&ior), "{n} n({l}) = {ior}");
                let a = g.absorption.eval(l);
                assert!(a.is_finite() && a >= 0.0, "{n} σ({l}) = {a}");
            }
            // normal dispersion: index falls with wavelength
            assert!(g.ior.eval(400.0) > g.ior.eval(700.0), "{n}");
            match g.kind {
                GemKind::Opaque | GemKind::Opal => assert!(g.body_albedo.is_some(), "{n}"),
                GemKind::Transparent => assert!(g.body_albedo.is_none(), "{n}"),
            }
        }
        assert!(get("cz").is_some() && get("almandine").is_some() && get("BK7").is_some());
        assert!(get("kryptonite").is_none());
    }

    #[test]
    fn dispersion_matches_the_references() {
        let near = |name: &str, f: fn(&GemOptics) -> f32, want: f32, tol: f32| {
            let v = f(&get(name).unwrap());
            assert!((v - want).abs() < tol, "{name}: {v} vs {want}");
        };
        near("diamond", |g| g.ior.eval(589.3), 2.417, 0.002);
        near("diamond", GemOptics::dispersion_bg, 0.044, 0.002);
        near("diamond", GemOptics::fire, 0.0256, 0.001);
        near("cubic_zirconia", GemOptics::dispersion_bg, 0.060, 0.003);
        near("cubic_zirconia", |g| g.ior.eval(589.3), 2.16, 0.01);
        near("moissanite", |g| g.ior.eval(589.3), 2.648, 0.005);
        near("moissanite", GemOptics::dispersion_bg, 0.104, 0.01);
        near("sapphire", |g| g.ior.eval(589.3), 1.768, 0.001);
        near("sapphire", GemOptics::dispersion_bg, 0.018, 0.001);
        near("amethyst", |g| g.ior.eval(589.3), 1.544, 0.001);
        near("spinel", GemOptics::dispersion_bg, 0.020, 0.001);
        near("glass", GemOptics::n_d, 1.5168, 2e-4);
        near("glass", GemOptics::dispersion_abbe, 64.17, 0.05);
        // Cauchy fits reproduce their gemological inputs
        for (name, n_d, bg) in [("emerald", 1.580, 0.014), ("garnet", 1.790, 0.027), ("tanzanite", 1.694, 0.030)] {
            near(name, |g| g.ior.eval(589.3), n_d, 1e-4);
            near(name, GemOptics::dispersion_bg, bg, 1e-4);
        }
        // fire ranks as jewellers expect
        let bg = |n: &str| get(n).unwrap().dispersion_bg();
        assert!(bg("moissanite") > bg("cubic_zirconia") && bg("cubic_zirconia") > bg("diamond"));
        assert!(bg("diamond") > bg("sapphire") && bg("sapphire") > bg("amethyst"));
    }

    #[test]
    fn coloured_stones_have_their_colour_at_4mm() {
        // (name, hue range in CIELAB degrees, L* range)
        type Case = (&'static str, (f32, f32), (f32, f32));
        let cases: &[Case] = &[
            ("sapphire", (280.0, 305.0), (25.0, 45.0)),
            ("ruby", (345.0, 25.0), (25.0, 45.0)),
            ("pink_sapphire", (330.0, 10.0), (65.0, 85.0)),
            ("emerald", (145.0, 175.0), (45.0, 70.0)),
            ("aquamarine", (195.0, 240.0), (80.0, 95.0)),
            ("morganite", (5.0, 60.0), (82.0, 95.0)),
            ("amethyst", (295.0, 330.0), (45.0, 65.0)),
            ("citrine", (70.0, 100.0), (75.0, 92.0)),
            ("smoky_quartz", (40.0, 90.0), (30.0, 55.0)),
            ("peridot", (100.0, 125.0), (78.0, 92.0)),
            ("blue_topaz", (230.0, 270.0), (68.0, 88.0)),
            ("garnet", (5.0, 40.0), (18.0, 38.0)),
            ("tsavorite", (130.0, 160.0), (42.0, 62.0)),
            ("spinel", (0.0, 30.0), (30.0, 50.0)),
            ("tanzanite", (280.0, 305.0), (28.0, 48.0)),
            ("tourmaline", (130.0, 165.0), (45.0, 65.0)),
        ];
        println!("{:<14} {:>22}   {:>5} {:>5} {:>6}", "stone", "4 mm linear sRGB", "L*", "C*", "h°");
        for &(name, (hlo, hhi), (llo, lhi)) in cases {
            let rgb = get(name).unwrap().transmittance_srgb(4.0);
            let (l, c, h) = lch(rgb);
            println!("{name:<14} {:>6.3} {:>6.3} {:>6.3}   {l:>5.1} {c:>5.1} {h:>6.1}", rgb[0], rgb[1], rgb[2]);
            assert!(hue_in(h, hlo, hhi), "{name}: hue {h} not in {hlo}..{hhi} ({rgb:?})");
            assert!((llo..=lhi).contains(&l), "{name}: L* {l} not in {llo}..{lhi}");
            assert!(c > 10.0, "{name}: chroma {c}");
        }
        // deeper with path length, as Beer-Lambert demands
        let s = get("sapphire").unwrap();
        assert!(lch(s.transmittance_srgb(8.0)).0 < lch(s.transmittance_srgb(4.0)).0);
    }

    #[test]
    fn colourless_stones_stay_neutral() {
        for name in ["diamond", "cubic_zirconia", "moissanite", "white_topaz", "glass"] {
            let rgb = get(name).unwrap().transmittance_srgb(4.0);
            let (l, c, _) = lch(rgb);
            assert!(l > 99.0 && c < 1.5, "{name}: L* {l} C* {c} {rgb:?}");
        }
        assert_eq!(get("diamond").unwrap().transmittance_srgb(10.0), cie::reflectance_srgb(|_| 1.0));
    }

    #[test]
    fn opaque_stones_block_light() {
        for name in ["onyx", "obsidian", "turquoise", "lapis"] {
            let g = get(name).unwrap();
            assert_eq!(g.kind, GemKind::Opaque);
            let t = g.transmittance_srgb(4.0);
            assert!(t.iter().all(|&v| v.abs() < 1e-6), "{name} {t:?}");
        }
        assert_eq!(get("black_opal").unwrap().kind, GemKind::Opal);
        let (w, b) = (get("opal").unwrap().body_albedo.unwrap(), get("black_opal").unwrap().body_albedo.unwrap());
        assert!(w.iter().zip(b.iter()).all(|(w, b)| w > b));
    }
}
