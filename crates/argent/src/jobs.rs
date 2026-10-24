//! The work behind each subcommand.

use anyhow::{bail, Context, Result};
use argent_core::camera::Lens;
use argent_core::shot::{grade, render_image, Look, Prepared, Quality, StoneRule};
use argent_core::studio::Studio;
use argent_core::views::View;
use argent_mesh::{LoadOptions, Pose};
use argent_post::Rgb8;
use argent_spectra::alloys::Alloy;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(clap::Args, Clone, Debug)]
pub struct PieceArgs {
    /// Mesh file(s) for one piece (STL/OBJ/PLY/3MF). Several files become
    /// parts: `ring.stl eye.stl` (name stone parts to match `--stone`).
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    /// Keep the file's own orientation (only centre it).
    #[arg(long)]
    pub no_orient: bool,
    /// Extra rotation after posing: "x,y,z" degrees, or ordered axis steps
    /// applied left to right, e.g. "z-90,y-90" (stand up, then face camera).
    #[arg(long, allow_hyphen_values = true)]
    pub rotate: Option<String>,
    /// Turn the piece around (180° about the vertical) if it faces away.
    #[arg(long)]
    pub flip: bool,
    /// Units of the file: mm (default), cm, m, in.
    #[arg(long, default_value = "mm")]
    pub units: String,
    /// Hang a cable chain from the bail: auto (pendants), on, off.
    #[arg(long, default_value = "auto")]
    pub chain: String,
    /// Scale the mesh so its largest dimension is this many mm (for models
    /// in unknown units, e.g. a sculpt to be shown at pendant size).
    #[arg(long)]
    pub fit: Option<f32>,
    /// Multiply all coordinates by this factor (after --units).
    #[arg(long)]
    pub scale: Option<f32>,
}

#[derive(clap::Args, Clone, Debug)]
pub struct ShotArgs {
    /// sterling, fine-silver, gold14k (= gold-filled), gold18k, gold24k, rose14k, white14k, bronze, brass, copper, platinum…
    #[arg(long, default_value = "sterling")]
    pub metal: String,
    /// Antique strength: 0 = bright polish, 1 = black recesses.
    #[arg(long, default_value_t = 0.8)]
    pub patina: f32,
    /// Polish: 0.03 mirror … 0.35 satin.
    #[arg(long, default_value_t = 0.07)]
    pub roughness: f32,
    /// noir (brand), white (listing), velvet, ember, window.
    #[arg(long, default_value = "noir")]
    pub studio: String,
    /// hero, front, side, top, three-quarter, back, low, detail.
    #[arg(long, default_value = "hero")]
    pub view: String,
    /// Override the view's angles: "azimuth,elevation" in degrees.
    #[arg(long)]
    pub angle: Option<String>,
    /// Fraction of the frame the piece fills.
    #[arg(long)]
    pub fill: Option<f32>,
    /// WxH, or story (1080x1920), square (2048x2048), listing (2000x2000), wide (1920x1080).
    #[arg(long, default_value = "1080x1920")]
    pub size: String,
    /// Samples per pixel.
    #[arg(long, default_value_t = 128)]
    pub spp: u32,
    /// Stone assignment "part-glob=gem", e.g. --stone 'eye*=sapphire'.
    #[arg(long = "stone")]
    pub stones: Vec<String>,
    /// Lens focal length (mm, full frame).
    #[arg(long, default_value_t = 100.0)]
    pub focal: f32,
    /// f-number for depth of field (0 = everything sharp). Default: f/16 for
    /// mood studios, sharp for the white listing studio and sheets.
    #[arg(long)]
    pub fstop: Option<f32>,
    /// Exposure compensation, EV.
    #[arg(long, default_value_t = 0.0)]
    pub exposure: f32,
    /// AgX look: punchy, base, golden.
    #[arg(long, default_value = "punchy")]
    pub grade: String,
    #[arg(long)]
    pub no_denoise: bool,
    #[arg(long, default_value_t = 32)]
    pub max_depth: u32,
    /// Light with an equirectangular HDR environment instead of the studio's softboxes.
    #[arg(long)]
    pub hdri: Option<PathBuf>,
    /// Turn the environment, degrees.
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    pub hdri_rotate: f32,
    /// Environment exposure, EV.
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    pub hdri_exposure: f32,
}

/// When set, print ZeroClaw delivery markers for final outputs.
pub static MARKERS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn marker(kind: &str, path: &Path) {
    if MARKERS.load(std::sync::atomic::Ordering::Relaxed) {
        let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        println!("[{kind}:{}]", abs.display());
    }
}

impl Default for PieceArgs {
    fn default() -> Self {
        PieceArgs {
            files: vec![],
            no_orient: false,
            rotate: None,
            flip: false,
            units: "mm".into(),
            chain: "auto".into(),
            fit: None,
            scale: None,
        }
    }
}

