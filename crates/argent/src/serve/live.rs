//! The live view.
//!
//! One render worker thread refines the current set progressively (rayon
//! inside each pass). Any change to the set bumps a generation counter and
//! the worker restarts accumulation: first a quick half-resolution pass so
//! orbiting stays responsive, then full preview resolution in passes sized
//! to ~70 ms.
//!
//! Frames are denoised (OIDN at Fast quality), developed and JPEG-encoded
//! inline between passes, never alongside one: both want every core, and
//! overlapping them made each several times slower. Frames go into a
//! `watch` channel (latest wins, so a slow client skips frames instead of
//! queueing them). Refinement frames space out as samples accumulate, since
//! the picture changes less and less. The converged frame goes out at once
//! with the fast denoiser; a polish thread then re-denoises it at full
//! quality while the worker idles and swaps it in, unless the set changed
//! meanwhile.
//!
//! Exposure, AgX look and the denoise toggle only re-develop: the samples
//! already accumulated are kept.

use super::params::{Params, Patch, SceneKey, Stone};
use super::pieces::Piece;
use anyhow::Result;
use argent_core::camera::Camera;
use argent_core::integrator::PathSettings;
use argent_core::render::{render_pass, Film};
use argent_core::scene::Scene;
use argent_post::{DenoiseBackend, DenoiseInput, DenoiseOptions, DenoiseQuality, Grade, Image, Rgb8};
use axum::body::Bytes;
use parking_lot::{Condvar, Mutex};
use serde::Serialize;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

const SEED: u32 = 1;
/// While the set is changing (orbit, sliders): at most ~12 fps.
const MOTION_INTERVAL: Duration = Duration::from_millis(83);
/// Refinement frames: at most ~6 fps …
const REFINE_INTERVAL: Duration = Duration::from_millis(166);
/// … slowing as √(spp / this): 166 ms up to 32 spp, 333 ms at 128, 666 ms at 512.
const REFINE_SPP: f64 = 32.0;
const REFINE_MAX: Duration = Duration::from_millis(1500);
/// A generation younger than this is "in motion": 1-spp passes, quick frames.
const MOTION_WINDOW: Duration = Duration::from_millis(350);
/// With no visible browser holding the stream, keep rendering this long
/// after the last frame fetch or API call, then idle (24/7 on a shared box).
const KEEPALIVE: Duration = Duration::from_secs(20);
/// Refinement also stops once successive denoised frames differ by less
/// than this (mean |ΔY| / mean Y), from this many samples on.
const SETTLE_DELTA: f64 = 0.005;
const SETTLE_MIN_SPP: u32 = 64;
/// Wall time to aim for per refinement pass (bounds the reaction to a change).
const PASS_TARGET_MS: f64 = 70.0;
/// In-progress frames use OIDN at Fast quality without aux prefiltering
/// (measured on this 32-core box: 16 ms at 270×480, 41 ms at 540×960; the
/// à-trous fallback 32 / 90 ms; OIDN High with prefiltering 125 / 390 ms,
/// kept for the converged frame). Frames are spaced so that denoising takes
/// at most this share of wall time.
const DENOISE_SHARE: f64 = 0.35;
/// Above this per-frame cost in-progress frames go out raw.
const DENOISE_MAX_MS: f64 = 400.0;
/// The full-quality denoise of a converged frame waits this long first:
/// a tweak right after convergence then never competes with it for cores.
const POLISH_DELAY: Duration = Duration::from_millis(400);
pub const JPEG_QUALITY: u8 = 85;

/// One published preview frame, packed once for every client:
/// `[u32 LE header length][JSON header][JPEG]` (see API.md, "Stream").
pub struct Frame {
    pub seq: u64,
    pub generation: u64,
    pub spp: u32,
    pub converged: bool,
    pub header: String,
    pub packed: Bytes,
    jpeg_at: usize,
}

