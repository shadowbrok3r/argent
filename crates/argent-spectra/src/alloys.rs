//! Jewelry alloys: the metals the studio actually casts and plates.
//!
//! # Model
//!
//! The first approximation of an alloy's optics is the linear (Vegard /
//! virtual-crystal) mix of the components' complex permittivities
//! ε = (n + ik)², weighted by **volume** fraction (mass fraction ÷ density):
//! ε − 1 is polarizability per unit volume, so volume is the right weight.
//! [`mix_permittivity`] does exactly that. Checked against measured colour it
//! is visibly wrong for the golds, because alloying moves the noble-metal d-band
//! absorption edge, which a weighted sum of two fixed edges cannot do:
//!
//! * ISO 8654 3N 18k yellow (75Au–12.5Ag–12.5Cu, the [`Alloy::Gold18kYellow`]
//!   composition) measures L\*a\*b\* = 89.30, 5.68, 22.45 and 5N red
//!   (75Au–4.5Ag–20.5Cu) 86.94, 9.60, 17.50 (US 10 683 570 B2, table 1; D65,
//!   d/8°, polished). Linear ε mixing of Johnson & Christy's metals gives
//!   ΔE₇₆ = 4.8 and 8.1 (too light, too yellow, not red enough);
//! * against Rioux et al. 2014's measured-model Au–Ag series, linear mixing is
//!   2.4–5.9 ΔE off (green golds too yellow);
//! * mixing copper with zinc cannot produce brass's yellow at all.
//!
//! Three physically motivated corrections fix the golds
//! ([`karat_gold`]); all are evaluated per wavelength on the 5 nm grid:
//!
//! 1. **Au–Ag excess permittivity**, `4 φ_Au φ_Ag Δε₅₀(λ)`, taken from Rioux,
//!    Vallières, Besner, Muñoz, Mazur & Meunier, Adv. Opt. Mater. 2, 176
//!    (2014): their composition-resolved Au–Ag dielectric functions depart
//!    from the linear mix by one spectral shape times 4x(1−x) (regular
//!    solution) to within 0.86–1.08 in amplitude, so the 50/50 excess
//!    transfers to any composition and onto Johnson & Christy's end members.
//! 2. **Au–Cu common d band.** Gold's and copper's interband edges (2.40 eV
//!    and 2.10 eV) merge in the solid solution; each component's interband
//!    permittivity (its ε minus its fitted Drude term) is shifted in photon
//!    energy to the volume-weighted edge `(φ_Au·2.40 + φ_Cu·2.10)/(φ_Au+φ_Cu)`.
//!    This is what makes copper-rich golds rosy rather than pale-yellow.
//! 3. **Alloy-disorder damping** (Matthiessen + Nordheim): the free-electron
//!    damping rises by `ħΔγ = ε₀(ħω_p)²Δρ/ħ` with residual resistivity
//!    `Δρ = 4 ρ_AuCu φ_Au φ_Cu`, ρ_AuCu = 20 µΩ·cm (fitted to the two ISO
//!    anchors; the DC value for disordered AuCu is ≈14 µΩ·cm, and optical
//!    damping in alloys runs higher). Au–Ag damping is already in Rioux's
//!    excess.
//!
//! With these the 3N anchor comes out at ΔE₇₆ 2.3 (CIE 1931 2°) / 0.7 (1964
//! 10°) and 5N at 3.0 / 2.5; for scale, the ISO nominal 5N (87.7, 8.32, 18.58)
//! and the measured 5N sample differ by 1.8. The remaining error is about the
//! spread between measurements, so no further tuning was done.
//!
//! Sterling is a solid-solution ε mix: as-cast sterling keeps most of its
//! copper in supersaturated α-silver; a fully two-phase (area-weighted
//! reflectance) model differs by less than 0.015 per channel. Brass is
//! Querry's measured 70Cu–30Zn directly (its F0 equals Hoffman's brass
//! reference to 1e-3). Bronze has no published visible optics; by the
//! Hume-Rothery rule the d-band edge of α-phase copper alloys follows the
//! electron concentration e/a (Zn gives 2 electrons, Sn 4), so 88Cu–12Sn
//! (6.80 at.% Sn, e/a = 1.204) is modelled as Johnson & Christy copper plus
//! the change Querry measured from copper to brass at that e/a (interpolated
//! between his 85/15 and 70/30 brasses), plus extra damping for tin's
//! stronger scattering (Linde's 2.88 µΩ·cm per at.% Sn against 0.30 for Zn:
//! +13.5 µΩ·cm, ħΔγ = 0.116 eV). The result is golden-brown, darker than
//! brass, between copper and brass in hue.
//!
//! # Normal-incidence colour (linear sRGB, D65, [`Alloy::f0_srgb`])
//!
//! | alloy | F0 | L\*, a\*, b\* (2°) |
//! |---|---|---|
//! | fine silver | 0.989, 0.984, 0.978 | 99.4, 0.1, 0.4 |
//! | .925 sterling | 0.990, 0.964, 0.927 | 98.7, 0.2, 2.4 |
//! | 24k | 1.040, 0.730, 0.363 | 90.3, 5.0, 34.0 |
//! | 18k yellow | 0.948, 0.734, 0.476 | 89.9, 3.4, 22.5 |
//! | 14k yellow (14/20 gold-filled) | 0.948, 0.748, 0.531 | 90.5, 3.7, 18.6 |
//! | 10k yellow | 0.958, 0.765, 0.580 | 91.4, 4.0, 15.8 |
//! | 14k rose | 0.937, 0.650, 0.486 | 86.9, 9.2, 17.5 |
//! | 14k white (Rh plate), rhodium | 0.797, 0.765, 0.742 | 90.3, 0.9, 2.0 |
//! | platinum | 0.679, 0.640, 0.584 | 84.2, 0.5, 4.8 |
//! | palladium | 0.733, 0.697, 0.652 | 87.1, 0.6, 3.7 |
//! | copper | 0.935, 0.620, 0.523 | 86.0, 12.3, 12.9 |
//! | bronze | 0.834, 0.672, 0.492 | 86.7, 3.1, 16.4 |
//! | brass | 0.910, 0.778, 0.423 | 90.8, −2.5, 28.6 |
//!
//! Compositions (mass): see [`Alloy::composition`].
//!
//! **14k yellow is 58.5Au–20.75Ag–20.75Cu**, chosen for the look of US 14K
//! yellow and 14/20 gold-filled stock, which the studio sells. Those alloys
//! are copper-rich with a little zinc (≈58.5Au–4–12Ag–26–32Cu–3–6Zn) and
//! measure a\* +1.0…+4.0, b\* +17.7…+20.5 (US 5 180 551 table 1A, twelve
//! 58.25Au alloys; US 6 676 776 reports a\* −0.9…+3.1, b\* +20…+22 for its
//! paler variants). The model has no zinc, and the equal-Ag:Cu ternary lands
//! inside that range: a\* 3.7, b\* 18.6, about 1.5 from the centre of the
//! US 5 180 551 range in the a\*b\* plane. The high-silver 58.5Au–30Ag–11.5Cu
//! first proposed is a pale, nearly hue-neutral 1N-type yellow, a\* 0.0,
//! b\* 16.9 (L\* 93.1), about 3.6 from that centre and outside the commercial
//! range. 10k yellow is 41.7Au with Ag:Cu 1:1 (29.15/29.15), the same yellow
//! balance at lower karat: a\* 4.0, b\* 15.8, paler than 14k as commercial 10K
//! is. (Densities stay the measured commercial values.)