pub fn parse_size(s: &str) -> Result<(u32, u32)> {
    Ok(match s {
        "story" | "reel" | "vertical" => (1080, 1920),
        "square" => (2048, 2048),
        "listing" => (2000, 2000),
        "wide" | "landscape" => (1920, 1080),
        _ => {
            let (w, h) = s.split_once('x').context("size must be WxH")?;
            (w.trim().parse()?, h.trim().parse()?)
        }
    })
}

fn parse_triple(s: &str) -> Result<[f32; 3]> {
    let v: Vec<f32> = s.split(',').map(|x| x.trim().parse::<f32>()).collect::<std::result::Result<_, _>>()?;
    if v.len() != 3 {
        bail!("expected x,y,z");
    }
    Ok([v[0], v[1], v[2]])
}

/// "x,y,z" Euler degrees, or axis steps like "z-90,y-90" applied in order.
fn parse_rotation(s: &str) -> Result<[[f32; 3]; 3]> {
    let s = s.trim();
    if s.starts_with(|c: char| matches!(c.to_ascii_lowercase(), 'x' | 'y' | 'z')) {
        let mut m = [[1.0f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        for step in s.split(',') {
            let step = step.trim();
            let (axis, deg) = step.split_at(1);
            let deg: f32 = deg.trim_start_matches('=').parse().with_context(|| format!("bad rotation step {step}"))?;
            let e = match axis.to_ascii_lowercase().as_str() {
                "x" => [deg, 0.0, 0.0],
                "y" => [0.0, deg, 0.0],
                _ => [0.0, 0.0, deg],
            };
            m = mul(rot_matrix(e), m);
        }
        return Ok(m);
    }
    Ok(rot_matrix(parse_triple(s)?))
}

fn rot_matrix(deg: [f32; 3]) -> [[f32; 3]; 3] {
    let (sx, cx) = deg[0].to_radians().sin_cos();
    let (sy, cy) = deg[1].to_radians().sin_cos();
    let (sz, cz) = deg[2].to_radians().sin_cos();
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    mul(rz, mul(ry, rx))
}

fn mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

/// RingDesigner exports stones beside the metal: `ring.stl` +
/// `ring_stones.obj` (+ `ring_stones.json` naming each stone). Pull the
/// companion in automatically when only the metal was given.
pub fn with_companions(files: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = files.to_vec();
    if files.len() == 1 {
        let f = &files[0];
        if let (Some(dir), Some(stem)) = (f.parent(), f.file_stem()) {
            let comp = dir.join(format!("{}_stones.obj", stem.to_string_lossy()));
            if comp.exists() && !out.contains(&comp) {
                out.push(comp);
            }
        }
    }
    out
}

/// Stone rules from RingDesigner's `<stem>_stones.json` sidecars: each
/// record's `object` gets the gem named in its `label`.
pub fn sidecar_stone_rules(files: &[PathBuf]) -> Vec<StoneRule> {
    let mut rules = vec![];
    for f in with_companions(files) {
        let Some(name) = f.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        let json = if name.ends_with("_stones.obj") {
            f.with_extension("json")
        } else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&json) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        for st in v.get("stones").and_then(|s| s.as_array()).into_iter().flatten() {
            let (Some(obj), Some(label)) = (st.get("object").and_then(|x| x.as_str()), st.get("label").and_then(|x| x.as_str())) else {
                continue;
            };
            if let Some(g) = argent_core::shot::gem_from_text(label) {
                rules.push(StoneRule { part: obj.to_string(), gem: g });
            }
        }
    }
    rules
}

pub fn piece_name(files: &[PathBuf]) -> String {
    files.first().and_then(|f| f.file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "piece".into())
}

/// Load, pose and prepare a piece.
pub fn prepare(args: &PieceArgs) -> Result<Prepared> {
    let t0 = Instant::now();
    let unit_scale = match args.units.as_str() {
        "mm" => 1.0,
        "cm" => 10.0,
        "m" => 1000.0,
        "in" | "inch" => 25.4,
        u => bail!("unknown units {u}"),
    };
    let opts = LoadOptions { unit_scale, ..Default::default() };
    let files = with_companions(&args.files);
    for extra in files.iter().skip(args.files.len()) {
        eprintln!("  + stones: {}", extra.display());
    }
    let mesh = if files.len() == 1 {
        argent_mesh::load(&files[0], &opts)
    } else {
        argent_mesh::load_many(&files, &opts)
    }
    .with_context(|| format!("loading {:?}", files))?;
    let mut mesh = mesh;
    if let Some(k) = args.scale {
        mesh.scale(k);
    }
    if let Some(mm) = args.fit {
        let (lo, hi) = mesh.bounds();
        let ext = (0..3).map(|i| hi[i] - lo[i]).fold(0.0f32, f32::max);
        if ext > 0.0 {
            mesh.scale(mm / ext);
        }
    }
    let t_load = t0.elapsed();
    let mut pose = if args.no_orient {
        let (lo, hi) = mesh.bounds();
        Some(Pose { center: [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0], ..Pose::IDENTITY })
    } else {
        None
    };
    if args.rotate.is_some() || args.flip {
        let base = pose.unwrap_or_else(|| argent_mesh::orient::detect(&mesh).pose);
        let mut rot = [[1.0f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        if let Some(r) = &args.rotate {
            rot = parse_rotation(r)?;
        }
        if args.flip {
            rot = mul(rot_matrix([0.0, 180.0, 0.0]), rot);
        }
        pose = Some(Pose { center: base.center, rotation: mul(rot, base.rotation) });
    }
    let chain = match args.chain.as_str() {
        "off" | "no" | "none" => None,
        _ => Some(argent_core::chain::ChainSpec::cable(1.0)),
    };
    let mut p = Prepared::build(mesh, pose, chain, args.chain == "on");
    p.default_stones = sidecar_stone_rules(&args.files);
    if !p.default_stones.is_empty() {
        eprintln!("  stones from sidecar: {}", p.default_stones.iter().map(|r| format!("{}={}", r.part, r.gem)).collect::<Vec<_>>().join(", "));
    }
    eprintln!(
        "piece: {} triangles, radius {:.1} mm, {:?} (load {:.2?}, prepare {:.2?})",
        p.mesh.triangles.len(),
        p.radius,
        p.orientation.kind,
        t_load,
        t0.elapsed() - t_load
    );
    Ok(p)
}

pub fn look(shot: &ShotArgs) -> Result<Look> {
    let studio = Studio::from_name(&shot.studio).with_context(|| format!("unknown studio {}", shot.studio))?;
    if Alloy::from_name(&shot.metal).is_none() {
        bail!("unknown metal {} (try sterling, gold14k, rose14k, bronze, brass, platinum)", shot.metal);
    }
    let mut stones = vec![];
    for s in &shot.stones {
        let (part, gem) = s.split_once('=').unwrap_or(("*", s.as_str()));
        // Opal is the renderer's own diffraction material, not a table entry.
        if !gem.to_ascii_lowercase().contains("opal") && argent_spectra::gems::get(gem).is_none() {
            bail!("unknown gem {gem}; known: opal, black_opal, {}", argent_spectra::gems::names().join(", "));
        }
        stones.push(StoneRule { part: part.into(), gem: gem.into() });
    }
    Ok(Look {
        metal: shot.metal.clone(),
        roughness: shot.roughness,
        patina: shot.patina,
        stones,
        studio,
        exposure_ev: shot.exposure,
        grade: shot.grade.clone(),
        hdri: shot.hdri.as_ref().map(|p| p.display().to_string()),
        hdri_rotate: shot.hdri_rotate,
        hdri_exposure: shot.hdri_exposure,
    })
}

pub fn view(shot: &ShotArgs) -> Result<View> {
    let mut v = View::named(&shot.view).with_context(|| format!("unknown view {} ({})", shot.view, View::NAMES.join(", ")))?;
    if let Some(a) = &shot.angle {
        let (az, el) = a.split_once(',').context("angle must be az,el")?;
        v.azimuth_deg = az.trim().parse()?;
        v.elevation_deg = el.trim().parse()?;
    }
    if let Some(f) = shot.fill {
        v.fill = f;
    }
    Ok(v)
}

pub fn quality(shot: &ShotArgs) -> Result<Quality> {
    let (width, height) = parse_size(&shot.size)?;
    let mut q = Quality { width, height, spp: shot.spp, denoise: !shot.no_denoise, ..Default::default() };
    q.path.max_depth = shot.max_depth;
    Ok(q)
}

fn lens(shot: &ShotArgs) -> Lens {
    let default = if Studio::from_name(&shot.studio) == Some(Studio::White) { 0.0 } else { 16.0 };
    Lens { focal_mm: shot.focal, f_stop: shot.fstop.unwrap_or(default) }
}

pub fn write_image(path: &Path, img: &Rgb8) -> Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("jpg") | Some("jpeg") => argent_post::io::write_jpeg(path, img, 93),
        _ => argent_post::io::write_png(path, img),
    }
}

/// Weight of the piece in a metal, grams (meaningful for closed meshes).
pub fn weight_g(p: &Prepared, alloy: Alloy) -> f64 {
    p.mesh.volume() / 1000.0 * alloy.density() as f64
}

#[derive(Serialize)]
struct InfoOut {
    files: Vec<String>,
    triangles: usize,
    vertices: usize,
    parts: Vec<String>,
    size_mm: [f32; 3],
    area_mm2: f64,
    volume_mm3: f64,
    kind: String,
    bore_diameter_mm: Option<f32>,
    us_ring_size: Option<f32>,
    confidence: f32,
    notes: Vec<String>,
    weights_g: Vec<(String, f64)>,
}

pub fn info(args: &PieceArgs, json: bool) -> Result<()> {
    let p = prepare(args)?;
    let (lo, hi) = p.mesh.bounds();
    let bore = p.orientation.bore_radius.map(|r| 2.0 * r);
    let out = InfoOut {
        files: args.files.iter().map(|f| f.display().to_string()).collect(),
        triangles: p.mesh.triangles.len(),
        vertices: p.mesh.positions.len(),
        parts: p.part_names.clone(),
        size_mm: [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]],
        area_mm2: p.mesh.area(),
        volume_mm3: p.mesh.volume(),
        kind: format!("{:?}", p.orientation.kind),
        bore_diameter_mm: bore,
        // US size from inner circumference: (C - 36.5) / 2.55 (RingDesigner's scale).
        us_ring_size: bore.map(|d| ((std::f32::consts::PI * d - 36.5) / 2.55 * 4.0).round() / 4.0),
        confidence: p.orientation.confidence,
        notes: p.orientation.notes.clone(),
        weights_g: [Alloy::Sterling, Alloy::Gold14kYellow, Alloy::Gold18kYellow, Alloy::Bronze, Alloy::Platinum]
            .iter()
            .map(|&a| (a.display().to_string(), (weight_g(&p, a) * 100.0).round() / 100.0))
            .collect(),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("{}", out.files.join(" + "));
        println!("  {} triangles, {} vertices, parts: {}", out.triangles, out.vertices, out.parts.join(", "));
        println!("  size {:.2} × {:.2} × {:.2} mm, area {:.0} mm², volume {:.1} mm³", out.size_mm[0], out.size_mm[1], out.size_mm[2], out.area_mm2, out.volume_mm3);
        print!("  {} (confidence {:.2})", out.kind, out.confidence);
        if let (Some(d), Some(us)) = (out.bore_diameter_mm, out.us_ring_size) {
            print!(", bore Ø {d:.2} mm ≈ US {us}");
        }
        println!();
        for n in &out.notes {
            println!("    {n}");
        }
        for (a, g) in &out.weights_g {
            println!("  {g:>8.2} g  {a}");
        }
    }
    Ok(())
}

