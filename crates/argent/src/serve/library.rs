//! The studio's library: meshes under the `--library` roots, and the only
//! paths the studio may open (canonicalised, prefix-checked).

use anyhow::{bail, Result};
use serde::Serialize;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub struct Library {
    /// Canonical roots.
    pub roots: Vec<PathBuf>,
    /// Offer the procedural demo ring.
    pub demo: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    /// Stable short id (hash of the canonical path).
    pub id: String,
    /// Display name ("Arachne the weaver · US 7").
    pub name: String,
    /// Canonical path of the main mesh: what `POST /api/open {path}` takes.
    pub path: String,
    /// Path relative to its library root.
    pub rel: String,
    /// First directory under the root (UI grouping).
    pub group: String,
    /// Bytes of the mesh plus its companions.
    pub bytes: u64,
    /// Triangle count when cheap to know (binary STL).
    pub triangles: Option<u64>,
    /// A RingDesigner `_stones.obj` comes along.
    pub stones: bool,
    /// Modification time, unix seconds.
    pub modified: u64,
}

/// Lighting environments (equirectangular Radiance HDR).
pub const HDRI_EXT: &[&str] = &["hdr"];
/// Reel soundtracks.
pub const AUDIO_EXT: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus"];

/// A non-mesh library file (HDRI, soundtrack).
#[derive(Clone, Debug, Serialize)]
pub struct Asset {
    pub id: String,
    pub name: String,
    pub path: String,
    pub rel: String,
    pub bytes: u64,
}

fn has_ext(p: &Path, exts: &[&str]) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Files with one of `exts` under `dir` (skips hidden and `target*` dirs).
fn walk(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if !name.starts_with('.') && !name.starts_with("target") {
                    stack.push(p);
                }
            } else if has_ext(&p, exts) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

pub fn is_mesh(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("stl" | "obj" | "ply" | "3mf")
    )
}

/// FNV-1a, hex: short stable ids for paths.
pub fn short_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:010x}", h & 0xff_ffff_ffff)
}

/// "arachne-the-weaver_size7" → "Arachne the weaver · US 7".
pub fn pretty_name(stem: &str) -> String {
    let (base, size) = match stem.rsplit_once("_size") {
        Some((b, s)) if !b.is_empty() && s.chars().next().is_some_and(|c| c.is_ascii_digit()) => (b, Some(s)),
        _ => (stem, None),
    };
    let words: Vec<String> = base.split(['-', '_', ' ']).filter(|w| !w.is_empty()).map(str::to_string).collect();
    let mut name = words.join(" ");
    if let Some(first) = name.get(..1) {
        name = first.to_uppercase() + &name[1..];
    }
    match size {
        Some(s) if s.ends_with("mm") => format!("{name} · {s} bore"),
        Some(s) => format!("{name} · US {s}"),
        None => name,
    }
}

/// Triangle count of a binary STL from its header, if it is one.
fn stl_triangles(path: &Path, len: u64) -> Option<u64> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 84];
    f.read_exact(&mut head).ok()?;
    let n = u32::from_le_bytes([head[80], head[81], head[82], head[83]]) as u64;
    (84 + 50 * n == len).then_some(n)
}

fn companion(p: &Path) -> Option<PathBuf> {
    let stem = p.file_stem()?.to_string_lossy();
    let c = p.parent()?.join(format!("{stem}_stones.obj"));
    c.is_file().then_some(c)
}

impl Library {
    pub fn new(dirs: &[PathBuf]) -> Library {
        let mut roots = vec![];
        for d in dirs {
            match std::fs::canonicalize(d) {
                Ok(c) if c.is_dir() => {
                    if !roots.contains(&c) {
                        roots.push(c);
                    }
                }
                _ => eprintln!("argent serve: library {} is not a directory, skipped", d.display()),
            }
        }
        let mut lib = Library { roots, demo: false };
        lib.demo = std::env::var("ARGENT_STUDIO_DEMO").is_ok_and(|v| v != "0") || lib.scan().is_empty();
        lib
    }

    /// Every piece under the roots. A RingDesigner `<stem>_stones.obj` next
    /// to `<stem>.stl` rides along with the STL instead of being listed.
    pub fn scan(&self) -> Vec<Entry> {
        let mut out = vec![];
        for root in &self.roots {
            let files = crate::jobs::find_meshes(root);
            for f in &files {
                let name = f.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                if let Some(tag) = name.strip_suffix("_stones.obj") {
                    let dir = f.parent().unwrap_or(root);
                    let has_metal = files.iter().any(|g| {
                        g.parent() == Some(dir)
                            && g.file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(tag))
                            && g.extension().is_some_and(|e| e.eq_ignore_ascii_case("stl"))
                    });
                    if has_metal {
                        continue;
                    }
                }
                let Ok(canon) = std::fs::canonicalize(f) else { continue };
                if !canon.starts_with(root) {
                    continue; // a symlink pointing out of the library
                }
                let meta = std::fs::metadata(&canon).ok();
                let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let comp = companion(&canon);
                let comp_len = comp.as_ref().and_then(|c| std::fs::metadata(c).ok()).map(|m| m.len()).unwrap_or(0);
                let rel = canon.strip_prefix(root).unwrap_or(&canon).to_path_buf();
                let group = match rel.components().count() {
                    0 | 1 => root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    _ => rel.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default(),
                };
                let stem = canon.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let is_stl = canon.extension().is_some_and(|e| e.eq_ignore_ascii_case("stl"));
                let path = canon.to_string_lossy().into_owned();
                out.push(Entry {
                    id: short_hash(&path),
                    name: pretty_name(&stem),
                    rel: rel.to_string_lossy().into_owned(),
                    group,
                    bytes: len + comp_len,
                    triangles: if is_stl { stl_triangles(&canon, len) } else { None },
                    stones: comp.is_some(),
                    modified: meta
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                    path,
                });
            }
        }
        out.sort_by(|a, b| a.group.cmp(&b.group).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        out
    }

