//! The live set: every knob the studio exposes, validated, plus the
//! conversions into argent-core types (live view) and CLI [`ShotArgs`]
//! (final renders reuse `jobs.rs`, so a studio still matches `argent render`).

use crate::jobs::{self, ShotArgs};
use anyhow::{bail, Context, Result};
use argent_core::camera::Lens;
use argent_core::shot::{Look, StoneRule};
use argent_core::studio::Studio;
use argent_core::views::View;
use argent_spectra::alloys::Alloy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Preview pixel budget (540×960 for a story frame).
pub const PREVIEW_PIXELS: f64 = 540.0 * 960.0;

pub const GRADES: &[&str] = &["punchy", "base", "golden"];

/// Named output formats: the live preview renders at the chosen format's
/// aspect, and a still renders at its full size.
pub const FORMATS: &[(&str, &str, u32, u32)] = &[
    ("story", "Story 9:16", 1080, 1920),
    ("listing", "Listing 1:1", 2000, 2000),
    ("wide", "Wide 16:9", 1920, 1080),
];

/// One part → gem assignment. `part` is a case-insensitive glob over part
/// names (`stone_*`), `gem` a name from `argent_spectra::gems::names()`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stone {
    pub part: String,
    pub gem: String,
}

/// The whole set as one flat, JSON-friendly record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Params {
    /// Alloy key (`sterling`, `gold14k`, `bronze`, …).
    pub metal: String,
    /// 0 = bright polish, 1 = black recesses.
    pub patina: f32,
    /// Polish: 0.03 mirror … 0.35 satin.
    pub roughness: f32,
    /// Studio key (`noir`, `white`, `velvet`, `ember`, `window`).
    pub studio: String,
    /// Exposure compensation, EV.
    pub exposure: f32,
    /// AgX look: `punchy`, `base`, `golden`.
    pub grade: String,
    pub stones: Vec<Stone>,
    /// Preset the camera came from, or `custom` once orbited.
    pub view: String,
    pub azimuth: f32,
    pub elevation: f32,
    /// Fraction of the frame the piece fills.
    pub fill: f32,
    /// Piece turned about the vertical, degrees (non-zero frames for a full turn, like `argent render --turn`).
    pub turntable: f32,
    /// Lens focal length, mm (full frame).
    pub focal: f32,
    /// f-number, 0 = everything sharp.
    pub fstop: f32,
    /// `story`, `listing`, `wide` or `WxH`.
    pub format: String,
    /// Denoise the live preview.
    pub denoise: bool,
    /// The live view stops refining here.
    pub max_spp: u32,
}

impl Default for Params {
    fn default() -> Self {
        let v = View::named("hero").expect("hero view");
        let lk = Look::default();
        let lens = Lens::default();
        Params {
            metal: lk.alloy().key().into(),
            patina: lk.patina,
            roughness: lk.roughness,
            studio: lk.studio.key().into(),
            exposure: lk.exposure_ev,
            grade: lk.grade.clone(),
            stones: vec![],
            view: "hero".into(),
            azimuth: v.azimuth_deg,
            elevation: v.elevation_deg,
            fill: v.fill,
            turntable: 0.0,
            focal: lens.focal_mm,
            fstop: lens.f_stop,
            format: "story".into(),
            denoise: true,
            max_spp: 512,
        }
    }
}

/// A partial update: every field optional, unknown fields rejected (so an
/// agent's typo is an error, not a silent no-op).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub metal: Option<String>,
    pub patina: Option<f32>,
    #[serde(alias = "polish")]
    pub roughness: Option<f32>,
    pub studio: Option<String>,
    #[serde(alias = "exposure_ev")]
    pub exposure: Option<f32>,
    pub grade: Option<String>,
    /// Replace every stone rule.
    pub stones: Option<Vec<Stone>>,
    /// Set one part's gem (`"gem": "metal"` clears it).
    pub stone: Option<Stone>,
    /// A preset name: sets azimuth, elevation and fill.
    pub view: Option<String>,
    pub azimuth: Option<f32>,
    pub elevation: Option<f32>,
    pub fill: Option<f32>,
    pub turntable: Option<f32>,
    pub focal: Option<f32>,
    pub fstop: Option<f32>,
    pub format: Option<String>,
    pub denoise: Option<bool>,
    pub max_spp: Option<u32>,
}

impl Patch {
    /// Parse a JSON object, naming the offending field on error.
    pub fn from_value(v: Value) -> Result<Patch> {
        serde_json::from_value(v).map_err(|e| anyhow::anyhow!("{e}"))
    }
}

fn finite(v: f32, what: &str) -> Result<f32> {
    if !v.is_finite() {
        bail!("{what} must be a finite number");
    }
    Ok(v)
}