impl Frame {
    pub fn jpeg(&self) -> Bytes {
        self.packed.slice(self.jpeg_at..)
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Stats {
    pub generation: u64,
    pub spp: u32,
    pub max_spp: u32,
    pub width: u32,
    pub height: u32,
    pub rendering: bool,
    pub paused: bool,
    pub converged: bool,
    pub viewers: usize,
    pub frames: u64,
    /// Published frames per second (recent).
    pub fps: f32,
    /// Full-resolution preview cost.
    pub ms_per_spp: f32,
    pub msamples_per_s: f32,
    /// Change → first frame of the new generation published.
    pub first_frame_ms: f32,
    /// Change → converged, last generation that got there.
    pub converge_s: f32,
    /// In-progress denoiser (`oidn-fast` or `atrous`) and its last cost.
    pub denoiser: &'static str,
    pub denoise_ms: f32,
    /// Full-quality denoise of the converged frame.
    pub polish_ms: f32,
    pub develop_ms: f32,
    pub encode_ms: f32,
    pub jpeg_kb: f32,
    pub error: Option<String>,
}

struct Shared {
    piece: Option<Arc<Piece>>,
    params: Params,
    /// Bumped by anything that changes the image (restart).
    generation: u64,
    /// Bumped by development-only changes (re-develop, keep samples).
    dgen: u64,
    changed_at: Instant,
    paused: bool,
    in_pass: bool,
    viewers: usize,
    poke: Instant,
}

/// A linear (denoised or raw) image as published, kept for re-developing.
#[derive(Clone)]
struct Linear {
    generation: u64,
    spp: u32,
    max_spp: u32,
    half: bool,
    converged: bool,
    denoiser: &'static str,
    img: Arc<Image>,
}

struct PolishJob {
    generation: u64,
    spp: u32,
    max_spp: u32,
    color: Image,
    albedo: Image,
    normal: Image,
    changed_at: Instant,
}

pub struct Live {
    st: Mutex<Shared>,
    cv: Condvar,
    polish: Mutex<Option<PolishJob>>,
    polish_cv: Condvar,
    /// Orders frames from the worker and the polish thread.
    publish_lock: Mutex<()>,
    last: Mutex<Option<Linear>>,
    seq: AtomicU64,
    last_gen: AtomicU64,
    last_emit: Mutex<Option<Instant>>,
    shutdown: AtomicBool,
    pub frames: watch::Sender<Option<Arc<Frame>>>,
    stats: Mutex<Stats>,
    /// In-progress denoise cost, ms per megapixel (EMA, seeded by calibration).
    dn_ms_per_mpx: Mutex<f64>,
    dn_retry: Mutex<Instant>,
}

/// What the worker built for the current generation.
struct Built {
    piece: Arc<Piece>,
    scene_key: SceneKey,
    scene: Scene,
    cam: Camera,
}

fn build(prev: Option<Built>, piece: &Arc<Piece>, p: &Params) -> Built {
    let key = p.scene_key();
    let mut scene = match prev {
        Some(b) if Arc::ptr_eq(&b.piece, piece) && b.scene_key == key => b.scene,
        _ => piece.prepared.scene(&p.look()),
    };
    let (w, h) = p.preview_size();
    let (cam, turn) = super::params::shot_camera(&piece.prepared, p, w as f32 / h as f32);
    scene.turntable_deg = turn;
    Built { piece: piece.clone(), scene_key: key, scene, cam }
}

fn panic_text(e: &(dyn std::any::Any + Send)) -> String {
    e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into())
}

pub fn encode_jpeg(img: &Rgb8, quality: u8) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(img.data.len() / 6);
    let mut enc = jpeg_encoder::Encoder::new(&mut buf, quality);
    enc.set_sampling_factor(jpeg_encoder::SamplingFactor::F_2_2);
    enc.encode(&img.data, img.width as u16, img.height as u16, jpeg_encoder::ColorType::Rgb)?;
    Ok(buf)
}

/// Mean absolute luminance difference relative to mean luminance, on a
/// strided subsample (cheap: ~130k pixels at 540×960).
fn relative_change(a: &Image, b: &Image) -> f64 {
    let lum = |p: &[f32; 3]| 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
    let (mut diff, mut base) = (0.0f64, 0.0f64);
    for (pa, pb) in a.pixels.iter().zip(&b.pixels).step_by(4) {
        let (la, lb) = (lum(pa), lum(pb));
        diff += (la - lb).abs();
        base += lb.abs();
    }
    if base <= 1e-9 { 0.0 } else { diff / base }
}

/// The denoiser for in-progress frames.
fn progress_denoiser() -> (DenoiseBackend, DenoiseOptions, &'static str) {
    if argent_post::oidn_available() {
        (DenoiseBackend::Oidn, DenoiseOptions { prefilter_aux: false, quality: DenoiseQuality::Fast, ..Default::default() }, "oidn-fast")
    } else {
        (DenoiseBackend::Atrous, DenoiseOptions::default(), "atrous")
    }
}

