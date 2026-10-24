//! Web and AR export for argent: casting meshes → GLB for Shopify 3D media.
//!
//! Casting files run 1–5 M triangles; a product page wants a GLB of a few
//! MB that `<model-viewer>` can spin and Shopify can convert to USDZ for
//! iOS AR. [`export_glb`]:
//!
//! 1. [`simplify`]: Garland–Heckbert quadric-error edge collapse to
//!    `target_triangles`. Parts never share a vertex, so stones stay
//!    separate meshes, and small stones are kept verbatim.
//! 2. Patina carried to the simplified vertices by averaging every vertex
//!    that collapsed into one ([`carry_scalars`]).
//! 3. Crease-split normals: 50° on metal, facet normals on cut stones
//!    (≤ 400 triangles), 50° on larger stones such as cabochons.
//! 4. A hand-written glTF 2.0 binary: one mesh per part, positions in
//!    metres, a PBR metallicRoughness metal (metallic 1), glass-like stones
//!    (`KHR_materials_transmission`, `KHR_materials_ior`,
//!    `KHR_materials_volume`), and the patina baked into `COLOR_0` as a
//!    darkening of `base_color` toward the tarnish colour.
//! 5. If the file is larger than `max_bytes`, simplification continues to
//!    a proportionally smaller target and the file is rebuilt.

mod bvh;
mod glb;
mod material;
mod math;
mod normals;
mod parts;
mod simplify;

pub use material::{Gem, PartKind, classify_part, gem_from_text};
pub use normals::crease_normals;
pub use simplify::{SimplifyStats, carry_scalars, simplify, simplify_with_stats};

use anyhow::Context;
use argent_mesh::Mesh;
use material::classify_parts;
use parts::PartTable;
use serde_json::{Value, json};
use std::path::Path;
use std::time::Instant;

/// Crease angle on metal parts and large stones.
const SMOOTH_CREASE_DEG: f64 = 50.0;
/// Crease angle on cut stones: coplanar triangles of one facet still share.
const FACET_CREASE_DEG: f64 = 1.0;
/// Stone parts up to this many triangles are cut stones (facet normals).
const FACETED_GEM_MAX_TRIS: usize = 400;
/// Stone parts up to this many triangles are not simplified.
const FROZEN_GEM_MAX_TRIS: usize = 5000;
/// Give up shrinking for `max_bytes` after this many extra passes.
const MAX_SIZE_PASSES: usize = 8;

/// Settings for [`export_glb`].
#[derive(Clone, Debug)]
pub struct GlbOptions {
    /// Triangle budget for the whole piece (all parts together).
    pub target_triangles: u32,
    /// File size cap; triangles are reduced further while the GLB exceeds it.
    pub max_bytes: Option<u64>,
    /// Linear RGB normal-incidence colour of the metal (glTF base colour of
    /// a metallic material).
    pub base_color: [f32; 3],
    /// glTF `roughnessFactor` of the metal.
    pub roughness: f32,
    /// Per-input-vertex mask in `0..=1` (1 = recess), tarnish colour
    /// (linear RGB) and strength: baked into `COLOR_0` on metal parts as
    /// `mix(base_color, tarnish, mask · strength)`.
    pub patina: Option<(Vec<f32>, [f32; 3], f32)>,
    /// Scene and root-node name.
    pub name: String,
}

impl Default for GlbOptions {
    fn default() -> Self {
        GlbOptions {
            target_triangles: 150_000,
            max_bytes: Some(15_000_000),
            // Polished sterling silver at normal incidence (linear).
            base_color: [0.97, 0.96, 0.91],
            roughness: 0.2,
            patina: None,
            name: "argent".into(),
        }
    }
}

/// What [`export_glb`] produced.
#[derive(Clone, Debug)]
pub struct ExportReport {
    pub triangles_in: usize,
    pub triangles_out: usize,
    pub bytes: u64,
    /// Largest distance from the original surface (every vertex plus one
    /// random point per triangle) to the simplified surface of the same part.
    pub max_deviation_mm: f32,
    /// Mean of the same samples.
    pub mean_deviation_mm: f32,
    /// Wall time of the whole export.
    pub seconds: f32,
}