use crate::data::alloys as tab;
use crate::metals::{self, ComplexIor, GRID_LEN, GRID_MIN, GRID_STEP};
use crate::spectrum::Spectrum;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Alloy {
    FineSilver,
    /// .925 sterling: 92.5% Ag, 7.5% Cu.
    Sterling,
    Gold24k,
    Gold18kYellow,
    /// 14k yellow, also the look of 14/20 gold-filled.
    Gold14kYellow,
    Gold10kYellow,
    Gold14kRose,
    /// Rhodium-plated white gold reads as rhodium.
    Gold14kWhite,
    Platinum,
    Palladium,
    Rhodium,
    Copper,
    /// Jewelers' bronze, ~88% Cu 12% Sn.
    Bronze,
    /// Yellow brass, ~70% Cu 30% Zn.
    Brass,
}

impl Alloy {
    pub const ALL: &'static [Alloy] = &[
        Alloy::FineSilver,
        Alloy::Sterling,
        Alloy::Gold24k,
        Alloy::Gold18kYellow,
        Alloy::Gold14kYellow,
        Alloy::Gold10kYellow,
        Alloy::Gold14kRose,
        Alloy::Gold14kWhite,
        Alloy::Platinum,
        Alloy::Palladium,
        Alloy::Rhodium,
        Alloy::Copper,
        Alloy::Bronze,
        Alloy::Brass,
    ];

    /// Optical constants for rendering (see the module docs for the model).
    pub fn ior(self) -> ComplexIor {
        let pure = |s: &str| metals::get(s).expect("known metal");
        match self {
            Alloy::FineSilver => pure("Ag"),
            Alloy::Gold24k => pure("Au"),
            Alloy::Gold14kWhite | Alloy::Rhodium => pure("Rh"),
            Alloy::Platinum => pure("Pt"),
            Alloy::Palladium => pure("Pd"),
            Alloy::Copper => pure("Cu"),
            Alloy::Sterling => {
                let (ag, cu) = (pure("Ag"), pure("Cu"));
                let v = volume_fractions(&[(0.925, DENSITY_AG), (0.075, DENSITY_CU)]);
                mix_permittivity(&[(v[0] as f32, &ag), (v[1] as f32, &cu)])
            }
            Alloy::Gold18kYellow | Alloy::Gold14kYellow | Alloy::Gold10kYellow | Alloy::Gold14kRose => {
                let mut w = [0.0f32; 3];
                for &(el, f) in self.composition() {
                    match el {
                        "Au" => w[0] = f,
                        "Ag" => w[1] = f,
                        "Cu" => w[2] = f,
                        _ => unreachable!("karat golds are Au-Ag-Cu"),
                    }
                }
                karat_gold(w[0], w[1], w[2])
            }
            Alloy::Brass => ComplexIor::from_grid(&tab::CU70ZN30_QUERRY),
            Alloy::Bronze => bronze(),
        }
    }

    /// Normal-incidence reflectance colour, linear sRGB under D65.
    pub fn f0_srgb(self) -> [f32; 3] {
        self.ior().f0_srgb()
    }

    /// Mass fractions of the surface the optics model (`[("Au", 0.75), …]`).
    /// 14k white is its rhodium plate; platinum and palladium are the pure
    /// metals (jewelry Pt950/Pd950 differ by a few percent of Ru/Ir/Co).
    pub fn composition(self) -> &'static [(&'static str, f32)] {
        match self {
            Alloy::FineSilver => &[("Ag", 1.0)],
            Alloy::Sterling => &[("Ag", 0.925), ("Cu", 0.075)],
            Alloy::Gold24k => &[("Au", 1.0)],
            Alloy::Gold18kYellow => &[("Au", 0.75), ("Ag", 0.125), ("Cu", 0.125)],
            Alloy::Gold14kYellow => &[("Au", 0.585), ("Ag", 0.2075), ("Cu", 0.2075)],
            Alloy::Gold10kYellow => &[("Au", 0.417), ("Ag", 0.2915), ("Cu", 0.2915)],
            Alloy::Gold14kRose => &[("Au", 0.585), ("Ag", 0.09), ("Cu", 0.325)],
            Alloy::Gold14kWhite | Alloy::Rhodium => &[("Rh", 1.0)],
            Alloy::Platinum => &[("Pt", 1.0)],
            Alloy::Palladium => &[("Pd", 1.0)],
            Alloy::Copper => &[("Cu", 1.0)],
            Alloy::Bronze => &[("Cu", 0.88), ("Sn", 0.12)],
            Alloy::Brass => &[("Cu", 0.70), ("Zn", 0.30)],
        }
    }

    /// Density in g/cm³, for weight estimates from mesh volume.
    pub fn density(self) -> f32 {
        match self {
            Alloy::FineSilver => 10.49,
            Alloy::Sterling => 10.36,
            Alloy::Gold24k => 19.32,
            Alloy::Gold18kYellow => 15.58,
            Alloy::Gold14kYellow => 13.07,
            Alloy::Gold10kYellow => 11.57,
            Alloy::Gold14kRose => 13.26,
            Alloy::Gold14kWhite => 12.61,
            Alloy::Platinum => 21.45,
            Alloy::Palladium => 12.02,
            Alloy::Rhodium => 12.41,
            Alloy::Copper => 8.96,
            Alloy::Bronze => 8.8,
            Alloy::Brass => 8.53,
        }
    }

    /// Stable machine name (`"sterling"`, `"gold14k"`, …).
    pub fn key(self) -> &'static str {
        match self {
            Alloy::FineSilver => "fine-silver",
            Alloy::Sterling => "sterling",
            Alloy::Gold24k => "gold24k",
            Alloy::Gold18kYellow => "gold18k",
            Alloy::Gold14kYellow => "gold14k",
            Alloy::Gold10kYellow => "gold10k",
            Alloy::Gold14kRose => "rose14k",
            Alloy::Gold14kWhite => "white14k",
            Alloy::Platinum => "platinum",
            Alloy::Palladium => "palladium",
            Alloy::Rhodium => "rhodium",
            Alloy::Copper => "copper",
            Alloy::Bronze => "bronze",
            Alloy::Brass => "brass",
        }
    }

    /// Display name (`".925 sterling silver"`).
    pub fn display(self) -> &'static str {
        match self {
            Alloy::FineSilver => "Fine silver",
            Alloy::Sterling => ".925 sterling silver",
            Alloy::Gold24k => "24k gold",
            Alloy::Gold18kYellow => "18k yellow gold",
            Alloy::Gold14kYellow => "14k yellow gold",
            Alloy::Gold10kYellow => "10k yellow gold",
            Alloy::Gold14kRose => "14k rose gold",
            Alloy::Gold14kWhite => "14k white gold",
            Alloy::Platinum => "Platinum",
            Alloy::Palladium => "Palladium",
            Alloy::Rhodium => "Rhodium",
            Alloy::Copper => "Copper",
            Alloy::Bronze => "Bronze",
            Alloy::Brass => "Brass",
        }
    }

    /// Parse a key or a common alias (`silver`, `gold`, `gold-filled`, `rose`).
    pub fn from_name(s: &str) -> Option<Alloy> {
        let s = s.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        let a = match s.as_str() {
            "silver" | "sterling" | "925" | "sterling-silver" => Alloy::Sterling,
            "fine-silver" | "999" => Alloy::FineSilver,
            "gold" | "yellow" | "gold14k" | "14k" | "gold-filled" | "gf" | "yellow-gold" => Alloy::Gold14kYellow,
            "gold24k" | "24k" => Alloy::Gold24k,
            "gold18k" | "18k" => Alloy::Gold18kYellow,
            "gold10k" | "10k" => Alloy::Gold10kYellow,
            "rose" | "rose-gold" | "rose14k" => Alloy::Gold14kRose,
            "white" | "white-gold" | "white14k" => Alloy::Gold14kWhite,
            "platinum" | "pt" => Alloy::Platinum,
            "palladium" | "pd" => Alloy::Palladium,
            "rhodium" | "rh" => Alloy::Rhodium,
            "copper" | "cu" => Alloy::Copper,
            "bronze" => Alloy::Bronze,
            "brass" => Alloy::Brass,
            _ => return None,
        };
        Some(a)
    }
}

