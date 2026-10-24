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
    /// Light with a photographed environment (equirectangular .hdr) instead
    /// of the studio's softboxes; the studio's floor stays.
    pub hdri: Option<String>,
    pub hdri_rotate: f32,
    pub hdri_exposure: f32,
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
            hdri: None,
            hdri_rotate: 0.0,
            hdri_exposure: 0.0,
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
        let hdri = look.hdri.as_deref().and_then(|p| match cached_envmap(p, look.hdri_exposure, look.hdri_rotate) {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!("argent: couldn't load HDRI {p}: {e:#}; using the studio lights");
                None
            }
        });
        let (panels, env, white) = match hdri {
            Some(m) => (vec![], crate::light::Env::Map(m), None),
            None => (rig.panels, rig.env, rig.white_kelvin.map(crate::color::blackbody_white)),
        };
        let mut scene = Scene::new(piece, materials, panels, env, floor);
        scene.white = white;
        scene
    }

    /// Camera for a still, focused on the nearest surface near the centre.
    pub fn camera(&self, view: &View, lens: &Lens, aspect: f32) -> Camera {
        let mut cam = frame(&self.hull, view, lens, aspect);
        if cam.lens_radius > 0.0 {
            self.autofocus(&mut cam, 0.0);
        }
        cam
    }

    /// Autofocus: probe the piece with a few rays around the frame centre
    /// and focus on the nearest surface (the face, not the bounding box
    /// centre). Leaves the focus alone when every probe misses.
    pub fn autofocus(&self, cam: &mut Camera, turn_deg: f32) {
        // Probes lean upward: a ring's head (the subject) sits in the upper
        // half of a hero frame. The median keeps one near shoulder or a far
        // bore wall from stealing focus.
        let mut hits = vec![];
        for (sx, sy) in [(0.0, 0.0), (0.0, 0.15), (0.0, 0.3), (0.12, 0.22), (-0.12, 0.22), (0.0, 0.42), (0.2, 0.1), (-0.2, 0.1)] {
            let r = cam.ray(sx, sy, 0.5, 0.5);
            let o = crate::geom::rot_y(r.o, -turn_deg);
            let d = crate::geom::rot_y(r.d, -turn_deg);
            if let Some(h) = self.piece.bvh.intersect(&crate::geom::Ray::new(o, d), f32::INFINITY) {
                hits.push(h.t * d.dot(crate::geom::rot_y(cam.forward, -turn_deg)));
            }
        }
        if !hits.is_empty() {
            hits.sort_by(|a, b| a.total_cmp(b));
            cam.focus_dist = hits[hits.len() / 2];
        }
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

/// Environment maps are slow to load and build (≈0.2 s for 2K); keep the
/// last few by (path, exposure). Rotation is applied at lookup, so turning
/// the environment costs nothing.
fn cached_envmap(path: &str, exposure: f32, rotate: f32) -> anyhow::Result<Arc<crate::envmap::EnvMap>> {
    use parking_lot::Mutex;
    use std::sync::OnceLock;
    type Entry = (String, u32, Arc<crate::envmap::EnvData>);
    static CACHE: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(vec![]));
    let found = cache.lock().iter().find(|e| e.0 == path && e.1 == exposure.to_bits()).map(|e| e.2.clone());
    let data = match found {
        Some(d) => d,
        None => {
            let d = crate::envmap::EnvData::load(std::path::Path::new(path), exposure)?;
            let mut c = cache.lock();
            if c.len() >= 3 {
                c.remove(0);
            }
            c.push((path.to_string(), exposure.to_bits(), d.clone()));
            d
        }
    };
    Ok(Arc::new(crate::envmap::EnvMap { data, rotation_deg: rotate }))
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
    /// Faster denoising for animation frames (OIDN balanced quality).
    pub fast_denoise: bool,
}

impl Default for Quality {
    fn default() -> Self {
        Quality { width: 1080, height: 1920, spp: 128, path: PathSettings::default(), denoise: true, fast_denoise: false }
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
    // The guides are deterministic first-hit buffers (only AA/DOF jitter),
    // so OIDN's aux prefilter passes buy nothing: 1.37 s → 0.48 s at 2 MP.
    let opts = argent_post::denoise::DenoiseOptions {
        prefilter_aux: false,
        quality: if q.fast_denoise { argent_post::denoise::DenoiseQuality::Balanced } else { argent_post::denoise::DenoiseQuality::High },
    };
    argent_post::denoise::denoise_with(
        argent_post::DenoiseInput { color: &color, albedo: Some(&albedo), normal: Some(&normal) },
        argent_post::DenoiseBackend::Auto,
        &opts,
    )
}

/// The grade for a look. Marketplace listings want a pure white backdrop
/// (sRGB ≈ 250+), which AgX deliberately never reaches, so the white studio
/// uses Khronos PBR Neutral unless a look is asked for explicitly.
pub fn grade(look: &Look) -> argent_post::Grade {
    let tonemap = match look.grade.as_str() {
        "base" => argent_post::ToneMap::AgX(argent_post::AgxLook::Base),
        "golden" => argent_post::ToneMap::AgX(argent_post::AgxLook::Golden),
        "neutral" => argent_post::ToneMap::KhronosNeutral,
        "aces" => argent_post::ToneMap::AcesFitted,
        "punchy" if look.studio == Studio::White && look.hdri.is_none() => argent_post::ToneMap::KhronosNeutral,
        _ => argent_post::ToneMap::AgX(argent_post::AgxLook::Punchy),
    };
    argent_post::Grade { exposure_ev: look.exposure_ev, tonemap, saturation: 1.0 }
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
