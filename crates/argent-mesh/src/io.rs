//! File readers: STL (binary + ASCII), OBJ, PLY (ascii / binary LE / BE)
//! and 3MF, dispatched on magic bytes first and the extension second.
//!
//! Pipeline: parse → `unit_scale` → weld (if `opts.weld`, else only drop
//! unreferenced vertices and collapsed triangles) → crease-aware normals
//! (which also fixes winding). Single-part files get one [`Part`] named after
//! the file stem.

mod obj;
mod ply;
mod stl;
mod threemf;

use crate::util::normalize_parts;
use crate::{LoadOptions, Mesh, Part};
use anyhow::Context;
use rayon::prelude::*;
use std::path::Path;
use std::time::{Duration, Instant};

/// Recognised file formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    StlBinary,
    StlAscii,
    Obj,
    Ply,
    ThreeMf,
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Format::StlBinary => "STL (binary)",
            Format::StlAscii => "STL (ASCII)",
            Format::Obj => "OBJ",
            Format::Ply => "PLY",
            Format::ThreeMf => "3MF",
        })
    }
}

/// Timings and statistics from [`load_with_report`].
#[derive(Clone, Debug)]
pub struct LoadReport {
    pub format: Format,
    pub file_bytes: usize,
    /// Triangles / vertices as parsed (before welding).
    pub raw_triangles: usize,
    pub raw_vertices: usize,
    pub read: Duration,
    pub parse: Duration,
    pub weld: Duration,
    pub normals: Duration,
    pub weld_report: Option<crate::weld::WeldReport>,
    pub normal_report: crate::normals::NormalReport,
    /// Non-fatal problems (bad indices, NaNs, unknown units, …).
    pub warnings: Vec<String>,
}

impl LoadReport {
    pub fn total(&self) -> Duration {
        self.read + self.parse + self.weld + self.normals
    }
}

/// Load, weld and shade one file.
pub fn load(path: &Path, opts: &LoadOptions) -> anyhow::Result<Mesh> {
    load_with_report(path, opts).map(|(m, _)| m)
}

/// [`load`], also returning timings and what the clean-up steps did.
pub fn load_with_report(path: &Path, opts: &LoadOptions) -> anyhow::Result<(Mesh, LoadReport)> {
    let t0 = Instant::now();
    let bytes = std::fs::read(path).with_context(|| format!("argent-mesh: reading {}", path.display()))?;
    let read = t0.elapsed();
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "mesh".into());
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    let (mesh, mut report) =
        load_bytes(&bytes, &stem, ext.as_deref(), opts).with_context(|| format!("argent-mesh: loading {}", path.display()))?;
    report.read = read;
    Ok((mesh, report))
}

/// Load from memory. `name` names a single-part mesh; `ext_hint` (lowercase,
/// no dot) breaks ties when magic bytes are inconclusive.
pub fn load_bytes(
    bytes: &[u8],
    name: &str,
    ext_hint: Option<&str>,
    opts: &LoadOptions,
) -> anyhow::Result<(Mesh, LoadReport)> {
    let format = detect_format(bytes, ext_hint).ok_or_else(|| {
        anyhow::anyhow!("unrecognised mesh format (extension {:?}, {} bytes)", ext_hint.unwrap_or(""), bytes.len())
    })?;
    let t = Instant::now();
    let (mut mesh, mut warnings) = parse(bytes, format)?;
    let parse_time = t.elapsed();
    let raw_triangles = mesh.triangles.len();
    let raw_vertices = mesh.positions.len();
    if mesh.triangles.is_empty() {
        anyhow::bail!("{format} file contains no triangles");
    }
    drop_invalid(&mut mesh, &mut warnings);
    if opts.unit_scale != 1.0 && opts.unit_scale.is_finite() && opts.unit_scale > 0.0 {
        let s = opts.unit_scale;
        mesh.positions.par_iter_mut().for_each(|p| *p = [p[0] * s, p[1] * s, p[2] * s]);
    }
    normalize_parts(&mut mesh, name);
    if mesh.parts.len() == 1 {
        mesh.parts[0].name = name.to_string();
    }
    let t = Instant::now();
    let weld_report = if opts.weld {
        Some(crate::weld::weld(&mut mesh, opts.weld_tolerance))
    } else {
        crate::weld::compact(&mut mesh);
        None
    };
    let weld_time = t.elapsed();
    if mesh.triangles.is_empty() {
        anyhow::bail!("{format} file has only degenerate triangles");
    }
    let t = Instant::now();
    let normal_report = crate::normals::compute(&mut mesh, opts.crease_angle_deg);
    let normals_time = t.elapsed();
    if normal_report.flipped_shells > 0 {
        warnings.push(format!("{} inside-out shell(s) flipped to face outward", normal_report.flipped_shells));
    }
    if normal_report.repaired_faces > 0 {
        warnings.push(format!("{} face(s) with inconsistent winding flipped", normal_report.repaired_faces));
    }
    let report = LoadReport {
        format,
        file_bytes: bytes.len(),
        raw_triangles,
        raw_vertices,
        read: Duration::ZERO,
        parse: parse_time,
        weld: weld_time,
        normals: normals_time,
        weld_report,
        normal_report,
        warnings,
    };
    Ok((mesh, report))
}

