//! Live studio: `argent serve`.
//!
//! One shared set (piece + look + camera) that a phone and agents
//! art-direct together. A render worker refines it progressively and
//! streams JPEG frames over a WebSocket; stills, listing sheets and reels
//! run as queued background jobs that write under `--out`. The page is
//! embedded in the binary (no CDN, no build step). API: `serve/API.md`.

mod demo;
mod files;
mod http;
mod library;
mod live;
mod params;
mod pieces;
mod queue;

use anyhow::{bail, Context, Result};
use parking_lot::Mutex;
use params::{Params, Patch};
use pieces::{Piece, PieceSpec, Pieces, Status};
use serde::Deserialize;
use serde_json::{json, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;

/// Change counters per topic; WebSocket clients get a message per topic
/// that moved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Topics {
    pub params: u64,
    pub piece: u64,
    pub jobs: u64,
    pub live: u64,
}

pub struct App {
    pub library: library::Library,
    /// Canonical output root: every file the studio writes or serves.
    pub out: PathBuf,
    pub live: Arc<live::Live>,
    pub queue: Arc<queue::Queue>,
    pub pieces: Mutex<Pieces>,
    pub topics: watch::Sender<Topics>,
    pub stopping: AtomicBool,
    pub started: Instant,
}

/// `POST /api/open` body.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenReq {
    /// Library path (absolute, or relative to a library root) …
    pub path: Option<String>,
    /// … or a library id from `GET /api/library`.
    pub id: Option<String>,
    #[serde(default)]
    pub flip: bool,
    /// Extra rotation after posing, degrees "x,y,z".
    pub rotate: Option<String>,
    /// mm (default), cm, m, in.
    pub units: Option<String>,
    #[serde(default)]
    pub no_orient: bool,
    /// Pendant chain from the bail: auto (pendants), on, off.
    pub chain: Option<String>,
    /// Scale so the largest dimension is this many mm (models in unknown units).
    pub fit: Option<f32>,
    /// Multiply all coordinates by this factor (after `units`).
    pub scale: Option<f32>,
    /// Block until the piece is ready (or failed).
    #[serde(default)]
    pub wait: bool,
    /// Look changes to apply once it is on set.
    pub look: Option<Value>,
}

impl App {
    pub fn notify_params(&self) {
        self.topics.send_modify(|t| t.params += 1);
    }
    pub fn notify_piece(&self) {
        self.topics.send_modify(|t| t.piece += 1);
    }
    pub fn notify_jobs(&self) {
        self.topics.send_modify(|t| t.jobs += 1);
    }
    pub fn notify_live(&self) {
        self.topics.send_modify(|t| t.live += 1);
    }

    /// A library path, library id or `demo:ring` → what to prepare.
    pub fn spec_for(&self, target: &str) -> Result<PieceSpec> {
        let t = target.trim();
        if t == demo::ID || t == "demo" {
            if !self.library.demo {
                bail!("the demo piece is off (set ARGENT_STUDIO_DEMO=1)");
            }
            return Ok(PieceSpec::demo());
        }
        let looks_like_id = t.len() == 10 && t.bytes().all(|b| b.is_ascii_hexdigit());
        let file = if looks_like_id { self.library.resolve_id(t)? } else { self.library.resolve(t)? };
        Ok(PieceSpec::file(file))
    }

