//! Spectral foundations for the argent renderer.
//!
//! Every path carries [`N`] wavelengths at once (hero-wavelength sampling,
//! Wilkie et al. 2014); a [`Spec`] holds one value per carried wavelength.
//! Dispersive events terminate the secondary wavelengths
//! ([`Wavelengths::terminate_secondary`]) so the estimator stays unbiased.
//!
//! Units: wavelengths in nanometres, absorption coefficients in 1/mm (the
//! renderer works in millimetres because jewelry is measured in them).

pub mod alloys;
pub mod cie;
mod data;
pub mod fresnel;
pub mod gems;
pub mod metals;
pub mod spec;
pub mod spectrum;

pub use spec::{Spec, Wavelengths, N, LAMBDA_MAX, LAMBDA_MIN};
pub use spectrum::{blackbody_normalized, Blackbody, Constant, Piecewise, RgbAlbedo, RgbIlluminant, Spectrum};