fn wrap180(a: f32) -> f32 {
    let r = (a + 180.0).rem_euclid(360.0) - 180.0;
    if r == -180.0 { 180.0 } else { r }
}

/// Canonical preset name for a view name or alias.
pub fn view_name(name: &str) -> Option<&'static str> {
    let v = View::named(name)?;
    View::NAMES.iter().copied().find(|n| {
        View::named(n).is_some_and(|w| w.azimuth_deg == v.azimuth_deg && w.elevation_deg == v.elevation_deg && w.fill == v.fill)
    })
}

/// `story`, `listing`, `wide` or any `WxH` (64…8192 a side, ≤ 40 Mpx).
pub fn parse_format(s: &str) -> Result<(String, u32, u32)> {
    let k = s.trim().to_ascii_lowercase();
    if let Some((key, _, w, h)) = FORMATS.iter().find(|f| f.0 == k) {
        return Ok((key.to_string(), *w, *h));
    }
    let (w, h) = jobs::parse_size(&k).with_context(|| format!("format {s:?}: use story, listing, wide or WxH"))?;
    if !(64..=8192).contains(&w) || !(64..=8192).contains(&h) || (w as u64 * h as u64) > 40_000_000 {
        bail!("format {s:?}: each side must be 64…8192 px and the frame ≤ 40 Mpx");
    }
    let key = FORMATS.iter().find(|f| f.2 == w && f.3 == h).map(|f| f.0.to_string()).unwrap_or(format!("{w}x{h}"));
    Ok((key, w, h))
}

pub fn validate_gem(gem: &str) -> Result<String> {
    let g = gem.trim().to_ascii_lowercase();
    match argent_spectra::gems::get(&g) {
        Some(o) => Ok(o.name.to_string()),
        None => bail!("unknown gem {gem:?}; known: {}", argent_spectra::gems::names().join(", ")),
    }
}

fn is_metal_word(g: &str) -> bool {
    matches!(g.trim().to_ascii_lowercase().as_str(), "" | "metal" | "none" | "off")
}

impl Params {
    /// Apply a partial update; on error nothing changes.
    pub fn apply(&mut self, p: &Patch) -> Result<()> {
        let mut n = self.clone();
        if let Some(m) = &p.metal {
            let a = Alloy::from_name(m).with_context(|| {
                format!("unknown metal {m:?}; known: {}", Alloy::ALL.iter().map(|a| a.key()).collect::<Vec<_>>().join(", "))
            })?;
            n.metal = a.key().into();
        }
        if let Some(v) = p.patina {
            n.patina = finite(v, "patina")?.clamp(0.0, 1.0);
        }
        if let Some(v) = p.roughness {
            n.roughness = finite(v, "roughness")?.clamp(0.01, 0.6);
        }
        if let Some(s) = &p.studio {
            let st = Studio::from_name(s).with_context(|| {
                format!("unknown studio {s:?}; known: {}", Studio::ALL.iter().map(|s| s.key()).collect::<Vec<_>>().join(", "))
            })?;
            n.studio = st.key().into();
        }
        if let Some(v) = p.exposure {
            n.exposure = finite(v, "exposure")?.clamp(-8.0, 8.0);
        }
        if let Some(g) = &p.grade {
            let g = g.trim().to_ascii_lowercase();
            if !GRADES.contains(&g.as_str()) {
                bail!("unknown grade {g:?}; known: {}", GRADES.join(", "));
            }
            n.grade = g;
        }
        if let Some(list) = &p.stones {
            let mut out = vec![];
            for s in list {
                if s.part.trim().is_empty() {
                    bail!("stone rule needs a part name or glob");
                }
                if is_metal_word(&s.gem) {
                    continue;
                }
                out.push(Stone { part: s.part.trim().into(), gem: validate_gem(&s.gem)? });
            }
            n.stones = out;
        }
        if let Some(s) = &p.stone {
            let part = s.part.trim().to_string();
            if part.is_empty() {
                bail!("stone needs a part name or glob");
            }
            n.stones.retain(|r| !r.part.eq_ignore_ascii_case(&part));
            if !is_metal_word(&s.gem) {
                // Most specific first: an exact part beats an earlier glob.
                n.stones.insert(0, Stone { part, gem: validate_gem(&s.gem)? });
            }
        }
        if let Some(name) = &p.view {
            let key = view_name(name).with_context(|| format!("unknown view {name:?}; known: {}", View::NAMES.join(", ")))?;
            let v = View::named(key).expect("preset");
            n.view = key.into();
            n.azimuth = v.azimuth_deg;
            n.elevation = v.elevation_deg;
            n.fill = v.fill;
        }
        let mut orbit = false;
        if let Some(a) = p.azimuth {
            n.azimuth = wrap180(finite(a, "azimuth")?);
            orbit = true;
        }
        if let Some(e) = p.elevation {
            n.elevation = finite(e, "elevation")?.clamp(-30.0, 89.0);
            orbit = true;
        }
        if let Some(f) = p.fill {
            n.fill = finite(f, "fill")?.clamp(0.1, 3.0);
            orbit = true;
        }
        if orbit {
            n.view = View::NAMES
                .iter()
                .find(|k| {
                    View::named(k).is_some_and(|v| {
                        (v.azimuth_deg - n.azimuth).abs() < 0.05 && (v.elevation_deg - n.elevation).abs() < 0.05 && (v.fill - n.fill).abs() < 0.001
                    })
                })
                .map(|k| k.to_string())
                .unwrap_or_else(|| "custom".into());
        }
        if let Some(t) = p.turntable {
            let t = finite(t, "turntable")?.rem_euclid(360.0);
            n.turntable = if t > 359.95 { 0.0 } else { t };
        }
        if let Some(f) = p.focal {
            n.focal = finite(f, "focal")?.clamp(18.0, 600.0);
        }
        if let Some(f) = p.fstop {
            let f = finite(f, "fstop")?;
            n.fstop = if f <= 0.0 { 0.0 } else { f.clamp(0.7, 128.0) };
        }
        if let Some(f) = &p.format {
            n.format = parse_format(f)?.0;
        }
        if let Some(d) = p.denoise {
            n.denoise = d;
        }
        if let Some(m) = p.max_spp {
            n.max_spp = m.clamp(1, 65_536);
        }
        *self = n;
        Ok(())
    }

