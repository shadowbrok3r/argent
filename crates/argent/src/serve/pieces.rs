//! Prepared pieces: an LRU of the last few (posing + BVH + cavity mask is
//! the expensive part of a render, seconds for a 2M-triangle casting), and
//! the open/prepare status the UI shows while one loads.

use super::demo;
use super::library::{pretty_name, short_hash};
use super::params::Stone;
use crate::jobs::{self, PieceArgs};
use anyhow::Result;
use argent_core::shot::{gem_from_text, glob, Prepared};
use argent_spectra::alloys::Alloy;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// How many prepared pieces stay in memory.
pub const CAPACITY: usize = 3;

/// An `f32` usable in a hashable key (compared by bit pattern).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Num(pub f32);

impl Eq for Num {}

impl std::hash::Hash for Num {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.0.to_bits().hash(h);
    }
}

/// What to load and how to pose it: the cache key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PieceSpec {
    pub files: Vec<PathBuf>,
    pub flip: bool,
    pub rotate: Option<String>,
    pub units: String,
    pub no_orient: bool,
    /// Pendant chain: auto, on, off.
    pub chain: String,
    /// Scale so the largest dimension is this many mm (unknown units).
    pub fit: Option<Num>,
    /// Multiply coordinates (after units).
    pub scale: Option<Num>,
    pub demo: bool,
}

impl PieceSpec {
    pub fn demo() -> PieceSpec {
        PieceSpec { demo: true, ..PieceSpec::file(PathBuf::new()) }
    }

    pub fn file(path: PathBuf) -> PieceSpec {
        PieceSpec {
            files: if path.as_os_str().is_empty() { vec![] } else { vec![path] },
            flip: false,
            rotate: None,
            units: "mm".into(),
            no_orient: false,
            chain: "auto".into(),
            fit: None,
            scale: None,
            demo: false,
        }
    }

    pub fn id(&self) -> String {
        if self.demo {
            return demo::ID.into();
        }
        let mut key = self.files.iter().map(|f| f.to_string_lossy().into_owned()).collect::<Vec<_>>().join("|");
        if self.flip || self.rotate.is_some() || self.units != "mm" || self.no_orient || self.chain != "auto" || self.fit.is_some() || self.scale.is_some() {
            key += &format!(
                "#{}:{:?}:{}:{}:{}:{:?}:{:?}",
                self.flip,
                self.rotate,
                self.units,
                self.no_orient,
                self.chain,
                self.fit.map(|n| n.0),
                self.scale.map(|n| n.0)
            );
            return short_hash(&key) + "v";
        }
        short_hash(&key)
    }

    pub fn name(&self) -> String {
        if self.demo {
            return demo::NAME.into();
        }
        let stem = self.files.first().and_then(|f| f.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        pretty_name(&stem)
    }

    /// File-name-safe stem for outputs.
    pub fn slug(&self) -> String {
        if self.demo {
            return "demo-ring".into();
        }
        let stem = jobs::piece_name(&self.files);
        slug(&stem)
    }

    /// Rough prepare cost in triangles (binary STL size, else a guess).
    pub fn est_triangles(&self) -> f64 {
        if self.demo {
            return 230_000.0;
        }
        jobs::with_companions(&self.files)
            .iter()
            .filter_map(|f| std::fs::metadata(f).ok().map(|m| (f, m.len())))
            .map(|(f, len)| {
                let ext = f.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                match ext.as_str() {
                    "stl" => len as f64 / 50.0,
                    "ply" => len as f64 / 30.0,
                    "obj" => len as f64 / 60.0,
                    _ => len as f64 / 20.0,
                }
            })
            .sum()
    }
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "piece".into() } else { out.chars().take(60).collect() }
}

/// A prepared piece plus what the UI shows about it.
pub struct Piece {
    pub spec: PieceSpec,
    pub id: String,
    pub name: String,
    pub prepared: Arc<Prepared>,
    pub prepare_secs: f32,
    /// Enclosed volume of the metal parts (stones excluded), mm³.
    pub metal_volume: f64,
}