    fn open_spec(&self, req: &OpenReq) -> Result<PieceSpec> {
        let target = req.path.as_deref().or(req.id.as_deref()).context("open needs \"path\" or \"id\"")?;
        let mut spec = self.spec_for(target)?;
        if spec.demo {
            return Ok(spec);
        }
        spec.flip = req.flip;
        spec.no_orient = req.no_orient;
        if let Some(r) = &req.rotate {
            let parts: Vec<&str> = r.split(',').collect();
            if parts.len() != 3 || parts.iter().any(|p| p.trim().parse::<f32>().map(|v| !v.is_finite()).unwrap_or(true)) {
                bail!("rotate must be \"x,y,z\" degrees");
            }
            spec.rotate = Some(r.trim().to_string());
        }
        if let Some(u) = &req.units {
            let u = u.trim().to_ascii_lowercase();
            if !matches!(u.as_str(), "mm" | "cm" | "m" | "in" | "inch") {
                bail!("units must be mm, cm, m or in");
            }
            spec.units = u;
        }
        if let Some(c) = &req.chain {
            let c = c.trim().to_ascii_lowercase();
            if !matches!(c.as_str(), "auto" | "on" | "off") {
                bail!("chain must be auto, on or off");
            }
            spec.chain = c;
        }
        if let Some(f) = req.fit {
            if !(f.is_finite() && (0.5..=500.0).contains(&f)) {
                bail!("fit must be 0.5…500 mm");
            }
            spec.fit = Some(pieces::Num(f));
        }
        if let Some(k) = req.scale {
            if !(k.is_finite() && (1e-4..=1e4).contains(&k)) {
                bail!("scale must be a positive factor (1e-4…1e4)");
            }
            spec.scale = Some(pieces::Num(k));
        }
        Ok(spec)
    }

    /// Turn client-supplied asset references in a patch into canonical
    /// library paths (HDRIs can only come from the library).
    pub fn resolve_patch(&self, patch: &mut Patch) -> Result<()> {
        if let Some(h) = &patch.hdri {
            let t = h.trim();
            if !(t.is_empty() || ["none", "off", "studio", "metal"].iter().any(|w| t.eq_ignore_ascii_case(w))) {
                let path = self.library.resolve_asset(t, library::HDRI_EXT)?;
                patch.hdri = Some(path.to_string_lossy().into_owned());
            }
        }
        Ok(())
    }

    /// Open a piece: instantly from the cache, else prepare it on a thread.
    /// Returns the request token and the immediate status.
    pub fn open(self: &Arc<Self>, req: OpenReq) -> Result<(u64, Value)> {
        let spec = self.open_spec(&req)?;
        let mut look = req.look.clone().map(Patch::from_value).transpose().context("look")?;
        if let Some(l) = look.as_mut() {
            self.resolve_patch(l)?;
        }
        if let Some(l) = &look {
            // Validate before anything moves.
            self.live.params().apply(l)?;
        }
        let mut ps = self.pieces.lock();
        ps.token += 1;
        let token = ps.token;
        if let Some(p) = ps.get(&spec) {
            ps.current = Some(p.clone());
            ps.status = Status::Idle;
            drop(ps);
            self.put_on_set(p.clone(), look.as_ref());
            return Ok((token, json!({"status": "ready", "token": token, "piece": p.info(&self.live.params().stones)})));
        }
        let est = (spec.est_triangles() * ps.secs_per_tri) as f32 + 0.3;
        let (id, name) = (spec.id(), spec.name());
        ps.status = Status::Preparing { id: id.clone(), name: name.clone(), started: Instant::now(), est_secs: est };
        drop(ps);
        self.live.set_piece(None, None);
        self.notify_piece();
        let app = self.clone();
        let reply = json!({"status": "preparing", "token": token, "id": id, "name": name, "estimate": est});
        std::thread::Builder::new().name("argent-prepare".into()).spawn(move || {
            let r = catch_unwind(AssertUnwindSafe(|| pieces::prepare(&spec)));
            let result = match r {
                Ok(Ok(p)) => Ok(Arc::new(p)),
                Ok(Err(e)) => Err(format!("{e:#}")),
                Err(p) => Err(format!(
                    "crashed: {}",
                    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default()
                )),
            };
            let mut ps = app.pieces.lock();
            match result {
                Ok(piece) => {
                    ps.learn(piece.prepared.mesh.triangles.len(), piece.prepare_secs);
                    let mine = ps.token == token;
                    if mine {
                        ps.current = Some(piece.clone());
                        ps.status = Status::Idle;
                    }
                    ps.insert(piece.clone());
                    drop(ps);
                    eprintln!("argent serve: prepared {} in {:.1} s", piece.name, piece.prepare_secs);
                    if mine {
                        app.put_on_set(piece, look.as_ref());
                    }
                }
                Err(error) => {
                    eprintln!("argent serve: preparing {name} failed: {error}");
                    if ps.token == token {
                        ps.status = Status::Failed { id, name, error };
                        let prev = ps.current.clone();
                        drop(ps);
                        let stones = app.live.params().stones;
                        app.live.set_piece(prev, Some(stones));
                    }
                }
            }
            app.notify_piece();
        })?;
        Ok((token, reply))
    }