/// Simplify, shade and write `mesh` (millimetres) as a GLB at `path`.
/// The file is written to a temporary name and renamed into place.
pub fn export_glb(mesh: &Mesh, opts: &GlbOptions, path: &Path) -> anyhow::Result<ExportReport> {
    let (bytes, report) = export_glb_bytes(mesh, opts)?;
    let tmp = path.with_extension("glb.partial");
    std::fs::write(&tmp, &bytes).with_context(|| format!("argent-export: writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("argent-export: renaming to {}", path.display()))?;
    Ok(report)
}

/// [`export_glb`] into memory.
pub fn export_glb_bytes(mesh: &Mesh, opts: &GlbOptions) -> anyhow::Result<(Vec<u8>, ExportReport)> {
    let t0 = Instant::now();
    anyhow::ensure!(!mesh.triangles.is_empty(), "argent-export: the mesh has no triangles");
    if let Some((mask, _, _)) = &opts.patina {
        anyhow::ensure!(
            mask.len() == mesh.positions.len(),
            "argent-export: patina mask has {} values for {} vertices",
            mask.len(),
            mesh.positions.len()
        );
    }
    let table = PartTable::new(mesh, &opts.name);
    let kinds = classify_parts(&table);
    let frozen = frozen_parts(&kinds, &table.counts, opts.target_triangles as usize);
    let mut s = simplify::Simplifier::new(mesh, &table, &frozen);
    let mut target = opts.target_triangles as usize;
    let mut passes = 0;
    loop {
        s.run(target);
        let (simp, map) = s.snapshot();
        let glb = build_glb(&simp, &map, &kinds, opts)?;
        if let Some(limit) = opts.max_bytes
            && glb.len() as u64 > limit
        {
            let n = simp.triangles.len();
            let next = (n as f64 * (limit as f64 / glb.len() as f64) * 0.97) as usize;
            let stuck = s.alive_triangles() > target;
            if passes < MAX_SIZE_PASSES && !stuck && next >= 4 && next < n {
                target = next;
                passes += 1;
                continue;
            }
            anyhow::bail!("argent-export: {} bytes at {n} triangles is still over max_bytes = {limit}", glb.len());
        }
        let (max_dev, mean_dev) = bvh::deviation(mesh, &table.tri_part, &simp);
        let report = ExportReport {
            triangles_in: mesh.triangles.len(),
            triangles_out: simp.triangles.len(),
            bytes: glb.len() as u64,
            max_deviation_mm: max_dev,
            mean_deviation_mm: mean_dev,
            seconds: t0.elapsed().as_secs_f32(),
        };
        return Ok((glb, report));
    }
}

/// Small stone parts are kept verbatim, unless together they would take
/// more than a quarter of the triangle budget.
pub(crate) fn frozen_parts(kinds: &[PartKind], counts: &[usize], target: usize) -> Vec<bool> {
    let small_gem: Vec<bool> =
        kinds.iter().zip(counts).map(|(k, &n)| matches!(k, PartKind::Gem(_)) && n <= FROZEN_GEM_MAX_TRIS).collect();
    let total: usize = small_gem.iter().zip(counts).filter(|(g, _)| **g).map(|(_, &n)| n).sum();
    if total * 4 > target { vec![false; kinds.len()] } else { small_gem }
}

/// `COLOR_0` multiplier: `mix(base, tarnish, t) / base`, so a viewer that
/// ignores vertex colours still shows the bare metal.
fn patina_color(base: [f32; 3], tarnish: [f32; 3], strength: f32, mask: f32) -> [f32; 3] {
    let t = mask * strength;
    let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
    [0, 1, 2].map(|i| {
        let b = base[i].clamp(0.0, 1.0);
        let mix = b + (tarnish[i].clamp(0.0, 1.0) - b) * t;
        if b > 1e-4 { (mix / b).clamp(0.0, 1.0) } else { 1.0 }
    })
}

#[derive(Default)]
struct Materials {
    json: Vec<Value>,
    keys: Vec<String>,
    transmission: bool,
    volume: bool,
    ior: bool,
}

impl Materials {
    fn slot(&mut self, key: String, make: impl FnOnce() -> Value) -> usize {
        if let Some(i) = self.keys.iter().position(|k| *k == key) {
            return i;
        }
        self.keys.push(key);
        self.json.push(make());
        self.json.len() - 1
    }

    fn metal(&mut self, opts: &GlbOptions) -> usize {
        let c = opts.base_color.map(|x| x.clamp(0.0, 1.0) as f64);
        let roughness = opts.roughness.clamp(0.0, 1.0) as f64;
        self.slot("metal".into(), || {
            json!({
                "name": "metal",
                "pbrMetallicRoughness": {
                    "baseColorFactor": [c[0], c[1], c[2], 1.0],
                    "metallicFactor": 1.0,
                    "roughnessFactor": roughness,
                },
            })
        })
    }

    /// Glass-like stone; `thickness_m` feeds `KHR_materials_volume`.
    fn gem(&mut self, g: &Gem, thickness_m: f32) -> usize {
        // Quantise so stones of one kind and size share a material.
        let thickness = ((thickness_m / 0.000_25).round() * 0.000_25) as f64;
        let clear = g.transmission > 0.0;
        self.ior = true;
        self.transmission |= clear;
        self.volume |= clear && thickness > 0.0;
        let c = g.color.map(|x| x.clamp(0.0, 1.0) as f64);
        self.slot(format!("{}:{thickness}", g.name), || {
            let mut ext = json!({ "KHR_materials_ior": { "ior": g.ior as f64 } });
            if clear {
                ext["KHR_materials_transmission"] = json!({ "transmissionFactor": g.transmission as f64 });
                if thickness > 0.0 {
                    ext["KHR_materials_volume"] = json!({ "thicknessFactor": thickness });
                }
            }
            json!({
                "name": format!("gem_{}", g.name),
                "pbrMetallicRoughness": {
                    "baseColorFactor": [c[0], c[1], c[2], 1.0],
                    "metallicFactor": 0.0,
                    "roughnessFactor": g.roughness as f64,
                },
                "extensions": ext,
            })
        })
    }

    fn extensions(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.ior {
            v.push("KHR_materials_ior");
        }
        if self.transmission {
            v.push("KHR_materials_transmission");
        }
        if self.volume {
            v.push("KHR_materials_volume");
        }
        v
    }
}

fn build_glb(simp: &Mesh, map: &[u32], kinds: &[PartKind], opts: &GlbOptions) -> anyhow::Result<Vec<u8>> {
    let table = PartTable::new(simp, &opts.name);
    let kind_of = |p: usize| kinds.get(p).copied().unwrap_or(PartKind::Metal);
    let mask = opts.patina.as_ref().map(|(m, _, _)| carry_scalars(map, m, simp.positions.len()));
    let part_cos: Vec<f64> = (0..table.len())
        .map(|p| {
            let deg = match kind_of(p) {
                PartKind::Gem(_) if table.counts[p] <= FACETED_GEM_MAX_TRIS => FACET_CREASE_DEG,
                _ => SMOOTH_CREASE_DEG,
            };
            deg.to_radians().cos()
        })
        .collect();
    let (split, src) = normals::crease_split(simp, &table.tri_part, &part_cos);
    let mut materials = Materials::default();
    let mut prims = Vec::new();
    let mut local = vec![u32::MAX; split.positions.len()];
    for p in 0..table.len() {
        let Some(part) = split.parts.get(p) else { break };
        let r = part.triangles.start as usize..(part.triangles.end as usize).min(split.triangles.len());
        if r.is_empty() {
            continue;
        }
        let mut verts: Vec<u32> = Vec::new();
        let mut indices = Vec::with_capacity(r.len() * 3);
        for t in &split.triangles[r] {
            for &v in t {
                if local[v as usize] == u32::MAX {
                    local[v as usize] = verts.len() as u32;
                    verts.push(v);
                }
                indices.push(local[v as usize]);
            }
        }
        for &v in &verts {
            local[v as usize] = u32::MAX;
        }
        let positions: Vec<[f32; 3]> =
            verts.iter().map(|&v| split.positions[v as usize].map(|x| (x as f64 * 0.001) as f32)).collect();
        let normals = verts.iter().map(|&v| split.normals[v as usize]).collect();
        let kind = kind_of(p);
        let colors = match (&kind, &mask, &opts.patina) {
            (PartKind::Metal, Some(m), Some((_, tarnish, strength))) => Some(
                verts
                    .iter()
                    .map(|&v| patina_color(opts.base_color, *tarnish, *strength, m[src[v as usize] as usize]))
                    .collect(),
            ),
            _ => None,
        };
        let material = match &kind {
            PartKind::Metal => materials.metal(opts),
            PartKind::Gem(g) => {
                let mut lo = [f32::INFINITY; 3];
                let mut hi = [f32::NEG_INFINITY; 3];
                for q in &positions {
                    for k in 0..3 {
                        lo[k] = lo[k].min(q[k]);
                        hi[k] = hi[k].max(q[k]);
                    }
                }
                let min_extent = (0..3).map(|k| hi[k] - lo[k]).fold(f32::INFINITY, f32::min);
                materials.gem(g, 0.6 * min_extent)
            }
        };
        prims.push(glb::Prim { name: part.name.clone(), positions, normals, colors, indices, material });
    }
    anyhow::ensure!(!prims.is_empty(), "argent-export: nothing left to write");
    Ok(glb::encode(&opts.name, &prims, &materials.json, &materials.extensions()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patina_multiplier() {
        let base = [0.97, 0.96, 0.91];
        assert_eq!(patina_color(base, [0.05; 3], 1.0, 0.0), [1.0, 1.0, 1.0]);
        let c = patina_color(base, [0.05; 3], 1.0, 1.0);
        for i in 0..3 {
            assert!((c[i] * base[i] - 0.05).abs() < 1e-6);
        }
        let half = patina_color(base, [0.05; 3], 0.5, 1.0);
        assert!((half[0] * base[0] - (0.97 + 0.05) / 2.0).abs() < 1e-6);
    }
}
