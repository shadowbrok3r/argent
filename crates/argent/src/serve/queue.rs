//! Final renders as background jobs: one at a time, the live preview paused
//! while one runs (they would fight for cores), every output under `--out`,
//! a JSON log of all jobs in `<out>/jobs.json`.
//!
//! Stills render in chunks (progress, cancel) with the same seed, look,
//! camera rule and denoise as `argent render`; sheets and reels call
//! `jobs::sheet_prepared` / `jobs::render_reel`, with progress read from
//! the files they write.

use super::params::{self, Params, Patch};
use super::pieces::{self, Piece, PieceSpec};
use super::App;
use crate::jobs;
use anyhow::{bail, Context, Result};
use argent_core::render::{render_pass, Film};
use argent_core::views::View;
use argent_post::{DenoiseBackend, DenoiseInput, Rgb8};
use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// More than this many waiting jobs is refused (a runaway agent loop).
const MAX_QUEUED: usize = 24;
/// Jobs kept in the log.
const LOG_KEEP: usize = 500;
const THUMB_SIDE: u32 = 480;

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `YYYYMMDD-HHMMSS` (UTC) for file names.
pub fn stamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Output {
    /// Path relative to `--out`.
    pub file: String,
    /// Where to fetch it (`/files/...`).
    pub url: String,
    /// Small JPEG preview, when the output is (or has) an image.
    pub thumb: Option<String>,
    /// `image`, `video` or `data`.
    pub kind: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    /// `still`, `sheet` or `reel`.
    pub kind: String,
    /// `queued`, `running`, `done`, `failed`, `cancelled` or `interrupted`.
    pub status: String,
    pub piece: String,
    pub piece_id: String,
    pub created: u64,
    #[serde(default)]
    pub started: Option<u64>,
    #[serde(default)]
    pub finished: Option<u64>,
    #[serde(default)]
    pub progress: f32,
    #[serde(default)]
    pub eta_s: Option<f32>,
    #[serde(default)]
    pub elapsed_s: Option<f32>,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub error: Option<String>,
    /// Everything the job renders with (kind options + the full set).
    pub spec: Value,
    #[serde(default)]
    pub outputs: Vec<Output>,
}

/// `POST /api/render` body.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderReq {
    pub kind: String,
    /// Library path or id to render instead of the piece on set.
    pub piece: Option<String>,
    /// Changes over the live set for this job only (same fields as `/api/look`).
    pub look: Option<Value>,
    /// Still/sheet frame: `story`, `listing`, `wide` or `WxH` (still default: the set's format).
    pub size: Option<String>,
    pub spp: Option<u32>,
    pub metals: Option<Vec<String>>,
    pub views: Option<Vec<String>>,
    pub title: Option<String>,
    pub seconds: Option<f32>,
    pub fps: Option<u32>,
    pub hook: Option<String>,
    pub rig: Option<String>,
    /// Reel soundtrack: a library audio file (path or id); default silent.
    pub audio: Option<String>,
}

#[derive(Clone, Debug)]
enum Kind {
    Still { w: u32, h: u32, spp: u32 },
    Sheet { metals: Vec<String>, views: Vec<String>, size: String, spp: u32, title: Option<String> },
    Reel { seconds: f32, fps: u32, spp: u32, hook: Option<String>, rig: String, audio: Option<PathBuf> },
}

enum PieceRef {
    Ready(Arc<Piece>),
    Load(PieceSpec),
}

/// How far a running job is.
enum Progress {
    /// Samples done / total (stills).
    Samples(Arc<AtomicU32>, u32),
    /// Files matching `prefix*suffix` in `dir` / total (sheet tiles, reel frames).
    Files { dir: PathBuf, prefix: String, suffix: String, total: u32 },
}

struct Work {
    kind: Kind,
    piece: PieceRef,
    params: Params,
    cancel: Arc<AtomicBool>,
    progress: Option<Progress>,
    started: Option<Instant>,
}