/// Clears `in_pass` (and wakes a waiting final job) when a pass ends.
struct PassGuard<'a>(&'a Live);

impl Drop for PassGuard<'_> {
    fn drop(&mut self) {
        self.0.st.lock().in_pass = false;
        self.0.cv.notify_all();
    }
}

impl Live {
    pub fn start(params: Params) -> Arc<Live> {
        let (frames, _) = watch::channel(None);
        let live = Arc::new(Live {
            st: Mutex::new(Shared {
                piece: None,
                params,
                generation: 1,
                dgen: 1,
                changed_at: Instant::now(),
                paused: false,
                in_pass: false,
                viewers: 0,
                poke: Instant::now(),
            }),
            cv: Condvar::new(),
            polish: Mutex::new(None),
            polish_cv: Condvar::new(),
            publish_lock: Mutex::new(()),
            last: Mutex::new(None),
            seq: AtomicU64::new(0),
            last_gen: AtomicU64::new(0),
            last_emit: Mutex::new(None),
            shutdown: AtomicBool::new(false),
            frames,
            stats: Mutex::new(Stats::default()),
            dn_ms_per_mpx: Mutex::new(0.0),
            dn_retry: Mutex::new(Instant::now()),
        });
        let w = live.clone();
        std::thread::Builder::new().name("argent-live".into()).spawn(move || w.worker()).expect("spawn live worker");
        let p = live.clone();
        std::thread::Builder::new().name("argent-polish".into()).spawn(move || p.polisher()).expect("spawn polisher");
        live
    }

    pub fn params(&self) -> Params {
        self.st.lock().params.clone()
    }

    pub fn piece(&self) -> Option<Arc<Piece>> {
        self.st.lock().piece.clone()
    }

    pub fn generation(&self) -> u64 {
        self.st.lock().generation
    }

    fn bump(s: &mut Shared, old: &Params, force: bool) {
        if force || old.scene_key() != s.params.scene_key() || old.camera_key() != s.params.camera_key() {
            s.generation += 1;
            s.changed_at = Instant::now();
        }
        if old.develop_key() != s.params.develop_key() {
            s.dgen += 1;
            if !force {
                s.changed_at = Instant::now();
            }
        }
        s.poke = Instant::now();
    }

    /// Apply a partial update to the set.
    pub fn update(&self, patch: &Patch) -> Result<Params> {
        let mut s = self.st.lock();
        let old = s.params.clone();
        s.params.apply(patch)?;
        Self::bump(&mut s, &old, false);
        let p = s.params.clone();
        drop(s);
        self.cv.notify_all();
        Ok(p)
    }

    /// Put a piece on set (`None` while the next one prepares). `stones`
    /// replaces the user's stone rules (they are per piece).
    pub fn set_piece(&self, piece: Option<Arc<Piece>>, stones: Option<Vec<Stone>>) {
        let mut s = self.st.lock();
        let old = s.params.clone();
        s.piece = piece;
        if let Some(st) = stones {
            s.params.stones = st;
        }
        Self::bump(&mut s, &old, true);
        drop(s);
        self.cv.notify_all();
    }

    /// Keep rendering for a while without a browser (agents polling frames).
    pub fn poke(&self) {
        self.st.lock().poke = Instant::now();
        self.cv.notify_all();
    }

    pub fn viewer(&self, delta: i32) {
        let mut s = self.st.lock();
        s.viewers = (s.viewers as i64 + delta as i64).max(0) as usize;
        s.poke = Instant::now();
        drop(s);
        self.cv.notify_all();
    }

    /// Stop the preview and wait for the pass in flight (a final job wants
    /// every core).
    pub fn pause(&self) {
        let mut s = self.st.lock();
        s.paused = true;
        while s.in_pass {
            self.cv.wait(&mut s);
        }
        drop(s);
        self.stats.lock().paused = true;
    }

    pub fn resume(&self) {
        self.st.lock().paused = false;
        self.stats.lock().paused = false;
        self.cv.notify_all();
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.cv.notify_all();
        self.polish_cv.notify_all();
    }