pub fn render(args: &PieceArgs, shot: &ShotArgs, out: &Path, turn: f32) -> Result<()> {
    let p = prepare(args)?;
    let lk = look(shot)?;
    let v = view(shot)?;
    let q = quality(shot)?;
    let mut scene = p.scene(&lk);
    scene.turntable_deg = turn;
    let cam = if turn != 0.0 { p.turntable_camera(&v, &lens(shot), q.width as f32 / q.height as f32) } else { p.camera(&v, &lens(shot), q.width as f32 / q.height as f32) };
    let t = Instant::now();
    let img = render_image(&scene, &cam, &q, 7)?;
    let png = argent_post::develop(&img, &grade(&lk));
    write_image(out, &png)?;
    eprintln!("{} ({}×{}, {} spp) in {:.2?}", out.display(), q.width, q.height, q.spp, t.elapsed());
    marker("IMAGE", out);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn turntable(
    args: &PieceArgs,
    shot: &ShotArgs,
    out: &Path,
    seconds: f32,
    fps: u32,
    hook: Option<&str>,
    audio: Option<PathBuf>,
    qc: bool,
    frames_dir: Option<PathBuf>,
    rig: &str,
) -> Result<()> {
    let p = prepare(args)?;
    let rig = Rig::from_name(rig).with_context(|| format!("unknown rig {rig} (turntable, reveal, sweep)"))?;
    render_reel(&p, shot, out, seconds, fps, hook, audio, frames_dir, rig)?;
    if qc {
        print_qc(out)?;
    }
    marker("VIDEO", out);
    Ok(())
}

pub fn print_qc(out: &Path) -> Result<bool> {
    // Forge's full set (loudness, true peak, frozen frames) once there is a
    // soundtrack; a silent reel would fail loudness until the studio PC adds one.
    let has_track = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a", "-show_entries", "stream=codec_name", "-of", "csv=p=0"])
        .arg(out)
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    let r = if has_track && !is_silent(out) { argent_post::qc::forge_reel_full(out)? } else { argent_post::qc::forge_reel(out)? };
    println!("Forge QC: {}", if r.passed { "PASSED" } else { "FAILED" });
    for c in &r.checks {
        println!("  [{}] {} — {}", if c.ok { "ok" } else { "XX" }, c.check, c.detail);
    }
    Ok(r.passed)
}

/// Camera and light motion for a reel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rig {
    /// One full turn, fixed camera: the safest reel there is.
    Turntable,
    /// Macro on the face with shallow focus, dolly out to the hero framing
    /// while focus racks to the whole piece, then a full turn.
    Reveal,
    /// A hot strip light glides across in front while the piece turns 40°.
    Sweep,
}

