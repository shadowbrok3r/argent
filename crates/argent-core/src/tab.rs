//! Spectra baked to a uniform 1 nm table at scene build: blackbody and
//! sigmoid spectra cost an `exp` (Planck in f64) per wavelength per lookup,
//! and a path looks lights and the backdrop up dozens of times.

use argent_spectra::{Spectrum, LAMBDA_MAX, LAMBDA_MIN};
use std::sync::Arc;

pub struct Tabulated {
    values: Vec<f32>,
}

impl Tabulated {
    pub fn new(s: &dyn Spectrum) -> Tabulated {
        let n = (LAMBDA_MAX - LAMBDA_MIN) as usize + 1;
        Tabulated { values: (0..n).map(|i| s.eval(LAMBDA_MIN + i as f32)).collect() }
    }
}

impl Spectrum for Tabulated {
    #[inline]
    fn eval(&self, l: f32) -> f32 {
        let x = (l - LAMBDA_MIN).clamp(0.0, (self.values.len() - 1) as f32);
        let i = (x as usize).min(self.values.len() - 2);
        let f = x - i as f32;
        self.values[i] * (1.0 - f) + self.values[i + 1] * f
    }
}

/// Bake `s` unless it is already cheap.
pub fn bake(s: Arc<dyn Spectrum>) -> Arc<dyn Spectrum> {
    Arc::new(Tabulated::new(s.as_ref()))
}
