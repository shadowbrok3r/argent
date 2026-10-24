//! Embedded measured data. Each table names its source next to it; the
//! modules that use them (`cie`, `metals`, `alloys`) explain the choices.

// Measured values are what they are: some sit near π-like constants, and the
// CIE strings carry more digits than an f32 holds.
#![allow(clippy::approx_constant, clippy::excessive_precision)]

pub(crate) mod cie;
pub(crate) mod metals;
pub(crate) mod alloys;