    /// Resolve a client-supplied path (absolute, or relative to a root) to a
    /// canonical mesh file inside the library. Nothing else can be opened.
    pub fn resolve(&self, p: &str) -> Result<PathBuf> {
        let p = p.trim();
        if p.is_empty() {
            bail!("empty path");
        }
        let path = Path::new(p);
        let candidates: Vec<PathBuf> =
            if path.is_absolute() { vec![path.to_path_buf()] } else { self.roots.iter().map(|r| r.join(path)).collect() };
        for c in candidates {
            if let Ok(canon) = std::fs::canonicalize(&c)
                && self.roots.iter().any(|r| canon.starts_with(r))
                && canon.is_file()
                && is_mesh(&canon)
            {
                return Ok(canon);
            }
        }
        bail!("{p} is not a mesh inside the studio library ({})", self.roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", "))
    }

    /// HDRIs or soundtracks under the roots.
    pub fn assets(&self, exts: &[&str]) -> Vec<Asset> {
        let mut out = vec![];
        for root in &self.roots {
            for f in walk(root, exts) {
                let Ok(canon) = std::fs::canonicalize(&f) else { continue };
                if !canon.starts_with(root) {
                    continue;
                }
                let path = canon.to_string_lossy().into_owned();
                let stem = canon.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                out.push(Asset {
                    id: short_hash(&path),
                    name: pretty_name(&stem.replace("_2k", "").replace("_4k", "").replace("_1k", "")),
                    rel: canon.strip_prefix(root).unwrap_or(&canon).to_string_lossy().into_owned(),
                    bytes: std::fs::metadata(&canon).map(|m| m.len()).unwrap_or(0),
                    path,
                });
            }
        }
        out
    }

    /// Resolve an HDRI/soundtrack path or id to a canonical file inside the
    /// library with one of `exts`.
    pub fn resolve_asset(&self, p: &str, exts: &[&str]) -> Result<PathBuf> {
        let p = p.trim();
        if p.len() == 10 && p.bytes().all(|b| b.is_ascii_hexdigit()) {
            if let Some(a) = self.assets(exts).into_iter().find(|a| a.id == p) {
                return Ok(PathBuf::from(a.path));
            }
        }
        let path = Path::new(p);
        let candidates: Vec<PathBuf> =
            if path.is_absolute() { vec![path.to_path_buf()] } else { self.roots.iter().map(|r| r.join(path)).collect() };
        for c in candidates {
            if let Ok(canon) = std::fs::canonicalize(&c)
                && self.roots.iter().any(|r| canon.starts_with(r))
                && canon.is_file()
                && has_ext(&canon, exts)
            {
                return Ok(canon);
            }
        }
        bail!("{p} is not a {} file inside the studio library", exts.join("/"))
    }

    /// Resolve a library id from [`Library::scan`].
    pub fn resolve_id(&self, id: &str) -> Result<PathBuf> {
        match self.scan().into_iter().find(|e| e.id == id) {
            Some(e) => self.resolve(&e.path),
            None => bail!("no library piece with id {id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(pretty_name("arachne-the-weaver_size7"), "Arachne the weaver · US 7");
        assert_eq!(pretty_name("vesper-celestial-reliquary_size9.34"), "Vesper celestial reliquary · US 9.34");
        assert_eq!(pretty_name("thanatos"), "Thanatos");
    }

    #[test]
    fn resolve_stays_inside() {
        let tmp = std::env::temp_dir().join(format!("argent-lib-test-{}", std::process::id()));
        let inside = tmp.join("lib");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(inside.join("a.stl"), b"solid a\nendsolid a\n").unwrap();
        std::fs::write(tmp.join("outside.stl"), b"solid b\nendsolid b\n").unwrap();
        std::fs::write(inside.join("notes.txt"), b"x").unwrap();
        let lib = Library { roots: vec![std::fs::canonicalize(&inside).unwrap()], demo: false };
        assert!(lib.resolve("a.stl").is_ok());
        assert!(lib.resolve(inside.join("a.stl").to_str().unwrap()).is_ok());
        assert!(lib.resolve("../outside.stl").is_err());
        assert!(lib.resolve(tmp.join("outside.stl").to_str().unwrap()).is_err());
        assert!(lib.resolve("notes.txt").is_err());
        assert!(lib.resolve("/etc/passwd").is_err());
        std::fs::remove_dir_all(&tmp).ok();
    }
}