    pub fn stats(&self) -> Stats {
        let mut st = self.stats.lock().clone();
        let s = self.st.lock();
        st.viewers = s.viewers;
        st.paused = s.paused;
        st
    }

    /// Time the in-progress denoiser on a preview-sized frame (the second
    /// run: the first pays for device start-up and page faults).
    fn calibrate(&self) {
        let (w, h) = (270u32, 480u32);
        let mut color = Image::new(w, h);
        let mut albedo = Image::new(w, h);
        let mut normal = Image::new(w, h);
        let mut x = 0x9e37_79b9u32;
        for i in 0..(w * h) as usize {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let v = (x >> 8) as f32 / (1u32 << 24) as f32;
            color.pixels[i] = [v, v * 0.9, v * 0.8];
            albedo.pixels[i] = [0.6, 0.6, 0.6];
            normal.pixels[i] = [0.0, 0.0, 1.0];
        }
        let (backend, opts, name) = progress_denoiser();
        let mut ms = 0.0;
        for _ in 0..2 {
            let t = Instant::now();
            let input = DenoiseInput { color: &color, albedo: Some(&albedo), normal: Some(&normal) };
            if argent_post::denoise_with(input, backend, &opts).is_err() {
                return;
            }
            ms = t.elapsed().as_secs_f64() * 1e3;
        }
        *self.dn_ms_per_mpx.lock() = ms / (w as f64 * h as f64 / 1e6);
        let mut st = self.stats.lock();
        st.denoiser = name;
        st.denoise_ms = ms as f32;
    }

    /// Estimated in-progress denoise cost for a frame of `pixels`.
    fn denoise_ms(&self, pixels: u32) -> f64 {
        *self.dn_ms_per_mpx.lock() * pixels as f64 / 1e6
    }

    /// `base`, stretched so denoising stays within its share of wall time.
    fn paced(&self, base: Duration, pixels: u32, p: &Params) -> Duration {
        let ms = self.denoise_ms(pixels);
        if !p.denoise || ms > DENOISE_MAX_MS {
            return base;
        }
        base.max(Duration::from_secs_f64(ms / DENOISE_SHARE / 1e3))
    }

    fn refine_interval(&self, spp: u32, pixels: u32, p: &Params) -> Duration {
        let base = REFINE_INTERVAL.mul_f64((spp as f64 / REFINE_SPP).sqrt().max(1.0)).min(REFINE_MAX);
        self.paced(base, pixels, p)
    }

    /// Denoise (fast), develop, encode and publish a film. Worker only.
    /// Returns whether the frame went out as final (converged, or settled:
    /// barely different from the previous frame).
    fn publish(&self, film: &Film, half: bool, converged: bool, p: &Params, generation: u64, changed_at: Instant) -> bool {
        let mut converged = converged;
        let pixels = film.width * film.height;
        // Too slow to denoise every frame? Still try now and then, so one
        // slow outlier can't switch it off for good.
        let retry = {
            let mut last = self.dn_retry.lock();
            let due = last.elapsed() > Duration::from_secs(5);
            if due {
                *last = Instant::now();
            }
            due
        };
        let color = film.color();
        let mut denoiser = "none";
        let mut img = None;
        if p.denoise && (converged || retry || self.denoise_ms(pixels) <= DENOISE_MAX_MS) {
            let (albedo, normal) = (film.albedo(), film.normal());
            let (backend, opts, name) = progress_denoiser();
            let t = Instant::now();
            match argent_post::denoise_with(DenoiseInput { color: &color, albedo: Some(&albedo), normal: Some(&normal) }, backend, &opts) {
                Ok(d) => {
                    let ms = t.elapsed().as_secs_f64() * 1e3;
                    {
                        let mut rate = self.dn_ms_per_mpx.lock();
                        let r = ms / (pixels as f64 / 1e6).max(1e-3);
                        *rate = if *rate == 0.0 { r } else { 0.7 * *rate + 0.3 * r };
                    }
                    let mut st = self.stats.lock();
                    st.denoiser = name;
                    st.denoise_ms = ms as f32;
                    drop(st);
                    // Settled? Compare with the previous full-resolution frame of this set.
                    if !converged && !half && film.samples >= SETTLE_MIN_SPP {
                        let prev = self.last.lock().clone();
                        if let Some(l) = prev
                            && l.generation == generation
                            && !l.half
                            && l.denoiser == name
                            && l.img.width == d.width
                            && l.img.height == d.height
                            && relative_change(&l.img, &d) < SETTLE_DELTA
                        {
                            converged = true;
                        }
                    }
                    img = Some(d);
                    denoiser = name;
                    if converged && argent_post::oidn_available() {
                        *self.polish.lock() = Some(PolishJob {
                            generation,
                            spp: film.samples,
                            max_spp: p.max_spp,
                            color: color.clone(),
                            albedo,
                            normal,
                            changed_at,
                        });
                        self.polish_cv.notify_one();
                    }
                }
                Err(e) => eprintln!("argent serve: preview denoise failed: {e:#}"),
            }
        }
        let lin = Linear {
            generation,
            spp: film.samples,
            max_spp: p.max_spp,
            half,
            converged,
            denoiser,
            img: Arc::new(img.unwrap_or(color)),
        };
        self.emit(lin, &argent_core::shot::grade(&p.look()), changed_at, false);
        converged
    }