impl Rig {
    pub fn from_name(s: &str) -> Option<Rig> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "turntable" | "turn" | "spin" => Rig::Turntable,
            "reveal" | "dolly" | "macro" => Rig::Reveal,
            "sweep" | "glint" | "light-sweep" => Rig::Sweep,
            _ => return None,
        })
    }
}

#[inline]
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Keeps the old name working for callers that only want a turn.
#[allow(clippy::too_many_arguments)]
pub fn render_turntable(
    p: &Prepared,
    shot: &ShotArgs,
    out: &Path,
    seconds: f32,
    fps: u32,
    hook: Option<&str>,
    audio: Option<PathBuf>,
    frames_dir: Option<PathBuf>,
) -> Result<()> {
    render_reel(p, shot, out, seconds, fps, hook, audio, frames_dir, Rig::Turntable)
}

#[allow(clippy::too_many_arguments)]
pub fn render_reel(
    p: &Prepared,
    shot: &ShotArgs,
    out: &Path,
    seconds: f32,
    fps: u32,
    hook: Option<&str>,
    audio: Option<PathBuf>,
    frames_dir: Option<PathBuf>,
    rig: Rig,
) -> Result<()> {
    let lk = look(shot)?;
    let v = view(shot)?;
    let mut q = quality(shot)?;
    if shot.size == "1080x1920" && shot.spp == 128 {
        // Reels are seen moving: fewer samples per frame, the denoiser does the rest.
        q.spp = 32;
    }
    q.fast_denoise = true;
    let n = (seconds * fps as f32).round().max(2.0) as u32;
    let aspect = q.width as f32 / q.height as f32;
    let lens0 = lens(shot);
    let turn_cam = p.turntable_camera(&v, &lens0, aspect);
    let base = p.scene(&lk);
    // Per-frame camera, turntable angle and studio for each rig.
    let frame_setup = |i: u32| -> (argent_core::camera::Camera, f32, Option<argent_core::light::Panel>) {
        let t = i as f32 / n as f32;
        match rig {
            Rig::Turntable => (turn_cam.clone(), 360.0 * t, None),
            Rig::Reveal => {
                const SPLIT: f32 = 0.38;
                if t < SPLIT {
                    let k = ease(t / SPLIT);
                    let mut vv = v;
                    vv.fill = 1.9 + (v.fill - 1.9) * k;
                    vv.azimuth_deg = v.azimuth_deg - 18.0 * (1.0 - k);
                    vv.elevation_deg = v.elevation_deg + 6.0 * (1.0 - k);
                    let l = argent_core::camera::Lens { focal_mm: lens0.focal_mm, f_stop: 2.8 + 30.0 * k * k };
                    let mut cam = p.camera(&vv, &l, aspect);
                    if k > 0.97 {
                        cam.lens_radius = 0.0;
                    }
                    (cam, 0.0, None)
                } else {
                    let k = ease((t - SPLIT) / (1.0 - SPLIT));
                    (p.camera(&v, &lens0, aspect), 360.0 * k, None)
                }
            }
            Rig::Sweep => {
                let k = ease(t);
                let r = p.radius / 15.0;
                let x = (-320.0 + 640.0 * k) * r;
                let strip = argent_core::light::Panel::facing(
                    "sweep",
                    argent_core::geom::V3::new(x, 60.0 * r, 260.0 * r),
                    argent_core::geom::V3::ZERO,
                    60.0 * r,
                    520.0 * r,
                    argent_core::geom::V3::Y,
                    Some(argent_core::studio::emitter(6000.0, 14.0)),
                );
                (turn_cam.clone(), -20.0 + 40.0 * k, Some(strip))
            }
        }
    };
    // Land the track's drop on the rig's main moment, as Forge's studio PC does.
    let main_moment = match rig {
        Rig::Turntable => 0.8,
        Rig::Reveal => 0.38 * seconds,
        Rig::Sweep => 0.5 * seconds,
    };
    let audio_spec = match audio {
        Some(a) => match cut_to_drop(&a, seconds, main_moment) {
            Ok(cut) => argent_post::video::Audio::File(cut),
            Err(e) => {
                eprintln!("  audio: couldn't align the drop ({e:#}); using the track from its start");
                argent_post::video::Audio::File(a)
            }
        },
        None => argent_post::video::Audio::Silent,
    };
    if let Some(dir) = out.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let spec = argent_post::video::VideoSpec { width: q.width, height: q.height, fps, audio: audio_spec, ..Default::default() };
    let mut writer = argent_post::video::VideoWriter::create(out, &spec)?;
    if let Some(d) = &frames_dir {
        std::fs::create_dir_all(d)?;
    }
    let g = grade(&lk);
    let t0 = Instant::now();
    for i in 0..n {
        let mut scene = base.clone();
        let (cam, turn, extra) = frame_setup(i);
        scene.turntable_deg = turn;
        if let Some(panel) = extra {
            scene.panels.push(panel);
            scene.rebuild_lights();
        }
        let img = render_image(&scene, &cam, &q, 11)?;
        #[allow(unused_mut)]
        let mut frame = argent_post::develop(&img, &g);
        if let Some(text) = hook {
            let t = i as f32 / fps as f32;
            crate::jobs::hook_overlay(&mut frame, text, t);
        }
        if let Some(d) = &frames_dir {
            argent_post::io::write_png(&d.join(format!("frame_{i:04}.png")), &frame)?;
        }
        writer.push(&frame)?;
        let el = t0.elapsed().as_secs_f32();
        eprint!("\r  frame {}/{}  {:.1}s/frame  eta {:.0}s   ", i + 1, n, el / (i + 1) as f32, el / (i + 1) as f32 * (n - i - 1) as f32);
    }
    eprintln!();
    writer.finish()?;
    eprintln!("{} ({} frames, {:.1}s) in {:.1?}", out.display(), n, seconds, t0.elapsed());
    Ok(())
}

