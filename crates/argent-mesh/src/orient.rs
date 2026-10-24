//! Automatic pose. TODO(agent): see crate docs for the canonical frame.

/// What the piece is, as far as geometry can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieceKind {
    Ring,
    Pendant,
    Other,
}

/// Canonical frame (millimetres, right-handed, +Y up):
/// - Ring: finger axis along ±Z, the head (signet table, skull, setting —
///   the side of the band carrying the most mass outside the bore) toward +Y,
///   bore centre at the origin. If the head has a "face" that looks along the
///   finger axis (a skull's face, an animal head), it looks toward +Z.
/// - Pendant: thinnest extent along Z with the decorated front toward +Z,
///   bail/loop (a small hole near one end of the longest axis) toward +Y,
///   bounding-box centre at the origin.
/// - Other: principal axes, longest along Y, thinnest along Z.
#[derive(Clone, Debug)]
pub struct Orientation {
    pub kind: PieceKind,
    pub pose: crate::Pose,
    /// Ring inner radius in mm, when a bore was found.
    pub bore_radius: Option<f32>,
    /// 0..1, how sure the classification and facing are.
    pub confidence: f32,
    /// Human-readable reasoning, for `argent info`.
    pub notes: Vec<String>,
}

/// Classify and pose a mesh. Never fails: falls back to principal axes.
pub fn detect(mesh: &crate::Mesh) -> Orientation {
    let (lo, hi) = mesh.bounds();
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
    Orientation {
        kind: PieceKind::Other,
        pose: crate::Pose { center: c, ..crate::Pose::IDENTITY },
        bore_radius: None,
        confidence: 0.0,
        notes: vec!["orientation detection not implemented yet: centred only".into()],
    }
}
