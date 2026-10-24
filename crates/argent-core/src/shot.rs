//! High-level shots: prepare a piece once (pose, BVH, cavity mask), then
//! render any look — metal, finish, stones, studio, view — against it.

use crate::camera::{Camera, Lens};
use crate::geom::V3;
use crate::integrator::PathSettings;
use crate::material::{default_patina, Material};
use crate::render::{render_pass, Film};
use crate::scene::{Piece, Scene};
use crate::studio::{floor_at, Studio};
use crate::views::{frame, View};
use argent_mesh::orient::{Orientation, PieceKind};
use argent_mesh::{Mesh, Pose};
use argent_spectra::alloys::Alloy;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Which material slot a part goes to: slot 0 is the metal, stones follow.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoneRule {
    /// Case-insensitive glob over the part name (`*` wildcard), e.g. `stone*`.
    pub part: String,
    /// Gem name from `argent_spectra::gems::names()`.
    pub gem: String,
}

/// Everything about how a piece looks, independent of its geometry.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    pub metal: String,
    /// Perceptual roughness of the polish (0.03 mirror .. 0.35 satin).
    pub roughness: f32,
    /// 0 = bright polish everywhere, 1 = deep antique in the recesses.
    pub patina: f32,
    pub stones: Vec<StoneRule>,
    pub studio: Studio,
    pub exposure_ev: f32,
    /// AgX look: `base`, `punchy` or `golden`.
    pub grade: String,
}

impl Default for Look {
    fn default() -> Self {
        Look {
            metal: "sterling".into(),
            roughness: 0.07,
            patina: 0.8,
            stones: vec![],
            studio: Studio::Noir,
            exposure_ev: 0.0,
            grade: "punchy".into(),
        }
    }
}

impl Look {
    pub fn alloy(&self) -> Alloy {
        Alloy::from_name(&self.metal).unwrap_or(Alloy::Sterling)
    }
}

/// A posed, accelerated piece ready to render under any look.
pub struct Prepared {
    pub mesh: Mesh,
    pub orientation: Orientation,
    pub piece: Arc<Piece>,
    /// Part index → material slot (0 = metal, k = stone rule k-1).
    pub part_names: Vec<String>,
    pub radius: f32,
    pub floor_y: f32,
    /// Subsample of vertices for framing.
    pub hull: Vec<V3>,
    /// Stone assignments that travel with the piece (e.g. from a CAD
    /// export's sidecar); a look's own rules take precedence.
    pub default_stones: Vec<StoneRule>,
}

impl Prepared {
    /// Pose the mesh (auto unless `pose` is given), build the BVH, compute
    /// the cavity mask.
    pub fn new(mesh: Mesh, pose: Option<Pose>) -> Prepared {
        Prepared::with_chain(mesh, pose, None)
    }

    /// As [`Prepared::new`], hanging a chain from a pendant's bail when
    /// `chain` is given and the piece is a pendant (or `force` kind Other
    /// with a clear top). The chain is metal but never framed.
    pub fn with_chain(mesh: Mesh, pose: Option<Pose>, chain: Option<crate::chain::ChainSpec>) -> Prepared {
        Prepared::build(mesh, pose, chain, false)
    }