    fn put_on_set(&self, p: Arc<Piece>, look: Option<&Patch>) {
        self.live.set_piece(Some(p), Some(vec![]));
        if let Some(l) = look
            && let Err(e) = self.live.update(l)
        {
            eprintln!("argent serve: look on open: {e:#}");
        }
        self.notify_params();
        self.notify_piece();
    }

    /// Final outcome of open request `token`, once there is one.
    pub fn open_result(&self, token: u64) -> Option<Value> {
        let ps = self.pieces.lock();
        if ps.token != token {
            return Some(json!({"status": "superseded", "note": "a newer open request replaced this one"}));
        }
        match &ps.status {
            Status::Preparing { .. } => None,
            Status::Failed { error, .. } => Some(json!({"status": "failed", "error": error})),
            Status::Idle => {
                let stones = self.live.params().stones;
                Some(json!({"status": "ready", "token": token, "piece": ps.current.as_ref().map(|p| p.info(&stones))}))
            }
        }
    }

    /// Blocking load for a job that names a piece not on set.
    pub fn load_piece(&self, spec: &PieceSpec) -> Result<Arc<Piece>> {
        if let Some(p) = self.pieces.lock().get(spec) {
            return Ok(p);
        }
        let piece = Arc::new(pieces::prepare(spec)?);
        let mut ps = self.pieces.lock();
        ps.learn(piece.prepared.mesh.triangles.len(), piece.prepare_secs);
        ps.insert(piece.clone());
        Ok(piece)
    }

    pub fn update_look(&self, patch: &Patch) -> Result<Params> {
        let mut patch = patch.clone();
        self.resolve_patch(&mut patch)?;
        let before = self.live.params().stones;
        let p = self.live.update(&patch)?;
        self.notify_params();
        if p.stones != before {
            self.notify_piece();
        }
        Ok(p)
    }

    pub fn piece_json(&self) -> Value {
        let stones = self.live.params().stones;
        self.pieces.lock().status_json(&stones)
    }

    pub fn jobs_json(&self) -> Value {
        json!({"active": self.queue.active(), "log_version": self.queue.log_version()})
    }

    pub fn state(&self) -> Value {
        json!({
            "params": self.live.params(),
            "piece": self.piece_json(),
            "live": self.live.stats(),
            "jobs": self.jobs_json(),
            "library": {"roots": self.library.roots, "demo": self.library.demo},
            "out": self.out,
            "uptime_s": self.started.elapsed().as_secs(),
            "version": env!("CARGO_PKG_VERSION"),
        })
    }
}

pub fn serve(host: &str, port: u16, library: Vec<PathBuf>, out: PathBuf) -> Result<()> {
    std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    let out = std::fs::canonicalize(&out)?;
    let library = library::Library::new(&library);
    if library.roots.is_empty() {
        eprintln!("argent serve: no --library directories; only the demo piece is available");
    }
    let (topics, _) = watch::channel(Topics::default());
    let app = Arc::new(App {
        library,
        live: live::Live::start(Params::default()),
        queue: Arc::new(queue::Queue::new(&out)),
        out,
        pieces: Mutex::new(Pieces::default()),
        topics,
        stopping: AtomicBool::new(false),
        started: Instant::now(),
    });
    let runner = app.clone();
    std::thread::Builder::new().name("argent-jobs".into()).spawn(move || queue::Queue::run(runner))?;
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).thread_name("argent-http").enable_all().build()?;
    let res = rt.block_on(http::run(app.clone(), host, port));
    app.stopping.store(true, Ordering::SeqCst);
    app.live.shutdown();
    res
}