    /// Develop, encode and publish a linear image; with `only_if_current`
    /// (the polish thread) nothing goes out once the set has moved on.
    fn emit(&self, lin: Linear, grade: &Grade, changed_at: Instant, only_if_current: bool) -> bool {
        let t1 = Instant::now();
        let rgb = argent_post::develop(&lin.img, grade);
        let dev_ms = t1.elapsed().as_secs_f64() * 1e3;
        let t2 = Instant::now();
        let jpeg = match encode_jpeg(&rgb, JPEG_QUALITY) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("argent serve: preview JPEG failed: {e:#}");
                return false;
            }
        };
        let enc_ms = t2.elapsed().as_secs_f64() * 1e3;
        let _order = self.publish_lock.lock();
        if only_if_current {
            let s = self.st.lock();
            if s.generation != lin.generation || !s.params.denoise {
                return false;
            }
        }
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let since_change = changed_at.elapsed().as_secs_f64() * 1e3;
        let header = serde_json::json!({
            "seq": seq,
            "gen": lin.generation,
            "spp": lin.spp,
            "max_spp": lin.max_spp,
            "w": rgb.width,
            "h": rgb.height,
            "half": lin.half,
            "denoised": lin.denoiser != "none",
            "denoiser": lin.denoiser,
            "converged": lin.converged,
            "ms": since_change.round(),
            "bytes": jpeg.len(),
        })
        .to_string();
        let kb = jpeg.len() as f32 / 1024.0;
        let mut packed = Vec::with_capacity(4 + header.len() + jpeg.len());
        packed.extend_from_slice(&(header.len() as u32).to_le_bytes());
        packed.extend_from_slice(header.as_bytes());
        let jpeg_at = packed.len();
        packed.extend_from_slice(&jpeg);
        let frame = Frame { seq, generation: lin.generation, spp: lin.spp, converged: lin.converged, header, packed: Bytes::from(packed), jpeg_at };
        self.frames.send_replace(Some(Arc::new(frame)));
        let first = self.last_gen.swap(lin.generation, Ordering::SeqCst) != lin.generation;
        *self.last.lock() = Some(lin);
        let now = Instant::now();
        let prev = self.last_emit.lock().replace(now);
        let mut st = self.stats.lock();
        st.frames = seq;
        if let Some(prev) = prev {
            let dt = now.duration_since(prev).as_secs_f32();
            if dt < 2.0 {
                let f = 1.0 / dt.max(1e-3);
                st.fps = if st.fps == 0.0 { f } else { 0.8 * st.fps + 0.2 * f };
            }
        }
        if first {
            st.first_frame_ms = since_change as f32;
        }
        st.develop_ms = dev_ms as f32;
        st.encode_ms = enc_ms as f32;
        st.jpeg_kb = kb;
        true
    }

    /// Full-quality denoise of converged frames, off the worker thread.
    fn polisher(self: Arc<Self>) {
        loop {
            let job = {
                let mut m = self.polish.lock();
                loop {
                    if self.shutdown.load(Ordering::SeqCst) {
                        return;
                    }
                    if let Some(j) = m.take() {
                        break j;
                    }
                    self.polish_cv.wait_for(&mut m, Duration::from_millis(500));
                }
            };
            // Give a quick follow-up change the chance to make this moot.
            let settle = Instant::now() + POLISH_DELAY;
            let mut stale = false;
            while Instant::now() < settle {
                if self.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                if self.st.lock().generation != job.generation || self.polish.lock().is_some() {
                    stale = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if stale || self.st.lock().generation != job.generation {
                continue;
            }
            let t = Instant::now();
            let res = catch_unwind(AssertUnwindSafe(|| {
                let input = DenoiseInput { color: &job.color, albedo: Some(&job.albedo), normal: Some(&job.normal) };
                argent_post::denoise_with(input, DenoiseBackend::Auto, &DenoiseOptions::default())
            }));
            let img = match res {
                Ok(Ok(img)) => img,
                Ok(Err(e)) => {
                    eprintln!("argent serve: polishing the converged frame failed: {e:#}");
                    continue;
                }
                Err(e) => {
                    eprintln!("argent serve: polishing the converged frame panicked: {}", panic_text(&*e));
                    continue;
                }
            };
            self.stats.lock().polish_ms = t.elapsed().as_secs_f32() * 1e3;
            let grade = argent_core::shot::grade(&self.st.lock().params.look());
            let lin = Linear {
                generation: job.generation,
                spp: job.spp,
                max_spp: job.max_spp,
                half: false,
                converged: true,
                denoiser: "oidn",
                img: Arc::new(img),
            };
            self.emit(lin, &grade, job.changed_at, true);
        }
    }

    fn worker(self: Arc<Self>) {
        self.calibrate();
        let ps = PathSettings::default();
        let mut built: Option<Built> = None;
        let mut film = Film::new(2, 2);
        let mut half = Film::new(2, 2);
        let (mut cur_gen, mut cur_dgen) = (0u64, 0u64);
        let mut half_done = false;
        let mut final_done = true;
        let mut last_post: Option<Instant> = None;
        let mut ms_per_spp = 0.0f64;
        let mut gen_started = Instant::now();
        let mut changed_at = Instant::now();
        // Generation of the last frame this worker published.
        let mut published_gen = 0u64;
        // The current change came hard on the heels of the previous one.
        let mut burst = false;
        // Refinement stopped early (frames settled) for this sample target.
        let mut settled_for: Option<u32> = None;
        let due = |last: Option<Instant>, every: Duration| last.is_none_or(|t| t.elapsed() >= every);
        loop {
            // Wait for something to do.
            let (piece, params, generation, dgen, chg) = {
                let mut s = self.st.lock();
                loop {
                    if self.shutdown.load(Ordering::SeqCst) {
                        return;
                    }
                    let active = s.viewers > 0 || s.poke.elapsed() < KEEPALIVE;
                    let more = film.samples < s.params.max_spp && settled_for != Some(s.params.max_spp);
                    let work = s.generation != cur_gen || s.dgen != cur_dgen || more || !final_done;
                    if !s.paused && active && s.piece.is_some() && work {
                        break;
                    }
                    self.stats.lock().rendering = false;
                    self.cv.wait_for(&mut s, Duration::from_millis(500));
                }
                s.in_pass = true;
                (s.piece.clone().expect("piece"), s.params.clone(), s.generation, s.dgen, s.changed_at)
            };
            let _guard = PassGuard(&self);
            self.stats.lock().rendering = true;

            // A change: rebuild what it touched and restart accumulation.
            if generation != cur_gen {
                let prev = built.take();
                match catch_unwind(AssertUnwindSafe(|| build(prev, &piece, &params))) {
                    Ok(b) => built = Some(b),
                    Err(e) => {
                        let msg = format!("live view: building the scene failed: {}", panic_text(&*e));
                        eprintln!("argent serve: {msg}");
                        self.stats.lock().error = Some(msg);
                        (cur_gen, cur_dgen, final_done) = (generation, dgen, true);
                        film = Film::new(2, 2);
                        film.samples = u32::MAX;
                        continue;
                    }
                }
                let (w, h) = params.preview_size();
                if film.width != w || film.height != h {
                    film = Film::new(w, h);
                    half = Film::new((w / 2).max(2), (h / 2).max(2));
                } else {
                    film.clear();
                    half.clear();
                }
                cur_gen = generation;
                cur_dgen = dgen;
                half_done = false;
                final_done = false;
                settled_for = None;
                burst = gen_started.elapsed() < MOTION_WINDOW;
                gen_started = Instant::now();
                changed_at = chg;
                let mut st = self.stats.lock();
                st.error = None;
                st.generation = generation;
                st.max_spp = params.max_spp;
                st.width = w;
                st.height = h;
                st.spp = 0;
                st.converged = false;
            }
            let Some(b) = built.as_ref() else { continue };

            // Development-only change: re-develop what was published.
            if dgen != cur_dgen {
                cur_dgen = dgen;
                let last = self.last.lock().clone();
                match last {
                    Some(l) if l.generation == generation && (l.denoiser != "none") == params.denoise => {
                        self.emit(l, &argent_core::shot::grade(&params.look()), chg, false);
                        last_post = Some(Instant::now());
                        continue;
                    }
                    _ if film.samples > 0 || half.samples > 0 => {
                        let (src, is_half) = if film.samples > 0 { (&film, false) } else { (&half, true) };
                        let conv = film.samples >= params.max_spp;
                        self.publish(src, is_half, conv, &params, generation, chg);
                        last_post = Some(Instant::now());
                        continue;
                    }
                    _ => {}
                }
            }

            let r = catch_unwind(AssertUnwindSafe(|| {
                let motion = gen_started.elapsed() < MOTION_WINDOW;
                if !half_done {
                    // First look at a new set: half resolution, 2 spp.
                    render_pass(&b.scene, &b.cam, &mut half, &ps, 2, SEED);
                    half_done = true;
                    // A lone change goes out at once; a burst (orbiting) is paced.
                    let gap = if burst { MOTION_INTERVAL } else { MOTION_INTERVAL / 4 };
                    if due(last_post, self.paced(gap, half.width * half.height, &params)) {
                        self.publish(&half, true, false, &params, generation, changed_at);
                        last_post = Some(Instant::now());
                        published_gen = generation;
                    }
                    return;
                }
                let max = params.max_spp;
                if film.samples < max && settled_for != Some(max) {
                    let count = if motion || ms_per_spp == 0.0 {
                        1
                    } else {
                        (PASS_TARGET_MS / ms_per_spp).floor().clamp(1.0, 32.0) as u32
                    }
                    .min(max - film.samples);
                    let t = Instant::now();
                    render_pass(&b.scene, &b.cam, &mut film, &ps, count, SEED);
                    let ms = t.elapsed().as_secs_f64() * 1e3 / count as f64;
                    ms_per_spp = if ms_per_spp == 0.0 { ms } else { 0.75 * ms_per_spp + 0.25 * ms };
                    {
                        let mut st = self.stats.lock();
                        st.spp = film.samples;
                        st.ms_per_spp = ms_per_spp as f32;
                        st.msamples_per_s = (film.width as f64 * film.height as f64 / ms_per_spp / 1e3) as f32;
                    }
                    let converged = film.samples >= max;
                    let pixels = film.width * film.height;
                    let interval =
                        if motion { self.paced(MOTION_INTERVAL, pixels, &params) } else { self.refine_interval(film.samples, pixels, &params) };
                    // While in motion the half-res look stands in until full res has 4 spp.
                    if !converged && motion && film.samples < 4 {
                        if published_gen != generation && due(last_post, self.paced(MOTION_INTERVAL, half.width * half.height, &params)) {
                            self.publish(&half, true, false, &params, generation, changed_at);
                            last_post = Some(Instant::now());
                            published_gen = generation;
                        }
                        return;
                    }
                    if converged || due(last_post, interval) {
                        let fin = self.publish(&film, false, converged, &params, generation, changed_at);
                        last_post = Some(Instant::now());
                        published_gen = generation;
                        if fin {
                            final_done = true;
                            if !converged {
                                settled_for = Some(max);
                            }
                            let mut st = self.stats.lock();
                            st.converged = true;
                            st.converge_s = changed_at.elapsed().as_secs_f32();
                        }
                    }
                    return;
                }
                if !final_done {
                    // max_spp lowered below what we have: publish as final.
                    self.publish(&film, false, true, &params, generation, changed_at);
                    last_post = Some(Instant::now());
                    published_gen = generation;
                    final_done = true;
                    self.stats.lock().converged = true;
                }
            }));
            if let Err(e) = r {
                let msg = format!("live view: render pass failed: {}", panic_text(&*e));
                eprintln!("argent serve: {msg}");
                self.stats.lock().error = Some(msg);
                // Give up on this generation until the set changes.
                film.samples = u32::MAX;
                final_done = true;
            }
        }
    }
}
