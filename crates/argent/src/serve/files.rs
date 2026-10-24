//! `GET /files/...`: renders under `--out`, nothing else. Single byte
//! ranges are honoured (mobile Safari will not play an mp4 without them).

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

pub fn content_type(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("json") => "application/json",
        Some("html") => "text/html; charset=utf-8",
        Some("exr") => "image/x-exr",
        Some("txt" | "log") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// `rel` (URL path under `/files/`) → canonical file inside `root`.
pub fn resolve(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = Path::new(rel);
    if rel.as_os_str().is_empty() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    let canon = std::fs::canonicalize(root.join(rel)).ok()?;
    (canon.starts_with(root) && canon.is_file()).then_some(canon)
}

/// Parse a single `bytes=` range against a length; `Err` = unsatisfiable.
fn parse_range(h: &str, len: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = h.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None; // multi-range: serve the whole file
    }
    let (a, b) = spec.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    let r = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        if n == 0 || len == 0 {
            return Some(Err(()));
        }
        (len.saturating_sub(n), len - 1)
    } else {
        let s: u64 = a.parse().ok()?;
        let e: u64 = if b.is_empty() { len.saturating_sub(1) } else { b.parse::<u64>().ok()?.min(len.saturating_sub(1)) };
        if s >= len || e < s {
            return Some(Err(()));
        }
        (s, e)
    };
    Some(Ok(r))
}

pub async fn serve(root: PathBuf, rel: String, headers: HeaderMap, download: bool) -> Response {
    let Some(path) = resolve(&root, &rel) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).map(str::to_string);
    let res = tokio::task::spawn_blocking(move || -> std::io::Result<(u64, Option<Result<(u64, u64), ()>>, Vec<u8>, PathBuf)> {
        let mut f = std::fs::File::open(&path)?;
        let len = f.metadata()?.len();
        let r = range.as_deref().and_then(|h| parse_range(h, len));
        let body = match r {
            Some(Ok((s, e))) => {
                f.seek(SeekFrom::Start(s))?;
                let mut buf = vec![0u8; (e - s + 1) as usize];
                f.read_exact(&mut buf)?;
                buf
            }
            Some(Err(())) => vec![],
            None => {
                let mut buf = Vec::with_capacity(len as usize);
                f.read_to_end(&mut buf)?;
                buf
            }
        };
        Ok((len, r, body, path))
    })
    .await;
    let (len, r, body, path) = match res {
        Ok(Ok(v)) => v,
        _ => return (StatusCode::NOT_FOUND, "not found").into_response(),
    };
    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(&path)));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600"));
    if download {
        let name = path.file_name().map(|n| n.to_string_lossy().replace(['"', '\\', '\r', '\n'], "_")).unwrap_or_default();
        if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
            h.insert(header::CONTENT_DISPOSITION, v);
        }
    }
    match r {
        Some(Ok((s, e))) => {
            if let Ok(v) = HeaderValue::from_str(&format!("bytes {s}-{e}/{len}")) {
                h.insert(header::CONTENT_RANGE, v);
            }
            (StatusCode::PARTIAL_CONTENT, h, Body::from(body)).into_response()
        }
        Some(Err(())) => {
            if let Ok(v) = HeaderValue::from_str(&format!("bytes */{len}")) {
                h.insert(header::CONTENT_RANGE, v);
            }
            (StatusCode::RANGE_NOT_SATISFIABLE, h).into_response()
        }
        None => (StatusCode::OK, h, Body::from(body)).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-1", 10), Some(Ok((0, 1))));
        assert_eq!(parse_range("bytes=5-", 10), Some(Ok((5, 9))));
        assert_eq!(parse_range("bytes=-3", 10), Some(Ok((7, 9))));
        assert_eq!(parse_range("bytes=0-99", 10), Some(Ok((0, 9))));
        assert_eq!(parse_range("bytes=10-", 10), Some(Err(())));
        assert_eq!(parse_range("bytes=0-1,4-5", 10), None);
    }

    #[test]
    fn no_escape() {
        let root = std::env::temp_dir().join(format!("argent-files-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("stills")).unwrap();
        std::fs::write(root.join("stills/a.png"), b"x").unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        assert!(resolve(&root, "stills/a.png").is_some());
        assert!(resolve(&root, "../etc/passwd").is_none());
        assert!(resolve(&root, "/etc/passwd").is_none());
        assert!(resolve(&root, "stills/../stills/a.png").is_none());
        assert!(resolve(&root, "").is_none());
        std::fs::remove_dir_all(&root).ok();
    }
}