struct QState {
    jobs: Vec<JobRecord>,
    work: std::collections::HashMap<String, Work>,
    next: u64,
    /// Bumped when the list changes (not on progress ticks).
    log_version: u64,
}

pub struct Queue {
    st: Mutex<QState>,
    cv: Condvar,
    out: PathBuf,
}

fn rel_url(rel: &str) -> String {
    let enc: Vec<String> = rel.split('/').map(url_escape).collect();
    format!("/files/{}", enc.join("/"))
}

fn url_escape(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// Box-filter downscale so the longer side is ≤ `side`.
pub fn downscale(img: &Rgb8, side: u32) -> Rgb8 {
    let s = side as f64 / img.width.max(img.height) as f64;
    if s >= 1.0 {
        return img.clone();
    }
    let (w, h) = (((img.width as f64 * s).round() as u32).max(1), ((img.height as f64 * s).round() as u32).max(1));
    let mut out = Rgb8::new(w, h);
    for y in 0..h {
        let y0 = (y as u64 * img.height as u64 / h as u64) as u32;
        let y1 = (((y + 1) as u64 * img.height as u64 / h as u64) as u32).max(y0 + 1);
        for x in 0..w {
            let x0 = (x as u64 * img.width as u64 / w as u64) as u32;
            let x1 = (((x + 1) as u64 * img.width as u64 / w as u64) as u32).max(x0 + 1);
            let mut acc = [0u32; 3];
            for yy in y0..y1 {
                let row = (yy * img.width) as usize * 3;
                for xx in x0..x1 {
                    let i = row + xx as usize * 3;
                    for c in 0..3 {
                        acc[c] += img.data[i + c] as u32;
                    }
                }
            }
            let n = (y1 - y0) * (x1 - x0);
            let o = ((y * w + x) * 3) as usize;
            for c in 0..3 {
                out.data[o + c] = ((acc[c] + n / 2) / n) as u8;
            }
        }
    }
    out
}

impl Queue {
    /// Load the log (jobs that were running when the server died become
    /// `interrupted`).
    pub fn new(out: &Path) -> Queue {
        let mut jobs: Vec<JobRecord> = std::fs::read(out.join("jobs.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        for j in &mut jobs {
            if j.status == "queued" || j.status == "running" {
                j.status = "interrupted".into();
                j.message = "server stopped before it finished".into();
            }
        }
        let next = jobs.iter().filter_map(|j| j.id.trim_start_matches('j').parse::<u64>().ok()).max().unwrap_or(0) + 1;
        Queue {
            st: Mutex::new(QState { jobs, work: Default::default(), next, log_version: 1 }),
            cv: Condvar::new(),
            out: out.to_path_buf(),
        }
    }

    fn save(&self, st: &QState) {
        let keep = &st.jobs[st.jobs.len().saturating_sub(LOG_KEEP)..];
        let tmp = self.out.join(".jobs.json.tmp");
        let res = serde_json::to_vec_pretty(keep)
            .map_err(anyhow::Error::from)
            .and_then(|b| std::fs::write(&tmp, b).map_err(anyhow::Error::from))
            .and_then(|_| std::fs::rename(&tmp, self.out.join("jobs.json")).map_err(anyhow::Error::from));
        if let Err(e) = res {
            eprintln!("argent serve: writing jobs.json: {e:#}");
        }
    }

    pub fn log_version(&self) -> u64 {
        self.st.lock().log_version
    }

    /// Newest first.
    pub fn list(&self, limit: usize) -> Vec<JobRecord> {
        let st = self.st.lock();
        st.jobs.iter().rev().take(limit).cloned().collect()
    }

    pub fn get(&self, id: &str) -> Option<JobRecord> {
        self.st.lock().jobs.iter().find(|j| j.id == id).cloned()
    }

    /// Queued and running jobs, oldest first (small: sent on every tick).
    pub fn active(&self) -> Vec<JobRecord> {
        self.st.lock().jobs.iter().filter(|j| j.status == "queued" || j.status == "running").cloned().collect()
    }

    /// Validate a request against the live set and queue it.
    pub fn submit(&self, app: &App, req: RenderReq) -> Result<JobRecord> {
        let mut params = app.live.params();
        let piece = match &req.piece {
            Some(p) => {
                let spec = app.spec_for(p)?;
                match app.pieces.lock().get(&spec) {
                    Some(ready) => PieceRef::Ready(ready),
                    None => PieceRef::Load(spec),
                }
            }
            None => match app.live.piece() {
                Some(p) => PieceRef::Ready(p),
                None => bail!("no piece on set: open one first, or pass \"piece\""),
            },
        };
        // Stone overrides belong to the piece on set; another piece starts from its own.
        let on_set = app.live.piece().map(|p| p.id.clone());
        let target = match &piece {
            PieceRef::Ready(p) => p.id.clone(),
            PieceRef::Load(s) => s.id(),
        };
        if on_set.as_deref() != Some(target.as_str()) {
            params.stones.clear();
        }
        if let Some(l) = &req.look {
            let mut patch = Patch::from_value(l.clone()).context("look")?;
            app.resolve_patch(&mut patch)?;
            params.apply(&patch)?;
        }
        let spp_ok = |s: u32| -> Result<u32> {
            if !(1..=16_384).contains(&s) {
                bail!("spp must be 1…16384");
            }
            Ok(s)
        };
        let kind = match req.kind.trim().to_ascii_lowercase().as_str() {
            "still" | "render" | "image" => {
                let (_, w, h) = params::parse_format(req.size.as_deref().unwrap_or(&params.format))?;
                Kind::Still { w, h, spp: spp_ok(req.spp.unwrap_or(128))? }
            }
            "sheet" | "listing" => {
                let metals = req.metals.clone().unwrap_or_else(|| vec!["sterling".into(), "gold14k".into(), "bronze".into()]);
                if metals.is_empty() || metals.len() > 14 {
                    bail!("sheet needs 1…14 metals");
                }
                let metals = metals
                    .iter()
                    .map(|m| argent_spectra::alloys::Alloy::from_name(m).map(|a| a.key().to_string()).with_context(|| format!("unknown metal {m:?}")))
                    .collect::<Result<Vec<_>>>()?;
                let views = req.views.clone().unwrap_or_else(|| vec!["hero".into(), "front".into(), "side".into()]);
                if views.is_empty() || views.len() > 8 {
                    bail!("sheet needs 1…8 views");
                }
                let views = views
                    .iter()
                    .map(|v| params::view_name(v).map(str::to_string).with_context(|| format!("unknown view {v:?}; known: {}", View::NAMES.join(", "))))
                    .collect::<Result<Vec<_>>>()?;
                let (_, w, h) = params::parse_format(req.size.as_deref().unwrap_or("1600x1600"))?;
                let title = req.title.clone().map(|t| t.chars().take(80).collect());
                Kind::Sheet { metals, views, size: format!("{w}x{h}"), spp: spp_ok(req.spp.unwrap_or(128))?, title }
            }
            "reel" | "turntable" | "video" => {
                let seconds = req.seconds.unwrap_or(6.0);
                if !(1.0..=90.0).contains(&seconds) {
                    bail!("reel seconds must be 1…90 (Forge wants 3…90)");
                }
                let fps = req.fps.unwrap_or(30);
                if !(12..=60).contains(&fps) {
                    bail!("reel fps must be 12…60");
                }
                let rig = req.rig.clone().unwrap_or_else(|| "turntable".into());
                if jobs::Rig::from_name(&rig).is_none() {
                    bail!("unknown rig {rig:?}; known: turntable, reveal, sweep");
                }
                let hook = req.hook.clone().map(|h| h.trim().chars().take(120).collect::<String>()).filter(|h| !h.is_empty());
                let audio = match req.audio.as_deref().map(str::trim) {
                    None | Some("") | Some("none") | Some("silent") => None,
                    Some(a) => Some(app.library.resolve_asset(a, super::library::AUDIO_EXT)?),
                };
                Kind::Reel { seconds, fps, spp: spp_ok(req.spp.unwrap_or(32))?, hook, rig, audio }
            }
            k => bail!("unknown kind {k:?}: still, sheet or reel"),
        };
        let (piece_name, piece_id) = match &piece {
            PieceRef::Ready(p) => (p.name.clone(), p.id.clone()),
            PieceRef::Load(s) => (s.name(), s.id()),
        };
        let kind_json = match &kind {
            Kind::Still { w, h, spp } => json!({"size": format!("{w}x{h}"), "spp": spp}),
            Kind::Sheet { metals, views, size, spp, title } => json!({"metals": metals, "views": views, "size": size, "spp": spp, "title": title}),
            Kind::Reel { seconds, fps, spp, hook, rig, audio } => {
                json!({"seconds": seconds, "fps": fps, "spp": spp, "hook": hook, "rig": rig, "audio": audio, "size": "1080x1920"})
            }
        };
        let mut st = self.st.lock();
        let waiting = st.jobs.iter().filter(|j| j.status == "queued").count();
        if waiting >= MAX_QUEUED {
            bail!("{waiting} jobs are already waiting; try again when some finish");
        }
        let id = format!("j{}", st.next);
        st.next += 1;
        let rec = JobRecord {
            id: id.clone(),
            kind: match kind {
                Kind::Still { .. } => "still",
                Kind::Sheet { .. } => "sheet",
                Kind::Reel { .. } => "reel",
            }
            .into(),
            status: "queued".into(),
            piece: piece_name,
            piece_id,
            created: now_secs(),
            started: None,
            finished: None,
            progress: 0.0,
            eta_s: None,
            elapsed_s: None,
            message: "waiting".into(),
            error: None,
            spec: json!({"job": kind_json, "set": params}),
            outputs: vec![],
        };
        st.jobs.push(rec.clone());
        st.work.insert(id, Work { kind, piece, params, cancel: Arc::new(AtomicBool::new(false)), progress: None, started: None });
        st.log_version += 1;
        self.save(&st);
        drop(st);
        self.cv.notify_all();
        Ok(rec)
    }

    /// Cancel a queued job, or stop a running still at its next chunk.
    pub fn cancel(&self, id: &str) -> Result<JobRecord> {
        let mut st = self.st.lock();
        let Some(i) = st.jobs.iter().position(|j| j.id == id) else { bail!("no job {id}") };
        match st.jobs[i].status.as_str() {
            "queued" => {
                st.work.remove(id);
                let j = &mut st.jobs[i];
                j.status = "cancelled".into();
                j.message = "cancelled before it started".into();
                j.finished = Some(now_secs());
            }
            "running" => {
                let w = st.work.get(id).context("running job has no work")?;
                if !matches!(w.kind, Kind::Still { .. }) {
                    bail!("a running {} can't be stopped part-way; it finishes on its own", st.jobs[i].kind);
                }
                w.cancel.store(true, Ordering::SeqCst);
                st.jobs[i].message = "cancelling…".into();
            }
            s => bail!("job {id} is already {s}"),
        }
        st.log_version += 1;
        self.save(&st);
        Ok(st.jobs[i].clone())
    }

    /// Refresh progress of the running job; true when it moved.
    pub fn tick(&self) -> bool {
        let mut st = self.st.lock();
        let Some(i) = st.jobs.iter().position(|j| j.status == "running") else { return false };
        let id = st.jobs[i].id.clone();
        let Some(w) = st.work.get(&id) else { return false };
        let (Some(p), Some(t0)) = (&w.progress, w.started) else { return false };
        let frac = match p {
            Progress::Samples(done, total) => done.load(Ordering::Relaxed) as f32 / (*total).max(1) as f32,
            Progress::Files { dir, prefix, suffix, total } => {
                let n = std::fs::read_dir(dir)
                    .map(|rd| {
                        rd.flatten()
                            .filter(|e| {
                                let n = e.file_name().to_string_lossy().into_owned();
                                n.starts_with(prefix.as_str()) && n.ends_with(suffix.as_str())
                            })
                            .count()
                    })
                    .unwrap_or(0);
                n as f32 / (*total).max(1) as f32
            }
        }
        .clamp(0.0, 0.99);
        let el = t0.elapsed().as_secs_f32();
        let j = &mut st.jobs[i];
        let moved = (frac - j.progress).abs() > 0.001;
        j.progress = frac;
        j.elapsed_s = Some(el);
        j.eta_s = (frac > 0.02).then(|| el / frac - el);
        moved || el as u32 % 5 == 0
    }

    fn set_message(&self, id: &str, msg: &str) {
        let mut st = self.st.lock();
        if let Some(j) = st.jobs.iter_mut().find(|j| j.id == id) {
            j.message = msg.into();
        }
    }

    fn set_progress(&self, id: &str, p: Progress) {
        let mut st = self.st.lock();
        if let Some(w) = st.work.get_mut(id) {
            w.progress = Some(p);
        }
    }

    /// The runner thread: one job at a time, the live view paused meanwhile.
    pub fn run(app: Arc<App>) {
        let q = app.queue.clone();
        loop {
            let (id, kind, piece, params, cancel) = {
                let mut st = q.st.lock();
                loop {
                    if app.stopping.load(Ordering::SeqCst) {
                        return;
                    }
                    let next = st.jobs.iter().position(|j| j.status == "queued");
                    if let Some(i) = next {
                        let id = st.jobs[i].id.clone();
                        if let Some(w) = st.work.get_mut(&id) {
                            w.started = Some(Instant::now());
                            let piece = match &w.piece {
                                PieceRef::Ready(p) => PieceRef::Ready(p.clone()),
                                PieceRef::Load(s) => PieceRef::Load(s.clone()),
                            };
                            let out = (id.clone(), w.kind.clone(), piece, w.params.clone(), w.cancel.clone());
                            let j = &mut st.jobs[i];
                            j.status = "running".into();
                            j.started = Some(now_secs());
                            j.message = "starting".into();
                            st.log_version += 1;
                            q.save(&st);
                            break out;
                        }
                        // A queued record without work (shouldn't happen): drop it.
                        st.jobs[i].status = "failed".into();
                        st.jobs[i].error = Some("lost its work order".into());
                        continue;
                    }
                    q.cv.wait_for(&mut st, std::time::Duration::from_millis(500));
                }
            };
            app.notify_jobs();
            app.live.pause();
            app.notify_live();
            let t0 = Instant::now();
            let res = catch_unwind(AssertUnwindSafe(|| q.execute(&app, &id, kind, piece, params, &cancel)));
            app.live.resume();
            app.notify_live();
            let mut st = q.st.lock();
            st.work.remove(&id);
            if let Some(j) = st.jobs.iter_mut().find(|j| j.id == id) {
                j.finished = Some(now_secs());
                j.elapsed_s = Some(t0.elapsed().as_secs_f32());
                j.eta_s = None;
                match res {
                    Ok(Ok(outputs)) => {
                        j.status = "done".into();
                        j.progress = 1.0;
                        j.message = format!("done in {:.0} s", t0.elapsed().as_secs_f32());
                        j.outputs = outputs;
                    }
                    Ok(Err(e)) if cancel.load(Ordering::SeqCst) => {
                        j.status = "cancelled".into();
                        j.message = format!("{e:#}");
                    }
                    Ok(Err(e)) => {
                        j.status = "failed".into();
                        j.error = Some(format!("{e:#}"));
                        j.message = "failed".into();
                        eprintln!("argent serve: job {id} failed: {e:#}");
                    }
                    Err(p) => {
                        let msg = p
                            .downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .or_else(|| p.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "panic".into());
                        j.status = "failed".into();
                        j.error = Some(format!("crashed: {msg}"));
                        j.message = "failed".into();
                        eprintln!("argent serve: job {id} panicked: {msg}");
                    }
                }
            }
            st.log_version += 1;
            q.save(&st);
            drop(st);
            app.notify_jobs();
        }
    }

    fn thumb(&self, id: &str, k: usize, img: &Rgb8) -> Option<String> {
        let rel = format!(".thumbs/{id}-{k}.jpg");
        let path = self.out.join(&rel);
        std::fs::create_dir_all(path.parent()?).ok()?;
        let small = downscale(img, THUMB_SIDE);
        let bytes = super::live::encode_jpeg(&small, 82).ok()?;
        std::fs::write(&path, bytes).ok()?;
        Some(rel_url(&rel))
    }

    fn output(&self, id: &str, k: usize, rel: &str, kind: &str, label: &str, img: Option<&Rgb8>) -> Output {
        let thumb = match img {
            Some(i) => self.thumb(id, k, i),
            None if kind == "image" => argent_post::io::read_rgb8(&self.out.join(rel)).ok().and_then(|i| self.thumb(id, k, &i)),
            None => None,
        };
        Output { file: rel.into(), url: rel_url(rel), thumb, kind: kind.into(), label: label.into() }
    }

    fn execute(&self, app: &App, id: &str, kind: Kind, piece: PieceRef, p: Params, cancel: &AtomicBool) -> Result<Vec<Output>> {
        let piece = match piece {
            PieceRef::Ready(p) => p,
            PieceRef::Load(spec) => {
                self.set_message(id, "preparing the piece");
                app.notify_jobs();
                app.load_piece(&spec)?
            }
        };
        let slug = piece.spec.slug();
        let when = stamp(now_secs());
        match kind {
            Kind::Still { w, h, spp } => {
                let size = format!("{w}x{h}");
                let shot = p.shot(&size, spp);
                let lk = jobs::look(&shot)?;
                let q = jobs::quality(&shot)?;
                let mut scene = piece.prepared.scene(&lk);
                let (cam, turn) = params::shot_camera(&piece.prepared, &p, q.width as f32 / q.height as f32);
                scene.turntable_deg = turn;
                let done = Arc::new(AtomicU32::new(0));
                self.set_progress(id, Progress::Samples(done.clone(), q.spp + q.spp / 8));
                self.set_message(id, &format!("rendering {w}×{h} at {} spp", q.spp));
                app.notify_jobs();
                let mut film = Film::new(q.width, q.height);
                let mut chunk = 2u32;
                while film.samples < q.spp {
                    if cancel.load(Ordering::SeqCst) {
                        bail!("cancelled at {} of {} spp", film.samples, q.spp);
                    }
                    let n = chunk.min(q.spp - film.samples);
                    let t = Instant::now();
                    // Seed 7 and sample indices continue across chunks: the
                    // same image `argent render` makes in one pass.
                    render_pass(&scene, &cam, &mut film, &q.path, n, 7);
                    done.store(film.samples, Ordering::Relaxed);
                    let per = t.elapsed().as_secs_f64() / n as f64;
                    chunk = ((1.5 / per.max(1e-4)) as u32).clamp(1, 64);
                }
                self.set_message(id, "denoising");
                app.notify_jobs();
                let color = film.color();
                let img = if q.denoise {
                    let (albedo, normal) = (film.albedo(), film.normal());
                    argent_post::denoise(DenoiseInput { color: &color, albedo: Some(&albedo), normal: Some(&normal) }, DenoiseBackend::Auto)?
                } else {
                    color
                };
                let png = argent_post::develop(&img, &argent_core::shot::grade(&lk));
                let view = if p.view == "custom" { "custom".to_string() } else { p.view.clone() };
                let rel = format!("stills/{slug}-{}-{view}-{size}-{when}-{id}.png", p.metal);
                jobs::write_image(&self.out.join(&rel), &png)?;
                done.store(q.spp + q.spp / 8, Ordering::Relaxed);
                let label = format!("{} · {} · {}", argent_spectra::alloys::Alloy::from_name(&p.metal).map(|a| a.display()).unwrap_or(""), p.studio, size);
                Ok(vec![self.output(id, 0, &rel, "image", &label, Some(&png))])
            }
            Kind::Sheet { metals, views, size, spp, title } => {
                let rel_dir = format!("sheets/{slug}-{when}-{id}");
                let dir = self.out.join(&rel_dir);
                std::fs::create_dir_all(&dir)?;
                let shot = p.shot(&size, spp);
                let total = (metals.len() * views.len()) as u32;
                self.set_progress(id, Progress::Files { dir: dir.clone(), prefix: format!("{slug}-"), suffix: ".png".into(), total: total + 1 });
                self.set_message(id, &format!("{total} renders at {size}, {spp} spp"));
                app.notify_jobs();
                let title = title.or_else(|| Some(piece.name.clone()));
                let manifest = jobs::sheet_prepared(&piece.prepared, &slug, &shot, &dir, &metals, &views, title)?;
                let m = serde_json::to_value(&manifest)?;
                let mut outs = vec![];
                if let Some(cs) = m.get("contact_sheet").and_then(|v| v.as_str()) {
                    outs.push(self.output(id, 0, &format!("{rel_dir}/{cs}"), "image", "Contact sheet", None));
                }
                for (k, it) in m.get("items").and_then(|v| v.as_array()).into_iter().flatten().enumerate() {
                    let (Some(f), Some(metal), Some(view)) =
                        (it.get("file").and_then(|v| v.as_str()), it.get("metal").and_then(|v| v.as_str()), it.get("view").and_then(|v| v.as_str()))
                    else {
                        continue;
                    };
                    outs.push(self.output(id, k + 1, &format!("{rel_dir}/{f}"), "image", &format!("{metal} · {view}"), None));
                }
                let json_rel = format!("{rel_dir}/{slug}.json");
                if self.out.join(&json_rel).is_file() {
                    outs.push(Output { file: json_rel.clone(), url: rel_url(&json_rel), thumb: None, kind: "data".into(), label: "Manifest".into() });
                }
                Ok(outs)
            }
            Kind::Reel { seconds, fps, spp, hook, rig, audio } => {
                let rel = format!("reels/{slug}-{}-{rig}-{when}-{id}.mp4", p.metal);
                let path = self.out.join(&rel);
                let work = self.out.join(".work").join(format!("{id}-frames"));
                std::fs::create_dir_all(&work)?;
                let n = (seconds * fps as f32).round().max(2.0) as u32;
                self.set_progress(id, Progress::Files { dir: work.clone(), prefix: "frame_".into(), suffix: ".png".into(), total: n });
                self.set_message(id, &format!("{n} frames, {spp} spp, {rig}"));
                app.notify_jobs();
                let shot = p.shot("1080x1920", spp);
                let rig_v = jobs::Rig::from_name(&rig).context("rig")?;
                let res = jobs::render_reel(&piece.prepared, &shot, &path, seconds, fps, hook.as_deref(), audio, Some(work.clone()), rig_v);
                let poster = argent_post::io::read_rgb8(&work.join("frame_0000.png")).ok();
                std::fs::remove_dir_all(&work).ok();
                res?;
                let mut out = self.output(id, 0, &rel, "video", &format!("{} · {rig} · {seconds:.0} s", p.metal), None);
                if let Some(img) = poster {
                    out.thumb = self.thumb(id, 0, &img);
                    let poster_rel = rel.replace(".mp4", "-poster.jpg");
                    if let Ok(b) = super::live::encode_jpeg(&img, 88) {
                        std::fs::write(self.out.join(&poster_rel), b).ok();
                    }
                }
                Ok(vec![out])
            }
        }
    }
}

#[allow(dead_code)]
fn _assert_send() {
    fn is_send<T: Send>() {}
    is_send::<pieces::Piece>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps() {
        assert_eq!(stamp(0), "19700101-000000");
        assert_eq!(stamp(1_792_800_000), "20261024-000000");
    }

    #[test]
    fn downscales() {
        let mut img = Rgb8::new(10, 4);
        img.data.iter_mut().enumerate().for_each(|(i, v)| *v = (i % 251) as u8);
        let s = downscale(&img, 5);
        assert_eq!((s.width, s.height), (5, 2));
    }
}