const DENSITY_AU: f32 = 19.32;
const DENSITY_AG: f32 = 10.49;
const DENSITY_CU: f32 = 8.96;

/// Photon energy (eV) of a wavelength (nm) and back.
const HC_EV_NM: f64 = 1239.841_93;
/// ε₀/ħ in SI-per-eV units: ħΔγ[eV] = (ε₀/ħ)·(ħω_p[eV])²·Δρ[Ω·m].
const EPS0_OVER_HBAR: f64 = 8.854_187_812_8e-12 / 6.582_119_569e-16;

/// Interband edges used for the Au–Cu common band (eV).
const EDGE_AU: f64 = 2.40;
const EDGE_CU: f64 = 2.10;
/// 50/50 residual resistivity used for Au–Cu disorder damping (Ω·m).
const RHO_AU_CU: f64 = 20e-8;

/// Free-electron parameters `(ħω_p, ħγ)` in eV, least-squares Drude fits
/// (ε∞ free) to each 5 nm table over the Drude regime (Au, Cu 650–830 nm;
/// Ag 450–830 nm). Used to split ε into free-electron and interband parts.
const DRUDE_AU: (f64, f64) = (8.849, 0.0735);
const DRUDE_AG: (f64, f64) = (9.146, 0.0255);
const DRUDE_CU: (f64, f64) = (9.087, 0.1180);

