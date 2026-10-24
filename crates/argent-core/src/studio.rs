//! Studio presets: where the softboxes, flags, backdrop and floor go.
//!
//! Positions are in millimetres around a piece centred at the origin and
//! sized ~20–40 mm; [`Studio::build`] scales the rig to the piece so a
//! pendant and a bangle get the same light.

use crate::geom::V3;
use crate::light::{Env, Floor, Panel};
use crate::material::Material;
use argent_spectra::{Blackbody, RgbAlbedo, Spectrum};
use std::sync::Arc;

/// A softbox emitting `intensity` × a blackbody at `kelvin` normalized to
/// luminance 1.
pub fn emitter(kelvin: f32, intensity: f32) -> Arc<dyn Spectrum> {
    struct Scaled(Blackbody, f32);
    impl Spectrum for Scaled {
        fn eval(&self, l: f32) -> f32 {
            self.0.eval(l) * self.1
        }
    }
    let bb = Blackbody::new(kelvin);
    let y = argent_spectra::spectrum::luminance_y(&bb).max(1e-9);
    crate::tab::bake(Arc::new(Scaled(bb, intensity / y)))
}

fn rgb_spec(v: [f32; 3]) -> Arc<dyn Spectrum> {
    crate::tab::bake(Arc::new(argent_spectra::RgbIlluminant::new(v)))
}

pub struct StudioRig {
    pub panels: Vec<Panel>,
    pub env: Env,
    /// Floor material (index assigned by the caller) and fade radii (mm,
    /// before scaling).
    pub floor: Option<(Material, f32, f32)>,
    /// Suggested exposure for this rig, EV.
    pub exposure_ev: f32,
    /// Colour temperature the camera is white-balanced to (`None` keeps the
    /// rig's own colour, e.g. ember's fire-and-ice contrast).
    pub white_kelvin: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Studio {
    /// The brand look: black backdrop, strip softboxes that draw long
    /// highlights down the metal, black acrylic floor.
    Noir,
    /// Marketplace listing: white seamless, soft tent light, black flags for
    /// contrast lines on polished metal.
    White,
    /// Jewelry-box velvet under a warm key.
    Velvet,
    /// Underworld: ember rim lights behind, cold key in front.
    Ember,
    /// One big window, soft daylight, grey stone floor.
    Window,
}

impl Studio {
    pub const ALL: &'static [Studio] = &[Studio::Noir, Studio::White, Studio::Velvet, Studio::Ember, Studio::Window];

