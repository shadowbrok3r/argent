//! Measured complex refractive indices of pure metals.
//!
//! Every metal is a table of `n + ik` on one common grid, 360..=830 nm at
//! 5 nm, linearly interpolated between rows (and held flat outside). The
//! sources tabulate at their own spacing (Johnson & Christy every ~20–65 nm,
//! Rakić's model tables every 3–16 nm, Querry every 10–40 nm); each was
//! resampled to the grid with a monotone cubic (Fritsch–Carlson PCHIP) in
//! wavelength, which follows the measured points without overshoot. Against
//! plain linear interpolation of the source points the normal-incidence
//! colour changes by at most 0.004 in any linear-sRGB channel. Data were taken
//! from the refractiveindex.info database (Polyanskiy, `database/data/main/
//! <El>/nk/`), whose YAML files carry the original references.
//!
//! | metal | source | normal-incidence colour (linear sRGB, D65) |
//! |---|---|---|
//! | Ag | Johnson & Christy 1972 | 0.989, 0.984, 0.978 |
//! | Au | Johnson & Christy 1972 | 1.040, 0.730, 0.363 |
//! | Cu | Johnson & Christy 1972 | 0.935, 0.620, 0.523 |
//! | Pt | Rakić et al. 1998, Brendel–Bormann | 0.679, 0.640, 0.584 |
//! | Pd | Johnson & Christy 1974 | 0.733, 0.697, 0.652 |
//! | Rh | Weaver, Olson & Lynch 1977 (Palik) | 0.797, 0.765, 0.742 |
//! | Ni | Rakić et al. 1998, Brendel–Bormann | 0.649, 0.608, 0.534 |
//! | Ti | Johnson & Christy 1974 | 0.619, 0.579, 0.542 |
//! | Al | Rakić et al. 1998, Brendel–Bormann | 0.912, 0.914, 0.920 |
//! | Cr | Rakić et al. 1998, Brendel–Bormann | 0.639, 0.652, 0.664 |
//! | Fe | Querry 1985 (polished bulk) | 0.575, 0.567, 0.563 |
//! | Zn | Querry 1987 reflectance, Gulbrandsen n,k | 0.743, 0.769, 0.769 |
//! | Sn | Drude fit to Golovashkin & Motulevich 1964 (IR), extrapolated | 0.815, 0.812, 0.809 |
//!
//! These agree with Naty Hoffman's published linear F0 values (SIGGRAPH 2015
//! "Physics and Math of Shading"; Real-Time Rendering 4th ed. table 9.2) to
//! within a few hundredths where both exist: Pd and Cr-from-Johnson exactly,
//! Pt and Ni (Rakić) within 0.01, Cu within 0.02. Johnson & Christy's gold is
//! a little less yellow-green than more recent films (McPeak 2015, Olmon 2012
//! give G ≈ 0.76–0.78 against 0.73 here) and their silver a little brighter in
//! the blue than Hoffman's 0.915; both are kept because they are the
//! reference measurements the rest of the literature compares against.
//!
//! Rejected: Werner et al. 2009 (REELS-derived) gives normal-incidence
//! reflectances of 0.83–0.97 for Fe, Ti, Ni, Pt and Pd, far above every
//! optical measurement (≈0.55–0.73); electron energy loss constrains the
//! high-energy loss function, not visible-light reflectance. Rakić's
//! Brendel–Bormann titanium tilts blue (B > R), unlike polished titanium, so
//! Ti uses Johnson & Christy 1974. Chromium uses Rakić (fitted to Palik's bulk
//! data, R ≈ 0.65, the bright bluish chrome of plating) rather than Johnson &
//! Christy's evaporated films (R ≈ 0.55).

use crate::data::metals as tab;
use crate::spec::{Spec, Wavelengths, N};
use crate::spectrum::{Piecewise, Spectrum};

/// The common wavelength grid of the tables: `GRID_MIN + GRID_STEP·i`, nm.
pub const GRID_MIN: f32 = 360.0;
pub const GRID_STEP: f32 = 5.0;
pub const GRID_LEN: usize = 95;

/// `n + i k` as functions of wavelength.
#[derive(Clone, Debug)]
pub struct ComplexIor {
    pub eta: Piecewise,
    pub k: Piecewise,
}

impl ComplexIor {
    pub fn new(eta: Piecewise, k: Piecewise) -> Self {
        ComplexIor { eta, k }
    }

    /// From `(nm, n, k)` rows.
    pub fn from_table(rows: &[(f32, f32, f32)]) -> Self {
        let eta: Vec<(f32, f32)> = rows.iter().map(|r| (r.0, r.1)).collect();
        let k: Vec<(f32, f32)> = rows.iter().map(|r| (r.0, r.2)).collect();
        ComplexIor { eta: Piecewise::new(&eta), k: Piecewise::new(&k) }
    }

    /// From `(n, k)` rows on the common 5 nm grid ([`GRID_MIN`], [`GRID_STEP`]).
    pub fn from_grid(rows: &[(f32, f32)]) -> Self {
        let rows: Vec<(f32, f32, f32)> =
            rows.iter().enumerate().map(|(i, &(n, k))| (GRID_MIN + GRID_STEP * i as f32, n, k)).collect();
        ComplexIor::from_table(&rows)
    }