impl Piece {
    /// The gem a part renders as under `rules` (+ the piece's own defaults),
    /// mirroring `Prepared::scene`'s slot logic, and where it came from.
    pub fn part_gem(&self, rules: &[Stone], part: &argent_mesh::Part) -> (Option<String>, &'static str) {
        if let Some(r) = rules.iter().find(|r| glob(&r.part, &part.name)) {
            return (Some(r.gem.clone()), "set");
        }
        if let Some(r) = self.prepared.default_stones.iter().find(|r| glob(&r.part, &part.name)) {
            return (Some(r.gem.clone()), "file");
        }
        for cand in [part.material.as_deref(), Some(part.name.as_str())].into_iter().flatten() {
            if let Some(g) = gem_from_text(cand) {
                return (Some(g), "name");
            }
        }
        let generic = |t: &str| {
            let t = t.to_ascii_lowercase();
            t == "gem" || t == "gems" || t.starts_with("stone") || t.starts_with("gem_")
        };
        if part.material.as_deref().is_some_and(generic) || generic(&part.name) {
            return (Some("diamond".into()), "auto");
        }
        (None, "metal")
    }

    /// Static facts plus per-part gems under the current rules.
    pub fn info(&self, rules: &[Stone]) -> Value {
        let p = &self.prepared;
        let (lo, hi) = p.mesh.bounds();
        let bore = p.orientation.bore_radius.map(|r| 2.0 * r);
        let parts: Vec<Value> = p
            .mesh
            .parts
            .iter()
            .map(|part| {
                let (gem, source) = self.part_gem(rules, part);
                json!({
                    "name": part.name,
                    "material": part.material,
                    "triangles": part.triangles.end - part.triangles.start,
                    "gem": gem,
                    "source": source,
                })
            })
            .collect();
        let weights: serde_json::Map<String, Value> = Alloy::ALL
            .iter()
            .map(|a| (a.key().to_string(), json!(((self.metal_volume / 1000.0 * a.density() as f64) * 100.0).round() / 100.0)))
            .collect();
        json!({
            "id": self.id,
            "name": self.name,
            "files": jobs::with_companions(&self.spec.files).iter().map(|f| f.display().to_string()).collect::<Vec<_>>(),
            "demo": self.spec.demo,
            "flip": self.spec.flip,
            "rotate": self.spec.rotate,
            "units": self.spec.units,
            "chain": self.spec.chain,
            "fit": self.spec.fit.map(|n| n.0),
            "scale": self.spec.scale.map(|n| n.0),
            "triangles": p.mesh.triangles.len(),
            "vertices": p.mesh.positions.len(),
            "kind": format!("{:?}", p.orientation.kind),
            "confidence": p.orientation.confidence,
            "notes": p.orientation.notes,
            "size_mm": [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]],
            "bore_mm": bore,
            // US size from inner circumference (RingDesigner's scale), as `argent info`.
            "us_size": bore.map(|d| ((std::f32::consts::PI * d - 36.5) / 2.55 * 4.0).round() / 4.0),
            "metal_volume_mm3": (self.metal_volume * 10.0).round() / 10.0,
            "weights_g": weights,
            "parts": parts,
            "prepare_secs": self.prepare_secs,
        })
    }
}

/// Signed-tetrahedra volume of the metal: every part except stones. The
/// part holding most of the triangles is the casting even when its name
/// happens to contain a gem word ("ophidian-amethyst-serpent").
fn metal_volume(piece: &Piece) -> f64 {
    let m = &piece.prepared.mesh;
    let total_tris = m.triangles.len().max(1) as u32;
    let mut total = 0.0f64;
    for part in &m.parts {
        let tris = part.triangles.end - part.triangles.start;
        let (gem, source) = piece.part_gem(&[], part);
        if gem.is_some() && !(source == "name" && tris * 2 > total_tris) {
            continue;
        }
        let mut v = 0.0f64;
        for t in &m.triangles[part.triangles.start as usize..part.triangles.end as usize] {
            let [a, b, c] = t.map(|i| m.positions[i as usize].map(|x| x as f64));
            v += (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0])) / 6.0;
        }
        total += v.abs();
    }
    if m.parts.is_empty() { m.volume() } else { total }
}