/// Loudness per 50 ms window (dB RMS) of an audio file, via ffmpeg.
fn loudness_envelope(path: &Path) -> Result<Vec<f32>> {
    let out = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-t", "120", "-i"])
        .arg(path)
        .args([
            "-af",
            "aresample=8000,asetnsamples=n=400,astats=metadata=1:reset=1,ametadata=print:key=lavfi.astats.Overall.RMS_level:file=-",
            "-f",
            "null",
            "-",
        ])
        .output()
        .context("running ffmpeg")?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text
        .lines()
        .filter_map(|l| l.split("RMS_level=").nth(1))
        .map(|v| v.trim().parse::<f32>().unwrap_or(-90.0).max(-90.0))
        .collect())
}

/// Find the drop: the first jump of ≥ 8 dB between the mean of the
/// preceding second and the following 250 ms, at least 2 s in. Seconds.
fn find_drop(env: &[f32]) -> Option<f32> {
    const W: usize = 20; // 1 s of 50 ms windows
    let mut best: Option<(usize, f32)> = None;
    for i in 40.max(W)..env.len().saturating_sub(5) {
        let before = env[i - W..i].iter().sum::<f32>() / W as f32;
        let after = env[i..i + 5].iter().sum::<f32>() / 5.0;
        let jump = after - before;
        if jump >= 8.0 {
            return Some(i as f32 * 0.05);
        }
        if best.is_none_or(|b| jump > b.1) {
            best = Some((i, jump));
        }
    }
    best.filter(|b| b.1 >= 4.0).map(|b| b.0 as f32 * 0.05)
}

