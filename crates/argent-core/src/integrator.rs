//! Spectral unidirectional path tracer with next-event estimation and
//! multiple importance sampling on the softboxes.

use crate::bsdf::Bsdf;
use crate::geom::{offset_origin, Frame, Ray, V3};
use crate::sampler::Sampler;
use crate::scene::{Hit, Scene};
use argent_spectra::{Spec, Spectrum, Wavelengths};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct PathSettings {
    pub max_depth: u32,
    /// Max RGB component of any contribution arriving after a rough bounce
    /// (firefly clamp). Direct views and mirror chains are never clamped.
    pub clamp_indirect: f32,
    /// Bounces before Russian roulette starts.
    pub rr_depth: u32,
}

impl Default for PathSettings {
    fn default() -> Self {
        PathSettings { max_depth: 32, clamp_indirect: 24.0, rr_depth: 6 }
    }
}

/// Denoiser guides from the first visible surface.
#[derive(Clone, Copy, Debug, Default)]
pub struct Guides {
    pub albedo: [f32; 3],
    pub normal: [f32; 3],
}

#[inline]
fn add_clamped(l: &mut Spec, c: Spec, clamp: Option<f32>) {
    match clamp {
        Some(m) => {
            let mx = c.max_value();
            if mx > m {
                *l += c * (m / mx);
            } else {
                *l += c;
            }
        }
        None => *l += c,
    }
}

/// Trace one camera path. Returns spectral radiance for `wl` (which may have
/// its secondary wavelengths terminated on return).
pub fn trace(scene: &Scene, mut ray: Ray, wl: &mut Wavelengths, s: &mut Sampler, ps: &PathSettings, guides: &mut Guides) -> Spec {
    let mut l = Spec::ZERO;
    let mut beta = Spec::ONE;
    let mut specular = true;
    let mut rough_seen = false;
    let mut prev_pdf = 1.0f32;
    let mut medium: Option<Arc<dyn Spectrum>> = None;
    let floor_n = V3::Y;

    for depth in 0..ps.max_depth {
        let clamp = if rough_seen { Some(ps.clamp_indirect) } else { None };
        // Dimensions per bounce: fixed blocks keep Sobol strata aligned.
        s.set_dim(8 + depth * 8);
        let hit = scene.intersect(&ray, depth == 0);
        if let (Some(m), Some(h)) = (&medium, &hit) {
            let sig = m.sample(wl);
            beta *= (-sig * h.t()).exp();
        }
        let (p, ng, ns, mat_id, cavity) = match hit {
            None => {
                add_clamped(&mut l, beta * scene.env.eval(ray.d, wl), clamp);
                break;
            }
            Some(Hit::Panel { t, index }) => {
                let panel = &scene.panels[index];
                let le = panel.le(ray.d, wl);
                if !le.is_black() {
                    if specular {
                        add_clamped(&mut l, beta * le, clamp);
                    } else {
                        let cos_l = (-ray.d).dot(panel.n).abs().max(1e-8);
                        let p_light = scene.light_pick_pdf(index) * t * t / (cos_l * panel.area);
                        let w = prev_pdf * prev_pdf / (prev_pdf * prev_pdf + p_light * p_light);
                        add_clamped(&mut l, beta * le * w, clamp);
                    }
                }
                break;
            }
            Some(Hit::Floor { p, .. }) => {
                let floor = scene.floor.as_ref().unwrap();
                if s.next_random() >= floor.presence(p) {
                    add_clamped(&mut l, beta * scene.env.eval(ray.d, wl), clamp);
                    break;
                }
                let n = if ray.d.y < 0.0 { floor_n } else { -floor_n };
                (p, n, n, floor.material as u16, 0.0)
            }
            Some(Hit::Piece { p, ng, ns, material, cavity, .. }) => (p, ng, ns, material, cavity),
        };

        let mat = &scene.materials[mat_id as usize];
        let bsdf = mat.bsdf(wl, cavity, s.next());
        let wo_w = -ray.d;
        let transmissive = matches!(bsdf, Bsdf::Dielectric { .. });
        // Two-sided shading for opaque surfaces; dielectrics keep the outward
        // normal so they know whether the ray is entering or leaving.
        let (mut nsu, ngu) = if !transmissive && ng.dot(wo_w) < 0.0 { (-ns, -ng) } else { (ns, ng) };
        if !transmissive && nsu.dot(wo_w) <= 0.0 {
            nsu = ngu;
        }
        let frame = Frame::new(nsu);
        let wo = frame.to_local(wo_w);
        if depth == 0 {
            guides.albedo = crate::spectral_rgb_guide(mat, cavity);
            guides.normal = nsu.into();
        }

        // Next-event estimation.
        if !bsdf.is_delta() {
            let ul = s.next();
            let (u1, u2) = s.next2();
            if let Some((li, pick)) = scene.pick_light(ul) {
                let panel = &scene.panels[li];
                let lp = panel.sample_point(u1, u2);
                let to = lp - p;
                let d2 = to.length_squared();
                let dir = to / d2.sqrt();
                let cos_l = (-dir).dot(panel.n);
                if cos_l > 0.0 && dir.dot(ngu) > 0.0 {
                    let wi = frame.to_local(dir);
                    let f = bsdf.eval(wo, wi);
                    if !f.is_black() {
                        let p_light = pick * d2 / (cos_l * panel.area);
                        let p_bsdf = bsdf.pdf(wo, wi);
                        let w = p_light * p_light / (p_light * p_light + p_bsdf * p_bsdf);
                        let origin = offset_origin(p, ngu, dir);
                        if scene.visible(origin, lp, li) {
                            let le = panel.le(dir, wl);
                            let c = beta * f * le * (wi.z.abs() * w / p_light);
                            add_clamped(&mut l, c, if rough_seen { Some(ps.clamp_indirect) } else { None });
                        }
                    }
                }
            }
        } else {
            s.next();
            s.next2();
        }

        // Continue the path.
        let u0 = s.next();
        let (u1, u2) = s.next2();
        let Some(smp) = bsdf.sample(wo, u0, u1, u2) else { break };
        if smp.pdf <= 0.0 || smp.f.is_black() {
            break;
        }
        let wi_w = frame.to_world(smp.wi);
        let side = wi_w.dot(ngu);
        if (!smp.transmit && side <= 0.0) || (smp.transmit && side >= 0.0) {
            break;
        }
        if smp.terminate_secondary {
            wl.terminate_secondary();
        }
        beta *= smp.f * (smp.wi.z.abs() / smp.pdf);
        if !beta.is_finite() {
            break;
        }
        specular = smp.delta;
        if !smp.delta {
            rough_seen = true;
        }
        prev_pdf = smp.pdf;
        if smp.transmit {
            medium = if wo.z > 0.0 { mat.medium().cloned() } else { None };
        }
        ray = Ray::new(offset_origin(p, ngu, wi_w), wi_w);

        if depth >= ps.rr_depth {
            let survive = beta.max_value().min(0.95);
            if survive <= 0.0 || s.next_random() >= survive {
                break;
            }
            beta /= survive;
        }
    }
    l
}
