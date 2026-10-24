//! A renderable scene: one piece (any number of parts and materials) in a
//! studio of panels, a backdrop and a floor.

use crate::bvh::Bvh;
use crate::geom::{Ray, V3};
use crate::light::{Env, Floor, Panel};
use crate::material::Material;

/// The piece, posed in the canonical frame (mm, +Y up, front +Z).
pub struct Piece {
    pub positions: Vec<V3>,
    pub normals: Vec<V3>,
    pub triangles: Vec<[u32; 3]>,
    /// Material index per triangle.
    pub tri_material: Vec<u16>,
    /// Cavity mask per vertex (0 = exposed, 1 = deep recess), for patina.
    pub cavity: Option<Vec<f32>>,
    pub bvh: Bvh,
}

impl Piece {
    pub fn new(mesh: &argent_mesh::Mesh, tri_material: Vec<u16>) -> Piece {
        let bvh = Bvh::build(&mesh.positions, &mesh.triangles);
        Piece {
            positions: mesh.positions.iter().map(|&p| V3::from(p)).collect(),
            normals: mesh.normals.iter().map(|&n| V3::from(n)).collect(),
            triangles: mesh.triangles.clone(),
            tri_material,
            cavity: None,
            bvh,
        }
    }

    pub fn bounds(&self) -> (V3, V3) {
        let b = self.bvh.bounds();
        (b.min, b.max)
    }
}

impl argent_mesh::cavity::Occluder for Piece {
    fn occluded(&self, origin: [f32; 3], dir: [f32; 3], max_dist: f32) -> bool {
        self.bvh.occluded(&Ray::new(V3::from(origin), V3::from(dir)), max_dist)
    }
}

#[derive(Clone)]
pub struct Scene {
    pub piece: std::sync::Arc<Piece>,
    pub materials: Vec<Material>,
    pub panels: Vec<Panel>,
    pub env: Env,
    pub floor: Option<Floor>,
    /// Turntable angle: the piece is rotated this many degrees about +Y
    /// (through the origin); the studio stays put.
    pub turntable_deg: f32,
    /// Emitting panels and their selection CDF (by power).
    pub(crate) lights: Vec<usize>,
    pub(crate) light_cdf: Vec<f32>,
}

/// What a ray hit.
pub enum Hit {
    Piece { t: f32, p: V3, p_obj: V3, ng: V3, ns: V3, material: u16, cavity: f32 },
    Panel { t: f32, index: usize },
    Floor { t: f32, p: V3 },
}

impl Hit {
    pub fn t(&self) -> f32 {
        match *self {
            Hit::Piece { t, .. } | Hit::Panel { t, .. } | Hit::Floor { t, .. } => t,
        }
    }
}

impl Scene {
    pub fn new(piece: std::sync::Arc<Piece>, materials: Vec<Material>, panels: Vec<Panel>, env: Env, floor: Option<Floor>) -> Scene {
        let mut s = Scene { piece, materials, panels, env, floor, turntable_deg: 0.0, lights: vec![], light_cdf: vec![] };
        s.rebuild_lights();
        s
    }

    pub fn rebuild_lights(&mut self) {
        self.lights.clear();
        self.light_cdf.clear();
        let mut acc = 0.0;
        for (i, p) in self.panels.iter().enumerate() {
            if let Some(e) = &p.emission {
                let power = p.area * argent_spectra::spectrum::luminance_y(e.as_ref()).max(1e-6);
                acc += power;
                self.lights.push(i);
                self.light_cdf.push(acc);
            }
        }
        if acc > 0.0 {
            for c in &mut self.light_cdf {
                *c /= acc;
            }
        }
    }

    /// Probability of choosing light `panel` in light sampling.
    pub(crate) fn light_pick_pdf(&self, panel: usize) -> f32 {
        let Some(k) = self.lights.iter().position(|&i| i == panel) else { return 0.0 };
        let lo = if k == 0 { 0.0 } else { self.light_cdf[k - 1] };
        self.light_cdf[k] - lo
    }

    pub(crate) fn pick_light(&self, u: f32) -> Option<(usize, f32)> {
        if self.lights.is_empty() {
            return None;
        }
        let k = self.light_cdf.partition_point(|&c| c < u).min(self.lights.len() - 1);
        let lo = if k == 0 { 0.0 } else { self.light_cdf[k - 1] };
        Some((self.lights[k], self.light_cdf[k] - lo))
    }

    #[inline]
    fn to_object(&self, v: V3) -> V3 {
        if self.turntable_deg == 0.0 { v } else { crate::geom::rot_y(v, -self.turntable_deg) }
    }

    #[inline]
    fn to_world(&self, v: V3) -> V3 {
        if self.turntable_deg == 0.0 { v } else { crate::geom::rot_y(v, self.turntable_deg) }
    }

    /// Closest hit. `camera_ray` lets hidden softboxes stay out of frame.
    pub fn intersect(&self, ray: &Ray, camera_ray: bool) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        let mut t_max = f32::INFINITY;
        let oray = Ray::new(self.to_object(ray.o), self.to_object(ray.d));
        if let Some(h) = self.piece.bvh.intersect(&oray, t_max) {
            t_max = h.t;
            let tri = self.piece.triangles[h.prim as usize];
            let [a, b, c] = tri.map(|i| self.piece.positions[i as usize]);
            let ng = (b - a).cross(c - a).normalize();
            let w = 1.0 - h.u - h.v;
            let mat = self.piece.tri_material[h.prim as usize];
            let flat = self.materials[mat as usize].wants_flat_normals();
            let ns = if flat || self.piece.normals.is_empty() {
                ng
            } else {
                let [na, nb, nc] = tri.map(|i| self.piece.normals[i as usize]);
                let n = na * w + nb * h.u + nc * h.v;
                if n.length_squared() > 1e-12 { n.normalize() } else { ng }
            };
            let cavity = match &self.piece.cavity {
                Some(m) => m[tri[0] as usize] * w + m[tri[1] as usize] * h.u + m[tri[2] as usize] * h.v,
                None => 0.0,
            };
            let p_obj = a * w + b * h.u + c * h.v;
            best = Some(Hit::Piece {
                t: h.t,
                p: self.to_world(p_obj),
                p_obj,
                ng: self.to_world(ng),
                ns: self.to_world(ns),
                material: mat,
                cavity,
            });
        }
        for (i, panel) in self.panels.iter().enumerate() {
            if camera_ray && !panel.camera_visible {
                continue;
            }
            if let Some(t) = panel.intersect(ray, t_max) {
                t_max = t;
                best = Some(Hit::Panel { t, index: i });
            }
        }
        if let Some(f) = &self.floor {
            if let Some(t) = f.intersect(ray, t_max) {
                best = Some(Hit::Floor { t, p: ray.at(t) });
            }
        }
        best
    }

    /// Shadow test between a point and a light point; panels without
    /// emission (flags) and the piece block, the floor blocks.
    pub fn visible(&self, from: V3, to: V3, ignore_panel: usize) -> bool {
        let d = to - from;
        let dist = d.length();
        let dir = d / dist;
        let t_max = dist * (1.0 - 1e-4);
        let oray = Ray::new(self.to_object(from), self.to_object(dir));
        if self.piece.bvh.occluded(&oray, t_max) {
            return false;
        }
        let ray = Ray::new(from, dir);
        for (i, p) in self.panels.iter().enumerate() {
            if i != ignore_panel && p.intersect(&ray, t_max).is_some() {
                return false;
            }
        }
        if let Some(f) = &self.floor {
            if f.intersect(&ray, t_max).is_some() {
                return false;
            }
        }
        true
    }
}