    pub fn from_name(s: &str) -> Option<Studio> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "noir" | "brand" | "dark" | "black" => Studio::Noir,
            "white" | "listing" | "etsy" | "seamless" => Studio::White,
            "velvet" | "box" => Studio::Velvet,
            "ember" | "underworld" | "forge" | "fire" => Studio::Ember,
            "window" | "daylight" | "natural" => Studio::Window,
            _ => return None,
        })
    }

    pub fn key(self) -> &'static str {
        match self {
            Studio::Noir => "noir",
            Studio::White => "white",
            Studio::Velvet => "velvet",
            Studio::Ember => "ember",
            Studio::Window => "window",
        }
    }

    /// Build the rig for a piece whose bounding radius is `radius` mm and
    /// whose lowest point is at `floor_y`.
    pub fn build(self, radius: f32) -> StudioRig {
        // Every preset is authored for a ~15 mm radius piece.
        let k = (radius / 15.0).max(0.2);
        let at = |x: f32, y: f32, z: f32| V3::new(x, y, z) * k;
        let o = V3::ZERO;
        let up = V3::Y;
        let box_ = |name: &str, pos: V3, w: f32, h: f32, kelvin: f32, i: f32| {
            Panel::facing(name, pos, o, w * k, h * k, up, Some(emitter(kelvin, i)))
        };
        let flag = |name: &str, pos: V3, w: f32, h: f32| Panel::facing(name, pos, o, w * k, h * k, up, None);
        match self {
            Studio::Noir => StudioRig {
                panels: vec![
                    box_("key-top", at(0.0, 270.0, 110.0), 340.0, 240.0, 5600.0, 3.2),
                    box_("strip-left", at(-230.0, 70.0, -50.0), 70.0, 540.0, 5600.0, 7.5),
                    box_("strip-right", at(230.0, 70.0, -50.0), 70.0, 540.0, 5600.0, 7.5),
                    box_("fill-front", at(30.0, 90.0, 330.0), 520.0, 300.0, 5400.0, 0.9),
                    box_("kicker-back", at(0.0, 260.0, -230.0), 300.0, 90.0, 6000.0, 1.4),
                ],
                env: Env::Gradient {
                    top: rgb_spec([0.010, 0.010, 0.011]),
                    horizon: rgb_spec([0.006, 0.006, 0.0065]),
                    bottom: rgb_spec([0.002, 0.002, 0.002]),
                    power: 0.6,
                },
                floor: Some((Material::plastic([0.003, 0.003, 0.0032], 1.49, 0.09), 40.0, 130.0)),
                exposure_ev: 0.0,
                white_kelvin: Some(5600.0),
            },
            Studio::White => StudioRig {
                panels: vec![
                    box_("tent-top", at(0.0, 300.0, 60.0), 460.0, 420.0, 5800.0, 2.6),
                    box_("tent-left", at(-270.0, 110.0, 90.0), 260.0, 380.0, 5800.0, 1.7),
                    box_("tent-right", at(270.0, 110.0, 90.0), 260.0, 380.0, 5800.0, 1.7),
                    box_("tent-front", at(0.0, 70.0, 340.0), 460.0, 220.0, 5800.0, 1.2),
                    // Black cards: polished metal needs something dark to
                    // reflect or it reads as a white blob.
                    flag("flag-left", at(-170.0, 60.0, -170.0), 170.0, 420.0),
                    flag("flag-right", at(170.0, 60.0, -170.0), 170.0, 420.0),
                    flag("flag-back", at(0.0, 120.0, -260.0), 300.0, 160.0),
                    flag("flag-front-low", at(0.0, 10.0, 260.0), 420.0, 60.0),
                ],
                env: Env::Gradient {
                    top: rgb_spec([1.8, 1.8, 1.8]),
                    horizon: rgb_spec([0.55, 0.55, 0.56]),
                    bottom: rgb_spec([0.9, 0.9, 0.9]),
                    power: 0.45,
                },
                floor: Some((Material::diffuse([0.86, 0.86, 0.86]), 80.0, 260.0)),
                exposure_ev: 0.0,
                white_kelvin: Some(5800.0),
            },
            Studio::Velvet => StudioRig {
                panels: vec![
                    box_("key-warm", at(-160.0, 220.0, 160.0), 220.0, 220.0, 3400.0, 3.2),
                    box_("strip-right", at(240.0, 70.0, -40.0), 70.0, 460.0, 4200.0, 4.0),
                    box_("back-rim", at(-60.0, 120.0, -280.0), 260.0, 80.0, 3800.0, 2.0),
                ],
                env: Env::Gradient {
                    top: rgb_spec([0.006, 0.004, 0.004]),
                    horizon: rgb_spec([0.012, 0.005, 0.005]),
                    bottom: rgb_spec([0.004, 0.001, 0.001]),
                    power: 0.7,
                },
                floor: Some((Material::diffuse([0.10, 0.012, 0.02]), 60.0, 200.0)),
                exposure_ev: 0.0,
                // Balanced cooler than the 3400 K key so the velvet stays warm.
                white_kelvin: Some(4300.0),
            },
            Studio::Ember => StudioRig {
                panels: vec![
                    box_("ember-left", at(-210.0, 40.0, -150.0), 80.0, 420.0, 1900.0, 9.0),
                    box_("ember-right", at(210.0, 40.0, -150.0), 80.0, 420.0, 2100.0, 9.0),
                    box_("cold-key", at(60.0, 220.0, 220.0), 220.0, 180.0, 9000.0, 1.4),
                    box_("floor-glow", at(0.0, -5.0, -220.0), 260.0, 60.0, 1700.0, 2.5),
                ],
                env: Env::Gradient {
                    top: rgb_spec([0.002, 0.002, 0.003]),
                    horizon: rgb_spec([0.010, 0.004, 0.002]),
                    bottom: rgb_spec([0.001, 0.0005, 0.0003]),
                    power: 0.8,
                },
                floor: Some((Material::plastic([0.004, 0.003, 0.003], 1.5, 0.12), 40.0, 150.0)),
                exposure_ev: 0.0,
                white_kelvin: None,
            },
            Studio::Window => StudioRig {
                panels: vec![
                    box_("window", at(-300.0, 160.0, 80.0), 300.0, 380.0, 6500.0, 3.0),
                    box_("bounce", at(260.0, 80.0, 120.0), 300.0, 300.0, 5800.0, 0.35),
                    flag("flag-back", at(160.0, 60.0, -220.0), 160.0, 300.0),
                ],
                env: Env::Gradient {
                    top: rgb_spec([0.30, 0.32, 0.36]),
                    horizon: rgb_spec([0.20, 0.20, 0.21]),
                    bottom: rgb_spec([0.08, 0.08, 0.08]),
                    power: 0.8,
                },
                floor: Some((Material::plastic([0.30, 0.29, 0.28], 1.5, 0.35), 70.0, 220.0)),
                exposure_ev: 0.0,
                white_kelvin: Some(6500.0),
            },
        }
    }
}

/// Floor at `y` with fades scaled like the rig.
pub fn floor_at(y: f32, material: usize, fade: (f32, f32), radius: f32) -> Floor {
    let k = (radius / 15.0).max(0.2);
    Floor { y, material, fade_start: fade.0 * k, fade_end: fade.1 * k }
}

#[allow(dead_code)]
fn _unused(_: RgbAlbedo) {}
