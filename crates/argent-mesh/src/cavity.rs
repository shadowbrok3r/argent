//! Per-vertex cavity masks for antiquing (oxidised recesses, polished highs).

/// Occlusion query: `(origin, direction, max_distance) -> hit?`.
pub trait Occluder: Sync {
    fn occluded(&self, origin: [f32; 3], dir: [f32; 3], max_dist: f32) -> bool;
}

/// Settings for [`patina_mask`]; distances in mm.
#[derive(Clone, Debug)]
pub struct PatinaSettings {
    /// AO rays per vertex.
    pub ao_samples: u32,
    /// How far a recess "sees" (≈ the polishing cloth's reach), mm.
    pub ao_distance: f32,
    /// Curvature analysis radius, mm.
    pub curvature_radius: f32,
    /// Blend between AO (0) and concavity (1).
    pub concavity_weight: f32,
    /// Laplacian smoothing passes over the final mask.
    pub smooth_iterations: u32,
}

impl Default for PatinaSettings {
    fn default() -> Self {
        PatinaSettings { ao_samples: 48, ao_distance: 1.2, curvature_radius: 0.4, concavity_weight: 0.35, smooth_iterations: 3 }
    }
}

/// One value per vertex in `0..=1`: 1 = deep recess (oxidised black),
/// 0 = exposed high point (polished bright). TODO(agent): AO via `occ`,
/// concavity from mean curvature, contrast-shaped, smoothed.
pub fn patina_mask(mesh: &crate::Mesh, _occ: &dyn Occluder, _s: &PatinaSettings) -> Vec<f32> {
    vec![0.0; mesh.positions.len()]
}