    /// Full-size frame of the format.
    pub fn size(&self) -> (u32, u32) {
        parse_format(&self.format).map(|(_, w, h)| (w, h)).unwrap_or((1080, 1920))
    }

    /// The format scaled to the preview budget (even sides, never upscaled).
    pub fn preview_size(&self) -> (u32, u32) {
        let (w, h) = self.size();
        let s = (PREVIEW_PIXELS / (w as f64 * h as f64)).sqrt().min(1.0);
        let even = |v: f64| (((v / 2.0).round() as u32) * 2).max(16);
        (even(w as f64 * s), even(h as f64 * s))
    }

    pub fn look(&self) -> Look {
        Look {
            metal: self.metal.clone(),
            roughness: self.roughness,
            patina: self.patina,
            stones: self.stones.iter().map(|s| StoneRule { part: s.part.clone(), gem: s.gem.clone() }).collect(),
            studio: Studio::from_name(&self.studio).unwrap_or(Studio::Noir),
            exposure_ev: self.exposure,
            grade: self.grade.clone(),
        }
    }

    pub fn view(&self) -> View {
        View { azimuth_deg: self.azimuth, elevation_deg: self.elevation, fill: self.fill }
    }

    pub fn lens(&self) -> Lens {
        Lens { focal_mm: self.focal, f_stop: self.fstop }
    }

    /// CLI shot arguments for a final render of this set.
    pub fn shot(&self, size: &str, spp: u32) -> ShotArgs {
        ShotArgs {
            metal: self.metal.clone(),
            patina: self.patina,
            roughness: self.roughness,
            studio: self.studio.clone(),
            view: if self.view == "custom" { "hero".into() } else { self.view.clone() },
            angle: Some(format!("{},{}", self.azimuth, self.elevation)),
            fill: Some(self.fill),
            size: size.into(),
            spp,
            stones: self.stones.iter().map(|s| format!("{}={}", s.part, s.gem)).collect(),
            focal: self.focal,
            fstop: self.fstop,
            exposure: self.exposure,
            grade: self.grade.clone(),
            no_denoise: false,
            max_depth: 32,
        }
    }

    /// What the scene depends on (rebuild + restart when it changes).
    pub fn scene_key(&self) -> (String, u32, u32, String, Vec<Stone>) {
        (self.metal.clone(), self.patina.to_bits(), self.roughness.to_bits(), self.studio.clone(), self.stones.clone())
    }

    /// What the camera depends on (restart when it changes).
    pub fn camera_key(&self) -> [u32; 8] {
        let (w, h) = self.preview_size();
        [self.azimuth.to_bits(), self.elevation.to_bits(), self.fill.to_bits(), self.turntable.to_bits(), self.focal.to_bits(), self.fstop.to_bits(), w, h]
    }

    /// What only development depends on (re-develop, keep the samples).
    pub fn develop_key(&self) -> (u32, String, bool) {
        (self.exposure.to_bits(), self.grade.clone(), self.denoise)
    }
}