#[derive(Clone, Copy, Debug, PartialEq)]
struct Cx {
    re: f64,
    im: f64,
}

impl Cx {
    const ZERO: Cx = Cx { re: 0.0, im: 0.0 };
    fn new(re: f64, im: f64) -> Cx {
        Cx { re, im }
    }
    /// ε = (n + ik)²
    fn from_nk(n: f32, k: f32) -> Cx {
        let (n, k) = (n as f64, k as f64);
        Cx::new(n * n - k * k, 2.0 * n * k)
    }
    fn add(self, o: Cx) -> Cx {
        Cx::new(self.re + o.re, self.im + o.im)
    }
    fn sub(self, o: Cx) -> Cx {
        Cx::new(self.re - o.re, self.im - o.im)
    }
    fn scale(self, s: f64) -> Cx {
        Cx::new(self.re * s, self.im * s)
    }
    /// Principal square root: `n + ik` with n ≥ 0 (and k ≥ 0 when Im ε ≥ 0).
    fn sqrt(self) -> Cx {
        let m = self.re.hypot(self.im);
        let n = (0.5 * (m + self.re)).max(0.0).sqrt();
        let k = (0.5 * (m - self.re)).max(0.0).sqrt();
        Cx::new(n, if self.im < 0.0 { -k } else { k })
    }
}

