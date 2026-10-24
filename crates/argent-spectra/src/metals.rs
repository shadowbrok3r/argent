//! Measured complex refractive indices of pure metals.
//!
//! PLACEHOLDER DATA: coarse values for Ag, Au and Cu only. To be replaced by
//! tabulated measurements (Johnson & Christy 1972 for Ag/Au/Cu, Rakić 1998 or
//! Palik for the rest) at ≤10 nm spacing over 360..830 nm, with the source
//! cited per metal. Keep the API.

use crate::spec::{Spec, Wavelengths, N};
use crate::spectrum::{Piecewise, Spectrum};

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
}

/// Pure metals by chemical symbol or English name, case-insensitive
/// (`"Ag"`, `"silver"`).
pub fn get(name: &str) -> Option<ComplexIor> {
    let n = name.to_ascii_lowercase();
    let rows: &[(f32, f32, f32)] = match n.as_str() {
        "ag" | "silver" => &[
            (360.0, 0.16, 1.40),
            (400.0, 0.05, 2.07),
            (450.0, 0.04, 2.66),
            (500.0, 0.05, 3.09),
            (550.0, 0.055, 3.59),
            (600.0, 0.056, 4.0),
            (650.0, 0.052, 4.4),
            (700.0, 0.04, 4.8),
            (830.0, 0.04, 5.8),
        ],
        "au" | "gold" => &[
            (360.0, 1.70, 1.90),
            (400.0, 1.66, 1.96),
            (450.0, 1.38, 1.92),
            (500.0, 0.97, 1.87),
            (550.0, 0.43, 2.46),
            (600.0, 0.25, 2.98),
            (650.0, 0.17, 3.5),
            (700.0, 0.16, 3.95),
            (830.0, 0.18, 5.0),
        ],
        "cu" | "copper" => &[
            (360.0, 1.25, 2.0),
            (400.0, 1.18, 2.21),
            (450.0, 1.15, 2.45),
            (500.0, 1.12, 2.6),
            (550.0, 0.95, 2.58),
            (600.0, 0.27, 3.4),
            (650.0, 0.21, 3.67),
            (700.0, 0.21, 4.2),
            (830.0, 0.25, 5.2),
        ],
        _ => return None,
    };
    Some(ComplexIor::from_table(rows))
}

/// Every name [`get`] accepts (canonical symbols).
pub fn names() -> &'static [&'static str] {
    &["Ag", "Au", "Cu"]
}
