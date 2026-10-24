//! Routes, JSON API and the WebSocket stream.

use super::live::Frame;
use super::params::{self, Patch};
use super::queue::RenderReq;
use super::{files, App, OpenReq, Topics};
use anyhow::Result;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const INDEX: &str = include_str!("../../assets/studio.html");
const CSS: &str = include_str!("../../assets/studio.css");
const JS: &str = include_str!("../../assets/studio.js");
const ICON: &str = include_str!("../../assets/icon.svg");
const MANIFEST: &str = include_str!("../../assets/manifest.webmanifest");

/// Per-client floor between frames (the publisher already paces at 6–12 fps).
const MIN_FRAME_GAP: Duration = Duration::from_millis(66);
/// Frames a client may have unacknowledged (bounds latency on slow links).
const CREDITS: i32 = 2;
/// A client that never acks still gets a frame this often.
const ACK_TIMEOUT: Duration = Duration::from_secs(3);

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError(StatusCode::BAD_REQUEST, format!("{e:#}"))
    }
}

type ApiResult<T> = std::result::Result<T, ApiError>;

fn bad(msg: impl std::fmt::Display) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.to_string())
}

fn body_json(b: &Bytes) -> ApiResult<Value> {
    if b.iter().all(|c| c.is_ascii_whitespace()) {
        return Ok(json!({}));
    }
    serde_json::from_slice(b).map_err(|e| bad(format!("invalid JSON body: {e}")))
}

fn parse<T: for<'de> Deserialize<'de>>(b: &Bytes) -> ApiResult<T> {
    serde_json::from_value(body_json(b)?).map_err(|e| bad(e))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> ApiResult<T> {
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r.map_err(ApiError::from),
        Err(e) => Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("internal error: {e}"))),
    }
}

fn asset(ct: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "no-cache")], body).into_response()
}

pub async fn run(app: Arc<App>, host: &str, port: u16) -> Result<()> {
    let router = Router::new()
        .route("/", get(|| async { asset("text/html; charset=utf-8", INDEX) }))
        .route("/studio.css", get(|| async { asset("text/css; charset=utf-8", CSS) }))
        .route("/studio.js", get(|| async { asset("text/javascript; charset=utf-8", JS) }))
        .route("/icon.svg", get(|| async { asset("image/svg+xml", ICON) }))
        .route("/manifest.webmanifest", get(|| async { asset("application/manifest+json", MANIFEST) }))
        .route("/api/options", get(|| async { Json(params::options()) }))
        .route("/api/state", get(api_state))
        .route("/api/library", get(api_library))
        .route("/api/open", post(api_open))
        .route("/api/look", get(api_look_get).post(api_look_post))
        .route("/api/frame.jpg", get(api_frame))
        .route("/api/ws", get(api_ws))
        .route("/api/render", post(api_render))
        .route("/api/jobs", get(api_jobs))
        .route("/api/jobs/{id}", get(api_job))
        .route("/api/jobs/{id}/cancel", post(api_cancel))
        .route("/files/{*path}", get(api_files))
        .with_state(app.clone());
    let listener = tokio::net::TcpListener::bind((host, port)).await?;
    eprintln!(
        "argent studio: http://{}:{}/  (library: {}; out: {})",
        if host == "0.0.0.0" { "<this-host>" } else { host },
        port,
        if app.library.roots.is_empty() { "none".into() } else { app.library.roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ") },
        app.out.display()
    );
    tokio::spawn(ticker(app.clone()));
    axum::serve(listener, router).with_graceful_shutdown(shutdown_signal()).await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
    eprintln!("argent studio: shutting down");
}

/// Job progress and piece-preparation progress, once a second.
async fn ticker(app: Arc<App>) {
    let mut iv = tokio::time::interval(Duration::from_secs(1));
    loop {
        iv.tick().await;
        let q = app.queue.clone();
        if tokio::task::spawn_blocking(move || q.tick()).await.unwrap_or(false) {
            app.notify_jobs();
        }
        if matches!(app.pieces.lock().status, super::pieces::Status::Preparing { .. }) {
            app.notify_piece();
        }
    }
}

async fn api_state(State(app): State<Arc<App>>) -> Json<Value> {
    Json(app.state())
}

async fn api_library(State(app): State<Arc<App>>) -> ApiResult<Json<Value>> {
    let a = app.clone();
    let entries = blocking(move || Ok(a.library.scan())).await?;
    let (cached, current) = {
        let ps = app.pieces.lock();
        (ps.cached_ids(), ps.current.as_ref().map(|p| p.id.clone()))
    };
    let mut pieces: Vec<Value> = entries
        .into_iter()
        .map(|e| {
            let mut v = json!(e);
            v["cached"] = json!(cached.contains(&e.id));
            v
        })
        .collect();
    if app.library.demo {
        pieces.insert(
            0,
            json!({"id": super::demo::ID, "name": super::demo::NAME, "path": super::demo::ID, "rel": "built in", "group": "demo",
                   "bytes": 0, "triangles": 230_432, "stones": true, "modified": 0, "cached": cached.iter().any(|c| c == super::demo::ID)}),
        );
    }
    let a = app.clone();
    let (hdris, audio) = blocking(move || Ok((a.library.assets(super::library::HDRI_EXT), a.library.assets(super::library::AUDIO_EXT)))).await?;
    Ok(Json(json!({"roots": app.library.roots, "demo": app.library.demo, "current": current, "pieces": pieces, "hdris": hdris, "audio": audio})))
}

async fn api_open(State(app): State<Arc<App>>, body: Bytes) -> ApiResult<Json<Value>> {
    let req: OpenReq = parse(&body)?;
    let wait = req.wait;
    let a = app.clone();
    let (token, reply) = blocking(move || a.open(req)).await?;
    if !wait || reply["status"] == "ready" {
        return Ok(Json(reply));
    }
    let mut rx = app.topics.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    loop {
        if let Some(v) = app.open_result(token) {
            if v["status"] == "failed" {
                return Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, v["error"].as_str().unwrap_or("failed").to_string()));
            }
            return Ok(Json(v));
        }
        if tokio::time::timeout_at(deadline, rx.changed()).await.is_err() {
            return Ok(Json(json!({"status": "preparing", "token": token, "note": "still preparing; poll GET /api/state"})));
        }
    }
}