type Grid = [Cx; GRID_LEN];

fn grid_lambda(i: usize) -> f64 {
    (GRID_MIN + GRID_STEP * i as f32) as f64
}

fn eps_of(rows: &[(f32, f32); GRID_LEN]) -> Grid {
    let mut g = [Cx::ZERO; GRID_LEN];
    for (o, &(n, k)) in g.iter_mut().zip(rows.iter()) {
        *o = Cx::from_nk(n, k);
    }
    g
}

/// Drude free-electron term `−ω_p²/(E² + iγE)` at photon energy `e` (eV).
fn drude(wp: f64, g: f64, e: f64) -> Cx {
    let d = e * e + g * g;
    Cx::new(-wp * wp / d, wp * wp * g / (e * d))
}

/// Interband part: ε − (1 + Drude).
fn interband(eps: &Grid, (wp, g): (f64, f64)) -> Grid {
    let mut out = *eps;
    for (i, o) in out.iter_mut().enumerate() {
        let e = HC_EV_NM / grid_lambda(i);
        *o = o.sub(drude(wp, g, e)).sub(Cx::new(1.0, 0.0));
    }
    out
}

/// Linear interpolation of a grid at `lambda` nm, clamped to the grid.
fn at(g: &Grid, lambda: f64) -> Cx {
    let x = ((lambda - GRID_MIN as f64) / GRID_STEP as f64).clamp(0.0, (GRID_LEN - 1) as f64);
    let i = (x as usize).min(GRID_LEN - 2);
    let t = x - i as f64;
    g[i].scale(1.0 - t).add(g[i + 1].scale(t))
}

fn to_ior(eps: &Grid) -> ComplexIor {
    let rows: Vec<(f32, f32)> = eps
        .iter()
        .map(|e| {
            let nk = e.sqrt();
            ((nk.re as f32).max(1e-4), (nk.im as f32).max(0.0))
        })
        .collect();
    ComplexIor::from_grid(&rows)
}

/// Volume fractions from `(mass fraction, density)` pairs (normalized).
fn volume_fractions<const K: usize>(parts: &[(f32, f32); K]) -> [f64; K] {
    let mut v = [0.0f64; K];
    for (o, &(m, rho)) in v.iter_mut().zip(parts.iter()) {
        *o = (m.max(0.0) / rho) as f64;
    }
    let s: f64 = v.iter().sum();
    if s > 0.0 {
        for o in v.iter_mut() {
            *o /= s;
        }
    }
    v
}