    #[inline]
    pub fn sample(&self, wl: &Wavelengths) -> (Spec, Spec) {
        let mut e = [0.0; N];
        let mut k = [0.0; N];
        for i in 0..N {
            e[i] = self.eta.eval(wl.lambda[i]);
            k[i] = self.k.eval(wl.lambda[i]);
        }
        (Spec(e), Spec(k))
    }

    /// Reflectance at normal incidence in air.
    pub fn r0(&self, lambda: f32) -> f32 {
        let n = self.eta.eval(lambda);
        let k = self.k.eval(lambda);
        ((n - 1.0).powi(2) + k * k) / ((n + 1.0).powi(2) + k * k)
    }

    /// Normal-incidence reflectance colour: linear sRGB of `r0(λ)` under D65
    /// (CIE 1931 2°, 1 nm). Gold's red channel exceeds 1: its reflectance
    /// colour lies outside the sRGB gamut, which a spectral renderer keeps.
    pub fn f0_srgb(&self) -> [f32; 3] {
        crate::cie::reflectance_srgb(|l| self.r0(l))
    }
}

/// The table and canonical symbol behind a name.
fn lookup(name: &str) -> Option<(&'static str, &'static [(f32, f32); GRID_LEN])> {
    let n = name.trim().to_ascii_lowercase();
    Some(match n.as_str() {
        // Johnson & Christy, "Optical constants of the noble metals",
        // Phys. Rev. B 6, 4370 (1972): evaporated films, 0.64–6.6 eV.
        "ag" | "silver" => ("Ag", &tab::AG),
        "au" | "gold" => ("Au", &tab::AU),
        "cu" | "copper" => ("Cu", &tab::CU),
        // Rakić, Djurišić, Elazar & Majewski, "Optical properties of
        // metallic films for vertical-cavity optoelectronic devices",
        // Appl. Opt. 37, 5271 (1998): Brendel–Bormann model fitted to the
        // measured data compiled in Palik's handbook.
        "pt" | "platinum" => ("Pt", &tab::PT),
        // Johnson & Christy, "Optical constants of transition metals: Ti, V,
        // Cr, Mn, Fe, Co, Ni, and Pd", Phys. Rev. B 9, 5056 (1974).
        "pd" | "palladium" => ("Pd", &tab::PD),
        // Weaver, Olson & Lynch, "Optical investigation of the electronic
        // structure of bulk Rh and Ir", Phys. Rev. B 15, 4115 (1977);
        // numerical values from Palik, Handbook of Optical Constants of
        // Solids (1985). Rhodium plating's white.
        "rh" | "rhodium" => ("Rh", &tab::RH),
        "ni" | "nickel" => ("Ni", &tab::NI),
        "ti" | "titanium" => ("Ti", &tab::TI),
        "al" | "aluminium" | "aluminum" => ("Al", &tab::AL),
        "cr" | "chromium" | "chrome" => ("Cr", &tab::CR),
        // Querry, "Optical constants", US Army CRDC report CRDC-CR-85034
        // (1985): reflectance of polished bulk samples, Kramers–Kronig.
        "fe" | "iron" => ("Fe", &tab::FE),
        // Querry, "Optical constants of minerals and other materials from the
        // millimeter to the ultraviolet", CRDEC-CR-88009 (1987). Querry
        // measured near-normal reflectance of bulk zinc and obtained n, k by
        // Kramers–Kronig; the reflectance is the measurement, but the
        // published visible n (9–14) exceeds k, a dielectric-like phase no
        // metal has (an artefact of the extrapolations). The table keeps
        // Querry's normal-incidence reflectance exactly and re-derives n, k
        // with `fresnel::gulbrandsen` at edge tint g = 0.90, between the
        // values of the d-metals of similar reflectance (Pd 0.87, Rh 0.88)
        // and of free-electron tin (0.93), so the angular falloff is metallic.
        // (Werner 2009's REELS zinc reads ~0.87, likely too bright; see above.)
        "zn" | "zinc" => ("Zn", &tab::ZN),
        // No visible-range measurement of tin is in the open databases; the
        // only one, Golovashkin & Motulevich, "Optical and electrical
        // properties of tin", Sov. Phys. JETP 19, 310 (1964), starts at
        // 730 nm. The table is a Drude fit to their 730–1000 nm points with
        // ε∞ = 1 (ħωp = 12.66 eV, ħγ = 1.345 eV; the large damping absorbs the
        // ~1 eV parallel-band transitions), extrapolated: R ≈ 0.81, flat and
        // neutral, consistent with tin's plain silver-white. Approximate.
        "sn" | "tin" => ("Sn", &tab::SN),
        _ => return None,
    })
}

/// Pure metals by chemical symbol or English name, case-insensitive
/// (`"Ag"`, `"silver"`; `"aluminium"`/`"aluminum"`).
pub fn get(name: &str) -> Option<ComplexIor> {
    lookup(name).map(|(_, t)| ComplexIor::from_grid(t))
}