async fn api_look_get(State(app): State<Arc<App>>) -> Json<Value> {
    Json(json!(app.live.params()))
}

async fn api_look_post(State(app): State<Arc<App>>, body: Bytes) -> ApiResult<Json<Value>> {
    let patch = Patch::from_value(body_json(&body)?)?;
    let p = app.update_look(&patch)?;
    Ok(Json(json!(p)))
}

#[derive(Deserialize)]
struct FrameQuery {
    min_spp: Option<u32>,
    wait: Option<f32>,
}

fn frame_response(f: &Frame) -> Response {
    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(v) = HeaderValue::from_str(&f.header) {
        h.insert("x-argent-frame", v);
    }
    h.insert("x-argent-spp", HeaderValue::from(f.spp));
    h.insert("x-argent-seq", HeaderValue::from(f.seq));
    h.insert("x-argent-gen", HeaderValue::from(f.generation));
    (StatusCode::OK, h, f.jpeg()).into_response()
}

/// Latest frame; `min_spp` + `wait` block until the current set has
/// refined that far (or converged).
async fn api_frame(State(app): State<Arc<App>>, Query(q): Query<FrameQuery>) -> Response {
    app.live.poke();
    let want = q.min_spp.unwrap_or(0);
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f32(q.wait.unwrap_or(0.0).clamp(0.0, 300.0));
    let mut rx = app.live.frames.subscribe();
    loop {
        let generation = app.live.generation();
        let f = rx.borrow_and_update().clone();
        if let Some(f) = &f
            && f.generation == generation
            && (f.spp >= want || f.converged)
        {
            return frame_response(f);
        }
        if tokio::time::Instant::now() >= deadline {
            return match &f {
                Some(f) => frame_response(f),
                None => ApiError(StatusCode::NOT_FOUND, "no frame yet: open a piece first".into()).into_response(),
            };
        }
        app.live.poke();
        let _ = tokio::time::timeout_at(deadline, rx.changed()).await;
    }
}

async fn api_render(State(app): State<Arc<App>>, body: Bytes) -> ApiResult<(StatusCode, Json<Value>)> {
    let req: RenderReq = parse(&body)?;
    let a = app.clone();
    let rec = blocking(move || a.queue.submit(&a, req)).await?;
    app.notify_jobs();
    Ok((StatusCode::ACCEPTED, Json(json!(rec))))
}

#[derive(Deserialize)]
struct JobsQuery {
    limit: Option<usize>,
}

async fn api_jobs(State(app): State<Arc<App>>, Query(q): Query<JobsQuery>) -> Json<Value> {
    Json(json!({"jobs": app.queue.list(q.limit.unwrap_or(100).min(500)), "log_version": app.queue.log_version()}))
}

#[derive(Deserialize)]
struct JobQuery {
    wait: Option<f32>,
}

/// One job; `wait=S` long-polls until it finishes (≤ 300 s).
async fn api_job(State(app): State<Arc<App>>, Path(id): Path<String>, Query(q): Query<JobQuery>) -> ApiResult<Json<Value>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f32(q.wait.unwrap_or(0.0).clamp(0.0, 300.0));
    let mut rx = app.topics.subscribe();
    loop {
        let Some(j) = app.queue.get(&id) else { return Err(ApiError(StatusCode::NOT_FOUND, format!("no job {id}"))) };
        let finished = !matches!(j.status.as_str(), "queued" | "running");
        if finished || tokio::time::Instant::now() >= deadline {
            return Ok(Json(json!(j)));
        }
        let _ = tokio::time::timeout_at(deadline, rx.changed()).await;
    }
}

async fn api_cancel(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let j = app.queue.cancel(&id).map_err(|e| ApiError(StatusCode::CONFLICT, format!("{e:#}")))?;
    app.notify_jobs();
    Ok(Json(json!(j)))
}