    /// `force_chain` hangs the chain even when the piece wasn't recognised
    /// as a pendant.
    pub fn build(mut mesh: Mesh, pose: Option<Pose>, chain: Option<crate::chain::ChainSpec>, force_chain: bool) -> Prepared {
        let orientation = match pose {
            Some(p) => Orientation { kind: PieceKind::Other, pose: p, bore_radius: None, confidence: 1.0, notes: vec!["pose given".into()] },
            None => argent_mesh::orient::detect(&mesh),
        };
        mesh.apply_pose(&orientation.pose);
        let (plo, phi) = mesh.bounds();
        let piece_hull: Vec<V3> = {
            let step = (mesh.positions.len() / 4000).max(1);
            mesh.positions.iter().step_by(step).map(|&p| V3::from(p)).collect()
        };
        let wants_chain = orientation.kind == PieceKind::Pendant || (force_chain && orientation.kind != PieceKind::Ring);
        if let (Some(spec), true) = (chain, wants_chain) {
            // Hang from the top of the piece, slightly below its highest
            // point so the first link passes through the bail.
            let size = (V3::from(phi) - V3::from(plo)).max_element();
            let top: Vec<V3> = mesh.positions.iter().map(|&p| V3::from(p)).filter(|p| p.y > phi[1] - 0.08 * size).collect();
            let c = top.iter().fold(V3::ZERO, |a, &b| a + b) / top.len().max(1) as f32;
            let anchor = V3::new(c.x, phi[1] - 0.04 * size, c.z);
            let scale = (size / 30.0).clamp(0.6, 1.6);
            let mut spec = spec;
            spec.wire *= scale;
            spec.link_len *= scale;
            spec.link_w *= scale;
            spec.strand = 3.0 * size;
            mesh.merge(crate::chain::hang(anchor, &spec));
        }
        let part_names = mesh.parts.iter().map(|p| p.name.clone()).collect();
        let tri_mat = vec![0u16; mesh.triangles.len()];
        let mut piece = Piece::new(&mesh, tri_mat);
        // Size the studio and the cavity analysis by the piece, not its chain.
        let lo = V3::from(plo);
        let radius = ((V3::from(phi) - lo) * 0.5).length().max(1e-3);
        let settings = argent_mesh::cavity::PatinaSettings {
            ao_distance: (radius * 0.09).clamp(0.4, 2.5),
            curvature_radius: (radius * 0.03).clamp(0.15, 1.0),
            ..Default::default()
        };
        let mask = argent_mesh::cavity::patina_mask(&mesh, &piece, &settings);
        piece.cavity = Some(mask);
        let hull = piece_hull;
        let _ = lo;
        Prepared { floor_y: plo[1], mesh, orientation, piece: Arc::new(piece), part_names, radius, hull, default_stones: vec![] }
    }

    /// Material slot per triangle for a set of stone rules: the rule index
    /// + 1 for the first matching part name, else 0 (metal). Parts whose
    /// own material name is a gem also become stones.
    fn slots(&self, rules: &[StoneRule]) -> (Vec<u16>, Vec<String>) {
        let mut gems: Vec<String> = vec![];
        let mut tri = vec![0u16; self.mesh.triangles.len()];
        for part in &self.mesh.parts {
            let mut gem = rules
                .iter()
                .chain(self.default_stones.iter())
                .find(|r| glob(&r.part, &part.name))
                .map(|r| r.gem.clone());
            if gem.is_none() {
                for cand in [part.material.as_deref(), Some(part.name.as_str())].into_iter().flatten() {
                    gem = gem_from_text(cand);
                    if gem.is_some() {
                        break;
                    }
                }
            }
            if gem.is_none() {
                // Generic stone parts ("gem", "stone_03") default to diamond.
                let generic = |t: &str| {
                    let t = t.to_ascii_lowercase();
                    t == "gem" || t == "gems" || t.starts_with("stone") || t.starts_with("gem_")
                };
                if part.material.as_deref().is_some_and(generic) || generic(&part.name) {
                    gem = Some("diamond".into());
                }
            }
            if let Some(g) = gem {
                let slot = match gems.iter().position(|x| *x == g) {
                    Some(i) => i + 1,
                    None => {
                        gems.push(g);
                        gems.len()
                    }
                } as u16;
                for t in part.triangles.clone() {
                    tri[t as usize] = slot;
                }
            }
        }
        (tri, gems)
    }

    /// Build the scene for `look`. Cheap: shares the BVH and mask.
    pub fn scene(&self, look: &Look) -> Scene {
        let alloy = look.alloy();
        let (tri, gems) = self.slots(&look.stones);
        let piece = if tri.iter().any(|&s| s != 0) {
            let p = &*self.piece;
            Arc::new(Piece {
                positions: p.positions.clone(),
                normals: p.normals.clone(),
                triangles: p.triangles.clone(),
                tri_material: tri,
                cavity: p.cavity.clone(),
                bvh: p.bvh.clone(),
            })
        } else {
            self.piece.clone()
        };
        let mut materials = vec![Material::metal(alloy, look.roughness).with_patina(default_patina(look.patina, alloy))];
        for g in &gems {
            materials.push(Material::gem(g).unwrap_or_else(|| Material::gem("diamond").unwrap()));
        }
        let rig = look.studio.build(self.radius);
        let floor = rig.floor.map(|(m, a, b)| {
            materials.push(m);
            floor_at(self.floor_y - 0.005, materials.len() - 1, (a, b), self.radius)
        });
        Scene::new(piece, materials, rig.panels, rig.env, floor)
    }