/// Cut `track` so its drop lands `at` seconds into a `len`-second reel,
/// with a short fade-out; returns the cut file.
fn cut_to_drop(track: &Path, len: f32, at: f32) -> Result<PathBuf> {
    let env = loudness_envelope(track)?;
    let drop = find_drop(&env).context("no drop found")?;
    let start = (drop - at).max(0.0);
    let out = std::env::temp_dir().join(format!("argent-audio-{}-{:.0}.m4a", std::process::id(), start * 1000.0));
    let fade = 0.8f32.min(len * 0.2);
    let fades = format!("afade=t=in:st=0:d=0.05,afade=t=out:st={:.3}:d={fade:.3}", len - fade);
    // Forge checks loudness (-14 LUFS integrated) and true peak (< -1 dBTP):
    // measure the cut first, then normalize linearly to the target.
    let measure = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-ss", &format!("{start:.3}"), "-t", &format!("{len:.3}"), "-i"])
        .arg(track)
        .args(["-af", &format!("{fades},loudnorm=I=-14:TP=-1.5:LRA=11:print_format=json"), "-f", "null", "-"])
        .output()?;
    let err = String::from_utf8_lossy(&measure.stderr);
    let json = err.rfind('{').and_then(|a| err[a..].find('}').map(|b| &err[a..a + b + 1]));
    let field = |k: &str| -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(json?).ok()?;
        v.get(k)?.as_str().map(str::to_string)
    };
    let norm = match (field("input_i"), field("input_tp"), field("input_lra"), field("input_thresh"), field("target_offset")) {
        (Some(i), Some(tp), Some(lra), Some(th), Some(off)) => format!(
            ",loudnorm=I=-14:TP=-1.5:LRA=11:measured_I={i}:measured_TP={tp}:measured_LRA={lra}:measured_thresh={th}:offset={off}:linear=true"
        ),
        _ => ",loudnorm=I=-14:TP=-1.5:LRA=11".into(),
    };
    let status = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-ss", &format!("{start:.3}"), "-t", &format!("{len:.3}"), "-i"])
        .arg(track)
        .args(["-af", &format!("{fades}{norm},aresample=48000"), "-c:a", "aac", "-b:a", "192k", "-ar", "48000"])
        .arg(&out)
        .status()?;
    if !status.success() {
        bail!("ffmpeg cut failed");
    }
    eprintln!("  audio: drop at {drop:.2}s lands at {at:.2}s of the reel (track from {start:.2}s), normalized to -14 LUFS");
    Ok(out)
}