async fn api_files(State(app): State<Arc<App>>, Path(rel): Path<String>, Query(q): Query<HashMap<String, String>>, headers: HeaderMap) -> Response {
    files::serve(app.out.clone(), rel, headers, q.contains_key("dl")).await
}

async fn api_ws(ws: WebSocketUpgrade, State(app): State<Arc<App>>) -> Response {
    ws.max_message_size(1 << 20).on_upgrade(move |socket| client(socket, app))
}

enum Inbound {
    Ack,
    /// The page was hidden (false) or shown again (true).
    Visible(bool),
    Reply(Value),
    Nothing,
}

fn handle_text(app: &App, text: &str) -> std::result::Result<Inbound, String> {
    let mut v: Value = serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
    match kind.as_str() {
        "ack" => Ok(Inbound::Ack),
        "visibility" => Ok(Inbound::Visible(v.get("visible").and_then(|b| b.as_bool()).unwrap_or(true))),
        "ping" => Ok(Inbound::Reply(json!({"type": "pong", "t": v.get("t")}))),
        "look" => {
            if let Some(o) = v.as_object_mut() {
                o.remove("type");
                o.remove("rid");
            }
            let patch = Patch::from_value(v).map_err(|e| format!("{e:#}"))?;
            app.update_look(&patch).map_err(|e| format!("{e:#}"))?;
            Ok(Inbound::Nothing)
        }
        "" => Err("message needs a \"type\"".into()),
        k => Err(format!("unknown message type {k:?}")),
    }
}

async fn send_json(socket: &mut WebSocket, v: Value) -> bool {
    socket.send(Message::Text(v.to_string().into())).await.is_ok()
}

async fn send_topics(socket: &mut WebSocket, app: &App, seen: Topics, now: Topics) -> bool {
    if now.params != seen.params && !send_json(socket, json!({"type": "params", "params": app.live.params()})).await {
        return false;
    }
    if now.piece != seen.piece && !send_json(socket, json!({"type": "piece", "piece": app.piece_json()})).await {
        return false;
    }
    if now.jobs != seen.jobs && !send_json(socket, json!({"type": "jobs", "jobs": app.jobs_json()})).await {
        return false;
    }
    if now.live != seen.live && !send_json(socket, json!({"type": "live", "live": app.live.stats()})).await {
        return false;
    }
    true
}

/// One browser: frames (credit-based, latest wins) + state deltas out;
/// look changes, acks and page visibility in. A hidden page stops counting
/// as a viewer, so a forgotten tab never keeps the renderer busy.
async fn client(mut socket: WebSocket, app: Arc<App>) {
    app.live.viewer(1);
    let mut visible = true;
    let mut frames = app.live.frames.subscribe();
    let mut topics = app.topics.subscribe();
    let mut seen = *topics.borrow_and_update();
    let hello = json!({"type": "hello", "options": params::options(), "state": app.state()});
    let mut pending = frames.borrow_and_update().is_some();
    let mut credits = CREDITS;
    let mut last_sent = Instant::now() - Duration::from_secs(1);
    let mut last_ack = Instant::now();
    if send_json(&mut socket, hello).await {
        loop {
            if pending && credits > 0 && last_sent.elapsed() >= MIN_FRAME_GAP {
                pending = false;
                let f = frames.borrow_and_update().clone();
                if let Some(f) = f {
                    if socket.send(Message::Binary(f.packed.clone())).await.is_err() {
                        break;
                    }
                    credits -= 1;
                    last_sent = Instant::now();
                }
            }
            let wake = if credits > 0 { last_sent + MIN_FRAME_GAP } else { last_ack + ACK_TIMEOUT };
            tokio::select! {
                r = frames.changed(), if !pending => {
                    if r.is_err() { break; }
                    pending = true;
                }
                r = topics.changed() => {
                    if r.is_err() { break; }
                    let now = *topics.borrow_and_update();
                    if !send_topics(&mut socket, &app, seen, now).await { break; }
                    seen = now;
                }
                m = socket.recv() => match m {
                    Some(Ok(Message::Text(t))) => match handle_text(&app, t.as_str()) {
                        Ok(Inbound::Ack) => {
                            credits = (credits + 1).min(CREDITS);
                            last_ack = Instant::now();
                        }
                        Ok(Inbound::Visible(now)) => {
                            if now != visible {
                                visible = now;
                                app.live.viewer(if now { 1 } else { -1 });
                            }
                        }
                        Ok(Inbound::Reply(v)) => if !send_json(&mut socket, v).await { break; },
                        Ok(Inbound::Nothing) => {}
                        Err(e) => if !send_json(&mut socket, json!({"type": "error", "error": e})).await { break; },
                    },
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                },
                _ = tokio::time::sleep_until(wake.into()), if pending => {
                    if credits <= 0 && last_ack.elapsed() >= ACK_TIMEOUT {
                        credits = 1;
                        last_ack = Instant::now();
                    }
                }
            }
        }
    }
    if visible {
        app.live.viewer(-1);
    }
}