/// Linear mixing of complex permittivities, `ε = Σ φᵢ (nᵢ + ikᵢ)²`, with
/// volume fractions `φᵢ` (normalized here), evaluated on the 5 nm grid
/// 360..=830 nm. The plain virtual-crystal first approximation of an alloy;
/// see the module docs for where it fails.
pub fn mix_permittivity(parts: &[(f32, &ComplexIor)]) -> ComplexIor {
    let total: f64 = parts.iter().map(|p| p.0.max(0.0) as f64).sum();
    let mut eps = [Cx::ZERO; GRID_LEN];
    for (i, e) in eps.iter_mut().enumerate() {
        let l = grid_lambda(i) as f32;
        for &(f, m) in parts {
            let w = if total > 0.0 { f.max(0.0) as f64 / total } else { 0.0 };
            *e = e.add(Cx::from_nk(m.eta.eval(l), m.k.eval(l)).scale(w));
        }
    }
    to_ior(&eps)
}

/// A karat gold of the given **mass** fractions of gold, silver and copper
/// (normalized; `karat_gold(0.75, 0.125, 0.125)` is 18k 3N yellow), with the
/// calibrated Au–Ag–Cu model of the module docs.
pub fn karat_gold(au: f32, ag: f32, cu: f32) -> ComplexIor {
    if au.max(0.0) + ag.max(0.0) + cu.max(0.0) <= 0.0 {
        return metals::get("Au").expect("gold");
    }
    let [fau, fag, fcu] = volume_fractions(&[(au, DENSITY_AU), (ag, DENSITY_AG), (cu, DENSITY_CU)]);
    let e_au = eps_of(metals::grid_rows("Au"));
    let e_ag = eps_of(metals::grid_rows("Ag"));
    let e_cu = eps_of(metals::grid_rows("Cu"));
    let mut eps = [Cx::ZERO; GRID_LEN];
    for i in 0..GRID_LEN {
        let (re, im) = tab::AU_AG_EXCESS_50[i];
        let excess = Cx::new(re as f64, im as f64).scale(4.0 * fau * fag);
        eps[i] = e_au[i].scale(fau).add(e_ag[i].scale(fag)).add(e_cu[i].scale(fcu)).add(excess);
    }
    if fau > 0.0 && fcu > 0.0 {
        let edge = (fau * EDGE_AU + fcu * EDGE_CU) / (fau + fcu);
        for (f, e_m, drude_m, edge_m) in [(fau, &e_au, DRUDE_AU, EDGE_AU), (fcu, &e_cu, DRUDE_CU, EDGE_CU)] {
            let ib = interband(e_m, drude_m);
            let shift = edge - edge_m;
            for i in 0..GRID_LEN {
                let e = HC_EV_NM / grid_lambda(i);
                let moved = at(&ib, HC_EV_NM / (e - shift));
                eps[i] = eps[i].add(moved.sub(ib[i]).scale(f));
            }
        }
    }
    let wp2 = fau * DRUDE_AU.0.powi(2) + fag * DRUDE_AG.0.powi(2) + fcu * DRUDE_CU.0.powi(2);
    let g0 = fau * DRUDE_AU.1 + fag * DRUDE_AG.1 + fcu * DRUDE_CU.1;
    let dg = EPS0_OVER_HBAR * wp2 * 4.0 * RHO_AU_CU * fau * fcu;
    if dg > 0.0 {
        let wp = wp2.sqrt();
        for (i, e) in eps.iter_mut().enumerate() {
            let en = HC_EV_NM / grid_lambda(i);
            *e = e.add(drude(wp, g0 + dg, en)).sub(drude(wp, g0, en));
        }
    }
    to_ior(&eps)
}