    /// Camera for a still.
    pub fn camera(&self, view: &View, lens: &Lens, aspect: f32) -> Camera {
        frame(&self.hull, view, lens, aspect)
    }

    /// Camera that keeps the piece in frame for a full turn.
    pub fn turntable_camera(&self, view: &View, lens: &Lens, aspect: f32) -> Camera {
        let mut pts = Vec::with_capacity(self.hull.len() * 24);
        for k in 0..24 {
            let a = k as f32 * 15.0;
            pts.extend(self.hull.iter().map(|&p| crate::geom::rot_y(p, a)));
        }
        frame(&pts, view, lens, aspect)
    }
}

/// Find a gem name in free text ("Shoulder emerald / 25 degrees" → emerald).
/// Longest match wins so "black_opal" beats "opal" and "pink_sapphire" beats
/// "sapphire".
pub fn gem_from_text(text: &str) -> Option<String> {
    let t = text.to_ascii_lowercase().replace([' ', '-'], "_");
    let mut best: Option<&str> = None;
    for g in argent_spectra::gems::names() {
        if t.contains(g) && best.is_none_or(|b| g.len() > b.len()) {
            best = Some(g);
        }
    }
    if best.is_none() {
        // Common words that aren't table keys.
        for (w, g) in [("opal", "opal"), ("cz", "cubic_zirconia"), ("zircon", "cubic_zirconia"), ("onyx", "onyx"), ("garnet", "garnet")] {
            if t.split(|c: char| !c.is_ascii_alphanumeric()).any(|x| x == w) {
                return Some(g.into());
            }
        }
    }
    best.map(str::to_string)
}

/// Case-insensitive glob with `*`.
pub fn glob(pattern: &str, name: &str) -> bool {
    let p = pattern.to_ascii_lowercase();
    let n = name.to_ascii_lowercase();
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == n;
    }
    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !n.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if let Some(k) = n[pos..].find(part) {
            pos += k + part.len();
        } else {
            return false;
        }
    }
    parts.last().map(|l| l.is_empty() || n.ends_with(l)).unwrap_or(true)
}

/// Render settings for one image.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Quality {
    pub width: u32,
    pub height: u32,
    pub spp: u32,
    pub path: PathSettings,
    pub denoise: bool,
}

impl Default for Quality {
    fn default() -> Self {
        Quality { width: 1080, height: 1920, spp: 128, path: PathSettings::default(), denoise: true }
    }
}

/// Render to a linear image (denoised when asked) plus timing.
pub fn render_image(scene: &Scene, cam: &Camera, q: &Quality, seed: u32) -> anyhow::Result<argent_post::Image> {
    let mut film = Film::new(q.width, q.height);
    render_pass(scene, cam, &mut film, &q.path, q.spp, seed);
    let color = film.color();
    if !q.denoise {
        return Ok(color);
    }
    let albedo = film.albedo();
    let normal = film.normal();
    argent_post::denoise(
        argent_post::DenoiseInput { color: &color, albedo: Some(&albedo), normal: Some(&normal) },
        argent_post::DenoiseBackend::Auto,
    )
}

/// The grade for a look.
pub fn grade(look: &Look) -> argent_post::Grade {
    let lk = match look.grade.as_str() {
        "base" => argent_post::AgxLook::Base,
        "golden" => argent_post::AgxLook::Golden,
        _ => argent_post::AgxLook::Punchy,
    };
    argent_post::Grade { exposure_ev: look.exposure_ev, tonemap: argent_post::ToneMap::AgX(lk), saturation: 1.0 }
}

#[cfg(test)]
mod tests {
    use super::glob;

    #[test]
    fn globs() {
        assert!(glob("stone*", "Stone_01"));
        assert!(glob("*eye*", "left-eye-gem"));
        assert!(!glob("stone*", "band"));
        assert!(glob("band", "BAND"));
    }
}
