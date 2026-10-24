//! argent core: a spectral path tracer for jewelry.
//!
//! The piece is one static triangle mesh (in the canonical pose from
//! `argent_mesh::orient`), shaded with measured metal and gem optics,
//! lit by studio panels; the turntable rotates the piece, not the studio.

pub mod bsdf;
pub mod bvh;
pub mod camera;
pub mod geom;
pub mod integrator;
pub mod light;
pub mod material;
pub mod microfacet;
pub mod render;
pub mod sampler;
pub mod scene;
pub mod shot;
pub mod studio;
pub mod views;

pub(crate) use material::guide_rgb as spectral_rgb_guide;