/// 88Cu–12Sn bronze by electron-concentration equivalence with Querry's
/// brasses (module docs).
fn bronze() -> ComplexIor {
    const M_CU: f64 = 63.546;
    const M_SN: f64 = 118.71;
    const M_ZN: f64 = 65.38;
    let x_sn = (0.12 / M_SN) / (0.12 / M_SN + 0.88 / M_CU);
    let ea = 1.0 + 3.0 * x_sn;
    let ea_brass = |zn: f64| 1.0 + (zn / M_ZN) / (zn / M_ZN + (1.0 - zn) / M_CU);
    let (ea15, ea30) = (ea_brass(0.15), ea_brass(0.30));
    let t = (ea - ea15) / (ea30 - ea15);
    let q_cu = eps_of(&tab::CU_QUERRY);
    let q15 = eps_of(&tab::CU85ZN15_QUERRY);
    let q30 = eps_of(&tab::CU70ZN30_QUERRY);
    let jc_cu = eps_of(metals::grid_rows("Cu"));
    // Tin scatters harder than the zinc it stands in for (Linde): residual
    // resistivity per at.% in copper, µΩ·cm.
    let drho = (x_sn * 100.0 * 2.88 - (ea - 1.0) * 100.0 * 0.30) * 1e-8;
    let (wp, g) = DRUDE_CU;
    let dg = EPS0_OVER_HBAR * wp * wp * drho;
    let mut eps = [Cx::ZERO; GRID_LEN];
    for i in 0..GRID_LEN {
        let brass = q15[i].scale(1.0 - t).add(q30[i].scale(t));
        let en = HC_EV_NM / grid_lambda(i);
        eps[i] = jc_cu[i].add(brass.sub(q_cu[i])).add(drude(wp, g + dg, en)).sub(drude(wp, g, en));
    }
    to_ior(&eps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cie;

    fn lab(rgb: [f32; 3]) -> [f32; 3] {
        cie::xyz_to_lab(cie::linear_srgb_to_xyz(rgb))
    }
    fn hue(l: [f32; 3]) -> f32 {
        l[2].atan2(l[1]).to_degrees().rem_euclid(360.0)
    }
    fn chroma(l: [f32; 3]) -> f32 {
        l[1].hypot(l[2])
    }
    fn de(a: [f32; 3], b: [f32; 3]) -> f32 {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    #[test]
    fn f0_table_is_plausible() {
        println!("{:<22} {:>24}   {:>20}", "alloy", "F0 linear sRGB (D65)", "L* a* b*");
        let mut f0 = std::collections::HashMap::new();
        for &a in Alloy::ALL {
            let c = a.f0_srgb();
            let l = lab(c);
            println!(
                "{:<22} {:>7.3} {:>7.3} {:>7.3}   {:>6.1} {:>6.1} {:>6.1}",
                a.display(),
                c[0],
                c[1],
                c[2],
                l[0],
                l[1],
                l[2]
            );
            assert!(c.iter().all(|v| v.is_finite() && *v > 0.3 && *v < 1.1), "{a:?} {c:?}");
            f0.insert(a, (c, l));
        }
        let g = |a: Alloy| f0[&a];
        // 14k (and 18k) paler than 24k: less chroma, more blue
        assert!(chroma(g(Alloy::Gold14kYellow).1) < chroma(g(Alloy::Gold18kYellow).1));
        assert!(chroma(g(Alloy::Gold18kYellow).1) < chroma(g(Alloy::Gold24k).1));
        assert!(g(Alloy::Gold14kYellow).0[2] > g(Alloy::Gold24k).0[2]);
        // rose redder than every yellow
        for y in [Alloy::Gold24k, Alloy::Gold18kYellow, Alloy::Gold14kYellow, Alloy::Gold10kYellow] {
            assert!(g(Alloy::Gold14kRose).1[1] > g(y).1[1], "{y:?}");
            assert!(g(Alloy::Gold14kRose).0[0] / g(Alloy::Gold14kRose).0[1] > g(y).0[0] / g(y).0[1] - 0.06, "{y:?}");
        }
        // bronze between copper and brass in hue, and in the green channel
        let (hc, hb, hz) = (hue(g(Alloy::Copper).1), hue(g(Alloy::Bronze).1), hue(g(Alloy::Brass).1));
        assert!(hc < hb && hb < hz, "{hc} {hb} {hz}");
        let (gc, gb, gz) = (g(Alloy::Copper).0[1], g(Alloy::Bronze).0[1], g(Alloy::Brass).0[1]);
        assert!(gc < gb && gb < gz);
        // sterling ≈ silver, a touch warmer and darker
        let (s, ag) = (g(Alloy::Sterling).0, g(Alloy::FineSilver).0);
        for k in 0..3 {
            assert!((s[k] - ag[k]).abs() < 0.06 && s[k] <= ag[k] + 1e-3, "{s:?} {ag:?}");
        }
        assert!(g(Alloy::Sterling).1[2] > g(Alloy::FineSilver).1[2]);
        // white metals are near-neutral
        for w in [Alloy::Gold14kWhite, Alloy::Platinum, Alloy::Palladium, Alloy::Rhodium, Alloy::FineSilver] {
            assert!(chroma(g(w).1) < 6.0, "{w:?}");
        }
    }

    #[test]
    fn measured_anchors() {
        // ISO 8654 3N (= Gold18kYellow) and 5N, US 10 683 570 B2 table 1.
        let n3 = lab(Alloy::Gold18kYellow.f0_srgb());
        let n5 = lab(karat_gold(0.75, 0.045, 0.205).f0_srgb());
        assert!(de(n3, [89.30, 5.68, 22.45]) < 3.0, "3N {n3:?}");
        assert!(de(n5, [86.94, 9.60, 17.50]) < 3.5, "5N {n5:?}");
        // the plain linear mix is clearly worse (why the model has corrections)
        let (au, ag, cu) = (metals::get("Au").unwrap(), metals::get("Ag").unwrap(), metals::get("Cu").unwrap());
        let v = volume_fractions(&[(0.75, DENSITY_AU), (0.045, DENSITY_AG), (0.205, DENSITY_CU)]);
        let lin = lab(mix_permittivity(&[(v[0] as f32, &au), (v[1] as f32, &ag), (v[2] as f32, &cu)]).f0_srgb());
        assert!(de(lin, [86.94, 9.60, 17.50]) > 6.0, "linear 5N {lin:?}");
        // brass is Querry's measured alloy: Hoffman's reference F0
        let b = Alloy::Brass.f0_srgb();
        for (k, want) in [0.910f32, 0.778, 0.423].into_iter().enumerate() {
            assert!((b[k] - want).abs() < 5e-3, "{b:?}");
        }
    }

    #[test]
    fn pure_end_members_are_the_measured_metals() {
        let same = |a: &ComplexIor, b: &ComplexIor| {
            for l in (360..=830).step_by(7) {
                let l = l as f32;
                assert!((a.eta.eval(l) - b.eta.eval(l)).abs() < 1e-4 && (a.k.eval(l) - b.k.eval(l)).abs() < 1e-4, "{l}");
            }
        };
        same(&karat_gold(1.0, 0.0, 0.0), &metals::get("Au").unwrap());
        same(&karat_gold(0.0, 1.0, 0.0), &metals::get("Ag").unwrap());
        same(&karat_gold(0.0, 0.0, 1.0), &metals::get("Cu").unwrap());
        same(&Alloy::Gold24k.ior(), &metals::get("Au").unwrap());
        let cu = metals::get("Cu").unwrap();
        same(&mix_permittivity(&[(1.0, &cu)]), &cu);
        assert_eq!(Alloy::Gold24k.f0_srgb(), metals::get("Au").unwrap().f0_srgb());
    }

    #[test]
    fn every_alloy_has_physical_optics() {
        for &a in Alloy::ALL {
            let m = a.ior();
            for ((_, n), (_, k)) in m.eta.samples().zip(m.k.samples()) {
                assert!(n > 0.0 && k > 0.0 && n.is_finite() && k.is_finite(), "{a:?}");
            }
            let s: f32 = a.composition().iter().map(|c| c.1).sum();
            assert!((s - 1.0).abs() < 1e-4, "{a:?}");
            assert_eq!(Alloy::from_name(a.key()), Some(a));
        }
    }

    #[test]
    fn gold_colour_moves_the_right_way_with_composition() {
        // more silver → greener (a* falls), more copper → redder (a* rises)
        let a = |au, ag, cu| lab(karat_gold(au, ag, cu).f0_srgb())[1];
        assert!(a(0.75, 0.25, 0.0) < a(0.75, 0.125, 0.125));
        assert!(a(0.75, 0.125, 0.125) < a(0.75, 0.0, 0.25));
        assert!(a(0.585, 0.30, 0.115) < a(0.585, 0.09, 0.325));
    }
}