/// True when the reel's audio track is digital silence (argent's default).
fn is_silent(path: &Path) -> bool {
    let out = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(path)
        .args(["-af", "volumedetect", "-vn", "-f", "null", "-"])
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stderr)
            .lines()
            .find_map(|l| l.split("max_volume:").nth(1))
            .and_then(|v| v.trim().trim_end_matches(" dB").parse::<f32>().ok())
            .map(|db| db < -80.0)
            .unwrap_or(true),
        Err(_) => true,
    }
}

/// Burned-in hook: full opacity for 1.6 s, fading out by 2.2 s.
pub fn hook_overlay(frame: &mut Rgb8, text: &str, t: f32) {
    let alpha = if t < 1.6 { 1.0 } else { (1.0 - (t - 1.6) / 0.6).max(0.0) };
    if alpha <= 0.0 {
        return;
    }
    crate::jobs::overlay_text(frame, text, alpha);
}

fn overlay_text(frame: &mut Rgb8, text: &str, alpha: f32) {
    let style = argent_post::sheet::HookStyle { opacity: alpha, ..Default::default() };
    argent_post::sheet::overlay_hook(frame, text, &style);
}

#[derive(Serialize)]
struct SheetItem {
    metal: String,
    view: String,
    file: String,
}

#[derive(Serialize)]
pub struct SheetManifest {
    piece: String,
    weights_g: Vec<(String, f64)>,
    items: Vec<SheetItem>,
    contact_sheet: String,
}

pub fn sheet(args: &PieceArgs, shot: &ShotArgs, out: &Path, metals: &[String], views: &[String], title: Option<String>) -> Result<SheetManifest> {
    let p = prepare(args)?;
    let m = sheet_prepared(&p, &piece_name(&args.files), shot, out, metals, views, title)?;
    marker("IMAGE", &out.join(&m.contact_sheet));
    Ok(m)
}

pub fn sheet_prepared(p: &Prepared, name: &str, shot: &ShotArgs, out: &Path, metals: &[String], views: &[String], title: Option<String>) -> Result<SheetManifest> {
    std::fs::create_dir_all(out)?;
    let mut shot = shot.clone();
    if shot.size == "1080x1920" {
        shot.size = "1600x1600".into();
    }
    // Listing images are focus-stacked in real studios: keep everything sharp.
    if shot.fstop.is_none() {
        shot.fstop = Some(0.0);
    }
    let q = quality(&shot)?;
    let mut tiles = vec![];
    let mut items = vec![];
    let mut weights = vec![];
    for m in metals {
        let alloy = Alloy::from_name(m).with_context(|| format!("unknown metal {m}"))?;
        weights.push((alloy.display().to_string(), (weight_g(p, alloy) * 100.0).round() / 100.0));
        let mut s = shot.clone();
        s.metal = m.clone();
        let lk = look(&s)?;
        let scene = p.scene(&lk);
        for vname in views {
            s.view = vname.clone();
            let v = view(&s)?;
            let cam = p.camera(&v, &lens(&s), q.width as f32 / q.height as f32);
            let t = Instant::now();
            let img = render_image(&scene, &cam, &q, 3)?;
            let png = argent_post::develop(&img, &grade(&lk));
            let file = format!("{name}-{}-{vname}.png", alloy.key());
            write_image(&out.join(&file), &png)?;
            eprintln!("  {file} in {:.1?}", t.elapsed());
            tiles.push(argent_post::sheet::Tile { image: png, caption: format!("{} · {}", alloy.display(), vname) });
            items.push(SheetItem { metal: alloy.key().into(), view: vname.clone(), file });
        }
    }
    let style = argent_post::sheet::SheetStyle {
        columns: views.len().max(1) as u32,
        tile_width: 480,
        background: [14, 14, 15],
        text: [232, 226, 214],
        title: Some(title.unwrap_or_else(|| name.to_string())),
        subtitle: Some(
            weights.iter().map(|(a, g)| format!("{a} ≈ {g:.1} g")).collect::<Vec<_>>().join("  ·  "),
        ),
    };
    let sheet = argent_post::sheet::contact_sheet(&tiles, &style);
    let sheet_file = format!("{name}-sheet.png");
    write_image(&out.join(&sheet_file), &sheet)?;
    let manifest = SheetManifest { piece: name.into(), weights_g: weights, items, contact_sheet: sheet_file };
    std::fs::write(out.join(format!("{name}.json")), serde_json::to_string_pretty(&manifest)?)?;
    Ok(manifest)
}