/// Approximate display swatch for a metal chip.
fn swatch(a: Alloy) -> &'static str {
    match a {
        Alloy::FineSilver => "#e6e6e3",
        Alloy::Sterling => "#d8d8d4",
        Alloy::Gold24k => "#e9b54a",
        Alloy::Gold18kYellow => "#e2b65c",
        Alloy::Gold14kYellow => "#d6ad64",
        Alloy::Gold10kYellow => "#cba56c",
        Alloy::Gold14kRose => "#d89c85",
        Alloy::Gold14kWhite => "#d9d8d2",
        Alloy::Platinum => "#cfcfcb",
        Alloy::Palladium => "#c8c6c0",
        Alloy::Rhodium => "#dcdcdc",
        Alloy::Copper => "#c97d56",
        Alloy::Bronze => "#b68b53",
        Alloy::Brass => "#caa850",
    }
}

fn studio_label(s: Studio) -> (&'static str, &'static str) {
    match s {
        Studio::Noir => ("Noir", "Black set, strip softboxes, black acrylic floor — the brand look"),
        Studio::White => ("White", "White seamless tent with black flags — marketplace listings"),
        Studio::Velvet => ("Velvet", "Jewelry-box velvet under a warm key"),
        Studio::Ember => ("Ember", "Underworld: ember rim lights, cold key"),
        Studio::Window => ("Window", "One big window, soft daylight, stone floor"),
    }
}

/// Everything a client needs to draw the controls.
pub fn options() -> Value {
    json!({
        "metals": Alloy::ALL.iter().map(|&a| json!({"key": a.key(), "name": a.display(), "swatch": swatch(a)})).collect::<Vec<_>>(),
        "studios": Studio::ALL.iter().map(|&s| {
            let (name, blurb) = studio_label(s);
            json!({"key": s.key(), "name": name, "blurb": blurb})
        }).collect::<Vec<_>>(),
        "views": View::NAMES.iter().map(|n| {
            let v = View::named(n).expect("preset");
            json!({"key": n, "azimuth": v.azimuth_deg, "elevation": v.elevation_deg, "fill": v.fill})
        }).collect::<Vec<_>>(),
        "gems": argent_spectra::gems::names(),
        "grades": GRADES,
        "formats": FORMATS.iter().map(|(k, name, w, h)| json!({"key": k, "name": name, "w": w, "h": h})).collect::<Vec<_>>(),
        "still_spp": [64, 128, 256, 512, 1024],
        "preview_spp": [128, 256, 512, 1024, 4096],
        "sheet": {"metals": ["sterling", "gold14k", "bronze"], "views": ["hero", "front", "side"], "size": "1600x1600", "spp": 128},
        "reel": {"seconds": 6.0, "fps": 30, "spp": 32, "size": "1080x1920"},
        "defaults": Params::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_validates_and_canonicalises() {
        let mut p = Params::default();
        let patch = Patch::from_value(json!({"metal": "gold-filled", "studio": "listing", "view": "34", "polish": 0.2})).unwrap();
        p.apply(&patch).unwrap();
        assert_eq!(p.metal, "gold14k");
        assert_eq!(p.studio, "white");
        assert_eq!(p.view, "three-quarter");
        assert_eq!(p.roughness, 0.2);
        assert!(p.apply(&Patch::from_value(json!({"metal": "unobtanium"})).unwrap()).is_err());
        assert_eq!(p.metal, "gold14k");
        assert!(Patch::from_value(json!({"metl": "gold"})).is_err());
        p.apply(&Patch::from_value(json!({"azimuth": 200.0})).unwrap()).unwrap();
        assert_eq!(p.view, "custom");
        assert!((p.azimuth + 160.0).abs() < 1e-3);
    }

    #[test]
    fn preview_sizes() {
        let mut p = Params::default();
        assert_eq!(p.preview_size(), (540, 960));
        p.format = "listing".into();
        assert_eq!(p.preview_size(), (720, 720));
        p.format = "wide".into();
        assert_eq!(p.preview_size(), (960, 540));
    }

    #[test]
    fn stones_merge() {
        let mut p = Params::default();
        p.apply(&Patch::from_value(json!({"stones": [{"part": "stone_*", "gem": "diamond"}]})).unwrap()).unwrap();
        p.apply(&Patch::from_value(json!({"stone": {"part": "stone_03", "gem": "sapphire"}})).unwrap()).unwrap();
        assert_eq!(p.stones[0], Stone { part: "stone_03".into(), gem: "sapphire".into() });
        p.apply(&Patch::from_value(json!({"stone": {"part": "stone_03", "gem": "metal"}})).unwrap()).unwrap();
        assert_eq!(p.stones.len(), 1);
    }
}