/// Every name [`get`] accepts (canonical symbols).
pub fn names() -> &'static [&'static str] {
    &["Ag", "Au", "Cu", "Pt", "Pd", "Rh", "Ni", "Ti", "Al", "Cr", "Fe", "Zn", "Sn"]
}

/// The raw `(n, k)` grid rows of a metal, for alloy mixing.
pub(crate) fn grid_rows(symbol: &str) -> &'static [(f32, f32); GRID_LEN] {
    lookup(symbol).expect("known metal").1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_resolves_by_symbol_and_english_name() {
        let english = [
            "silver", "gold", "copper", "platinum", "palladium", "rhodium", "nickel", "titanium", "aluminium", "chromium",
            "iron", "zinc", "tin",
        ];
        for (sym, en) in names().iter().zip(english) {
            let a = get(sym).unwrap_or_else(|| panic!("{sym}"));
            let b = get(&en.to_ascii_uppercase()).unwrap_or_else(|| panic!("{en}"));
            let c = get(&sym.to_ascii_lowercase()).unwrap();
            for l in [400.0f32, 550.0, 700.0] {
                assert_eq!(a.r0(l), b.r0(l));
                assert_eq!(a.r0(l), c.r0(l));
            }
            assert_eq!(lookup(en).unwrap().0, *sym);
        }
        assert!(get("aluminum").is_some());
        assert!(get("unobtainium").is_none());
    }

    #[test]
    fn tables_are_physical_on_an_ascending_grid() {
        for sym in names() {
            let m = get(sym).unwrap();
            let lam: Vec<f32> = m.eta.samples().map(|s| s.0).collect();
            assert_eq!(lam.len(), GRID_LEN);
            assert!(lam.windows(2).all(|w| w[1] - w[0] == GRID_STEP), "{sym}");
            assert_eq!(lam[0], 360.0);
            assert_eq!(*lam.last().unwrap(), 830.0);
            for ((_, n), (_, k)) in m.eta.samples().zip(m.k.samples()) {
                assert!(n > 0.0 && k > 0.0 && n.is_finite() && k.is_finite(), "{sym}: {n} {k}");
                // a metal: ε1 = n² − k² < 0 everywhere in the visible
                assert!(n * n - k * k < 0.0, "{sym}: n={n} k={k}");
            }
            for l in (360..=830).step_by(5) {
                let r = m.r0(l as f32);
                assert!((0.3..1.0).contains(&r), "{sym} {l}: {r}");
            }
        }
    }

    #[test]
    fn reference_colours() {
        // Hoffman 2015 / RTR4 linear F0 where the same data underlie them.
        let close = |sym: &str, want: [f32; 3], tol: f32| {
            let got = get(sym).unwrap().f0_srgb();
            for k in 0..3 {
                assert!((got[k] - want[k]).abs() < tol, "{sym}: {got:?} vs {want:?}");
            }
        };
        close("Pd", [0.733, 0.697, 0.652], 0.005);
        close("Pt", [0.673, 0.637, 0.585], 0.01);
        close("Ni", [0.660, 0.609, 0.526], 0.015);
        close("Cu", [0.955, 0.638, 0.538], 0.025);
        close("Al", [0.913, 0.922, 0.924], 0.01);
        // silver near-white, gold yellow with red above 1, copper salmon
        let ag = get("Ag").unwrap().f0_srgb();
        assert!(ag.iter().all(|&c| c > 0.97), "{ag:?}");
        let au = get("Au").unwrap().f0_srgb();
        assert!(au[0] > 1.0 && au[1] > 0.7 && au[2] < 0.4, "{au:?}");
        let cu = get("Cu").unwrap().f0_srgb();
        assert!(cu[0] > cu[1] && cu[1] > cu[2], "{cu:?}");
        // rhodium is the brightest of the platinum-group whites
        let rh = get("Rh").unwrap().f0_srgb()[1];
        assert!(rh > get("Pd").unwrap().f0_srgb()[1] && rh > get("Pt").unwrap().f0_srgb()[1]);
    }

    #[test]
    fn johnson_christy_points_survive_resampling() {
        // JC 1972 gold at 1.88 eV (659.5 nm): n = 0.14, k = 3.697;
        // copper at 2.26 eV (548.6 nm): n = 1.02, k = 2.577.
        let au = get("Au").unwrap();
        assert!((au.eta.eval(659.5) - 0.14).abs() < 0.01 && (au.k.eval(659.5) - 3.697).abs() < 0.01);
        let cu = get("Cu").unwrap();
        assert!((cu.eta.eval(548.6) - 1.02).abs() < 0.02 && (cu.k.eval(548.6) - 2.577).abs() < 0.01);
    }

    #[test]
    fn zinc_keeps_querrys_reflectance() {
        // Querry 1987 at 560 nm: n = 13.908, k = 3.973 → R = 0.7661.
        let r = get("Zn").unwrap().r0(560.0);
        assert!((r - 0.7661).abs() < 2e-3, "{r}");
    }
}