/// Captioned contact sheet from existing images.
pub fn montage(images: &[PathBuf], out: &Path, columns: u32, tile: u32, title: Option<String>, subtitle: Option<String>) -> Result<()> {
    let mut tiles = vec![];
    for p in images {
        let img = argent_post::io::read_rgb8(p).with_context(|| format!("reading {}", p.display()))?;
        let caption = p
            .file_stem()
            .map(|s| s.to_string_lossy().replace(['-', '_'], " "))
            .unwrap_or_default();
        tiles.push(argent_post::sheet::Tile { image: img, caption });
    }
    let style = argent_post::sheet::SheetStyle {
        columns: columns.max(1),
        tile_width: tile,
        background: [14, 14, 15],
        text: [232, 226, 214],
        title,
        subtitle,
    };
    let sheet = argent_post::sheet::contact_sheet(&tiles, &style);
    write_image(out, &sheet)?;
    eprintln!("{} ({} tiles, {}×{})", out.display(), tiles.len(), sheet.width, sheet.height);
    marker("IMAGE", out);
    Ok(())
}

pub fn find_meshes(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if matches!(
                p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
                Some("stl" | "obj" | "ply" | "3mf")
            ) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

pub fn batch(dir: &Path, out: &Path, shot: &ShotArgs, metals: &[String], views: &[String], turntables: bool, seconds: f32) -> Result<()> {
    let files = find_meshes(dir);
    if files.is_empty() {
        bail!("no meshes under {}", dir.display());
    }
    std::fs::create_dir_all(out)?;
    let mut rows = vec![];
    for f in &files {
        let name = piece_name(std::slice::from_ref(f));
        eprintln!("== {name}");
        let args = PieceArgs { files: vec![f.clone()], no_orient: false, rotate: None, flip: false, units: "mm".into(), chain: "auto".into(), fit: None, scale: None };
        let p = match prepare(&args) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("  skipped: {e:#}");
                continue;
            }
        };
        let pdir = out.join(&name);
        let m = sheet_prepared(&p, &name, shot, &pdir, metals, views, None)?;
        let mut reel = None;
        if turntables {
            let file = format!("{name}-turntable.mp4");
            let mut s = shot.clone();
            s.size = "1080x1920".into();
            s.spp = 128;
            render_turntable(&p, &s, &pdir.join(&file), seconds, 30, None, None, None)?;
            reel = Some(file);
        }
        rows.push((name, m, reel));
    }
    let mut html = String::from(
        "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'><title>argent catalog</title>\
         <style>body{background:#0e0e0f;color:#e8e2d6;font-family:Georgia,serif;margin:0;padding:16px}h2{font-weight:normal;margin:28px 0 6px}\
         .row{display:flex;gap:10px;overflow-x:auto}.row img,.row video{height:360px;border-radius:6px}small{color:#a99}</style><h1>Catalog</h1>",
    );
    for (name, m, reel) in &rows {
        html.push_str(&format!("<h2>{name}</h2><small>{}</small><div class=row>", m.weights_g.iter().map(|(a, g)| format!("{a} ≈ {g:.1} g")).collect::<Vec<_>>().join(" · ")));
        if let Some(r) = reel {
            html.push_str(&format!("<video src='{name}/{r}' autoplay muted loop playsinline></video>"));
        }
        for it in &m.items {
            html.push_str(&format!("<img loading=lazy src='{name}/{}' title='{} {}'>", it.file, it.metal, it.view));
        }
        html.push_str("</div>");
    }
    std::fs::write(out.join("index.html"), html)?;
    eprintln!("catalog: {} pieces → {}", rows.len(), out.join("index.html").display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_is_the_first_big_jump() {
        let mut env = vec![-18.0f32; 400];
        for v in env.iter_mut().skip(264) {
            *v = -6.0;
        }
        let d = find_drop(&env).unwrap();
        assert!((d - 13.2).abs() < 0.11, "{d}");
    }

    #[test]
    fn real_track_drop() {
        let p = Path::new("/home/shadowbroker/argent/library/audio/ES_Lone_Wolf-Toby_Tranter.mp3");
        if !p.exists() {
            return;
        }
        let env = loudness_envelope(p).unwrap();
        let d = find_drop(&env).unwrap();
        assert!((d - 13.22).abs() < 0.15, "{d}");
    }
}