/// Load + pose + accelerate. Blocking (seconds for big castings).
pub fn prepare(spec: &PieceSpec) -> Result<Piece> {
    let t0 = Instant::now();
    let prepared = if spec.demo {
        let mesh = demo::mesh();
        let pose = demo::pose(&mesh);
        Prepared::new(mesh, Some(pose))
    } else {
        jobs::prepare(&PieceArgs {
            files: spec.files.clone(),
            no_orient: spec.no_orient,
            rotate: spec.rotate.clone(),
            flip: spec.flip,
            units: spec.units.clone(),
            chain: spec.chain.clone(),
            fit: spec.fit.map(|n| n.0),
            scale: spec.scale.map(|n| n.0),
            ..Default::default()
        })?
    };
    let mut piece = Piece {
        id: spec.id(),
        name: spec.name(),
        spec: spec.clone(),
        prepared: Arc::new(prepared),
        prepare_secs: t0.elapsed().as_secs_f32(),
        metal_volume: 0.0,
    };
    piece.metal_volume = metal_volume(&piece);
    Ok(piece)
}

pub enum Status {
    Idle,
    Preparing { id: String, name: String, started: Instant, est_secs: f32 },
    Failed { id: String, name: String, error: String },
}

pub struct Pieces {
    lru: Vec<Arc<Piece>>,
    pub current: Option<Arc<Piece>>,
    pub status: Status,
    /// Bumped by every open request; a prepare that finishes after a newer
    /// request only fills the cache.
    pub token: u64,
    /// Measured prepare speed, seconds per triangle.
    pub secs_per_tri: f64,
}

impl Default for Pieces {
    fn default() -> Self {
        Pieces { lru: vec![], current: None, status: Status::Idle, token: 0, secs_per_tri: 2.5e-6 }
    }
}

impl Pieces {
    pub fn get(&mut self, spec: &PieceSpec) -> Option<Arc<Piece>> {
        let i = self.lru.iter().position(|p| &p.spec == spec)?;
        let p = self.lru.remove(i);
        self.lru.push(p.clone());
        Some(p)
    }

    pub fn insert(&mut self, p: Arc<Piece>) {
        self.lru.retain(|q| q.spec != p.spec);
        self.lru.push(p);
        while self.lru.len() > CAPACITY {
            // Never evict the piece on screen.
            match self.lru.iter().position(|q| !self.current.as_ref().is_some_and(|c| Arc::ptr_eq(c, q))) {
                Some(i) => {
                    self.lru.remove(i);
                }
                None => break,
            }
        }
    }

    pub fn cached_ids(&self) -> Vec<String> {
        self.lru.iter().map(|p| p.id.clone()).collect()
    }

    pub fn learn(&mut self, triangles: usize, secs: f32) {
        if triangles > 10_000 && secs > 0.05 {
            let r = secs as f64 / triangles as f64;
            self.secs_per_tri = 0.5 * self.secs_per_tri + 0.5 * r;
        }
    }

    /// The `piece` block of the studio state.
    pub fn status_json(&self, rules: &[Stone]) -> Value {
        let current = self.current.as_ref().map(|p| p.info(rules));
        match &self.status {
            Status::Idle => json!({"status": if current.is_some() { "ready" } else { "none" }, "current": current, "cached": self.cached_ids()}),
            Status::Preparing { id, name, started, est_secs } => {
                let el = started.elapsed().as_secs_f32();
                json!({
                    "status": "preparing",
                    "id": id,
                    "name": name,
                    "elapsed": el,
                    "estimate": est_secs,
                    "progress": (el / est_secs.max(0.1)).min(0.95),
                    "current": current,
                    "cached": self.cached_ids(),
                })
            }
            Status::Failed { id, name, error } => {
                json!({"status": "failed", "id": id, "name": name, "error": error, "current": current, "cached": self.cached_ids()})
            }
        }
    }
}