/// Identify the format from magic bytes, falling back to the extension.
pub fn detect_format(bytes: &[u8], ext_hint: Option<&str>) -> Option<Format> {
    if bytes.starts_with(b"PK\x03\x04") {
        return Some(Format::ThreeMf);
    }
    if bytes.len() >= 4 && &bytes[..3] == b"ply" && matches!(bytes[3], b'\n' | b'\r' | b' ') {
        return Some(Format::Ply);
    }
    if stl::is_exact_binary(bytes) {
        return Some(Format::StlBinary);
    }
    let text_start = skip_bom_ws(bytes);
    let head = &bytes[text_start..bytes.len().min(text_start + 512)];
    if head.len() >= 5 && head[..5].eq_ignore_ascii_case(b"solid") && stl::looks_ascii(bytes) {
        return Some(Format::StlAscii);
    }
    match ext_hint {
        Some("stl") => {
            return if stl::looks_ascii(bytes) { Some(Format::StlAscii) } else { Some(Format::StlBinary) };
        }
        Some("obj") => return Some(Format::Obj),
        Some("ply") => return Some(Format::Ply),
        Some("3mf") => return Some(Format::ThreeMf),
        _ => {}
    }
    if obj::looks_like_obj(&bytes[text_start..bytes.len().min(text_start + 65536)]) {
        return Some(Format::Obj);
    }
    if bytes.len() > 84 && stl::is_lenient_binary(bytes) {
        return Some(Format::StlBinary);
    }
    None
}

/// Parse without any clean-up (no welding, no normals). Returns the raw
/// mesh (`normals` empty) and warnings.
pub fn parse(bytes: &[u8], format: Format) -> anyhow::Result<(Mesh, Vec<String>)> {
    let mut warnings = Vec::new();
    let mesh = match format {
        Format::StlBinary => stl::parse_binary(bytes, &mut warnings)?,
        Format::StlAscii => stl::parse_ascii(bytes, &mut warnings)?,
        Format::Obj => obj::parse(bytes, &mut warnings)?,
        Format::Ply => ply::parse(bytes, &mut warnings)?,
        Format::ThreeMf => threemf::parse(bytes, &mut warnings)?,
    };
    Ok((mesh, warnings))
}

fn skip_bom_ws(b: &[u8]) -> usize {
    let mut i = if b.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// Remove triangles with out-of-range indices or non-finite corners.
fn drop_invalid(mesh: &mut Mesh, warnings: &mut Vec<String>) {
    let nv = mesh.positions.len();
    let bad_pos: Vec<bool> =
        mesh.positions.par_iter().with_min_len(8192).map(|p| !(p[0].is_finite() && p[1].is_finite() && p[2].is_finite())).collect();
    let any_bad_pos = bad_pos.iter().any(|b| *b);
    let keep: Vec<bool> = mesh
        .triangles
        .par_iter()
        .with_min_len(8192)
        .map(|t| t.iter().all(|&i| (i as usize) < nv && !bad_pos[i as usize]))
        .collect();
    let removed = crate::util::retain_triangles(mesh, &keep);
    if removed > 0 {
        warnings.push(format!(
            "{removed} triangle(s) dropped: {}",
            if any_bad_pos { "non-finite coordinates or invalid indices" } else { "invalid vertex indices" }
        ));
    }
}

/// Build parts from `(start_triangle, name, material)` markers (sorted by
/// start); empty runs are skipped.
pub(crate) fn parts_from_markers(markers: Vec<(u32, String, Option<String>)>, ntri: u32) -> Vec<Part> {
    let mut parts: Vec<Part> = Vec::new();
    for (i, (start, name, material)) in markers.iter().enumerate() {
        let end = markers.get(i + 1).map(|m| m.0).unwrap_or(ntri);
        if end > *start {
            parts.push(Part { name: name.clone(), triangles: *start..end, material: material.clone() });
        }
    }
    parts
}

/// Split `bytes` into roughly `target`-sized pieces, each ending right
/// after a `\n` for which `ok_split(prefix_before_newline)` holds.
pub(crate) fn split_at_lines(bytes: &[u8], target: usize, ok_split: impl Fn(&[u8], usize) -> bool) -> Vec<&[u8]> {
    let n = bytes.len();
    let k = (n / target.max(1)).clamp(1, 4096);
    let mut out = Vec::with_capacity(k);
    let mut start = 0;
    for i in 1..k {
        let mut p = (n / k * i).max(start);
        let mut found = None;
        while p < n {
            match bytes[p..].iter().position(|&b| b == b'\n') {
                None => break,
                Some(off) => {
                    let q = p + off;
                    if ok_split(bytes, q) {
                        found = Some(q + 1);
                        break;
                    }
                    p = q + 1;
                }
            }
        }
        match found {
            Some(e) if e > start && e < n => {
                out.push(&bytes[start..e]);
                start = e;
            }
            _ => {}
        }
    }
    out.push(&bytes[start..]);
    out
}

#[cfg(test)]
mod tests;
