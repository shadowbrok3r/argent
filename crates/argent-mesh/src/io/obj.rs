//! Wavefront OBJ: `v` and `f` (v, v/vt, v//vn, v/vt/vn; negative indices;
//! n-gons fanned). `o`/`g`/`usemtl` start new parts. Texture coordinates and
//! file normals are ignored (normals are recomputed). Parsed in parallel
//! line-aligned chunks; relative indices are resolved after the per-chunk
//! vertex counts are known.

use crate::util::{Tokens, parse_f32, parse_i64};
use crate::Mesh;
use rayon::prelude::*;

pub(crate) fn looks_like_obj(head: &[u8]) -> bool {
    let (mut v, mut f, mut known, mut unknown) = (0, 0, 0, 0);
    for line in head.split(|&b| b == b'\n').take(2000) {
        let mut t = Tokens::new(line);
        match t.next() {
            Some(b"v") => v += 1,
            Some(b"f") => f += 1,
            Some(
                b"vn" | b"vt" | b"vp" | b"o" | b"g" | b"usemtl" | b"mtllib" | b"s" | b"l" | b"p" | b"mg" | b"usemap"
                | b"maplib" | b"lod" | b"cstype" | b"deg" | b"curv" | b"curv2" | b"surf" | b"parm" | b"end",
            ) => known += 1,
            Some(x) if x.starts_with(b"#") => known += 1,
            None => {}
            Some(_) => unknown += 1,
        }
    }
    let good = v + f + known;
    (v > 0 || f > 0) && unknown * 10 <= good
}

enum Event {
    Object(String),
    Group(String),
    Material(String),
}

/// A face corner: absolute 0-based index, or relative to the chunk's
/// first vertex (negative OBJ index resolved against the local count).
#[derive(Clone, Copy)]
enum Corner {
    Abs(u32),
    Rel(i64),
}

#[derive(Default)]
struct Chunk {
    verts: Vec<[f32; 3]>,
    tris: Vec<[u32; 3]>,
    fixups: Vec<(u32, [Corner; 3])>,
    events: Vec<(u32, Event)>,
    bad_faces: usize,
    bad_vertices: usize,
}

const BAD: u32 = u32::MAX;

pub(crate) fn parse(bytes: &[u8], warnings: &mut Vec<String>) -> anyhow::Result<Mesh> {
    let chunks = super::split_at_lines(bytes, 4 << 20, |b, q| {
        // Never split inside a backslash line continuation.
        let mut e = q;
        if e > 0 && b[e - 1] == b'\r' {
            e -= 1;
        }
        !(e > 0 && b[e - 1] == b'\\')
    });
    let parsed: Vec<Chunk> = chunks.par_iter().map(|c| parse_chunk(c)).collect();

    let mut vbase = Vec::with_capacity(parsed.len());
    let mut tbase = Vec::with_capacity(parsed.len());
    let (mut nv, mut nt) = (0usize, 0usize);
    for c in &parsed {
        vbase.push(nv);
        tbase.push(nt);
        nv += c.verts.len();
        nt += c.tris.len();
    }
    if nv >= u32::MAX as usize || nt >= u32::MAX as usize {
        anyhow::bail!("OBJ too large for 32-bit indices");
    }
    let mut positions = Vec::with_capacity(nv);
    let mut triangles: Vec<[u32; 3]> = Vec::with_capacity(nt);
    let mut markers: Vec<(u32, String, Option<String>)> = Vec::new();
    let (mut object, mut group, mut material): (Option<String>, Option<String>, Option<String>) = (None, None, None);
    let mut current: Option<(String, Option<String>)> = None;
    let (mut bad_faces, mut bad_vertices) = (0, 0);
    for (ci, mut c) in parsed.into_iter().enumerate() {
        let vb = vbase[ci] as i64;
        for (t, corners) in c.fixups.drain(..) {
            let mut out = [0u32; 3];
            for (k, corner) in corners.iter().enumerate() {
                out[k] = match *corner {
                    Corner::Abs(i) => i,
                    Corner::Rel(r) => {
                        let g = vb + r;
                        if g >= 0 && (g as usize) < nv { g as u32 } else { BAD }
                    }
                };
            }
            c.tris[t as usize] = out;
        }
        let tb = tbase[ci] as u32;
        for (t, ev) in c.events {
            match ev {
                Event::Object(o) => {
                    object = Some(o);
                    group = None;
                }
                Event::Group(g) => group = Some(g),
                Event::Material(m) => material = Some(m),
            }
            let name = match (&object, &group) {
                (Some(o), Some(g)) if o != g => format!("{o}/{g}"),
                (Some(o), _) => o.clone(),
                (None, Some(g)) => g.clone(),
                (None, None) => "default".to_string(),
            };
            let key = (name, material.clone());
            if current.as_ref() != Some(&key) {
                let start = tb + t;
                if let Some(last) = markers.last_mut().filter(|m| m.0 == start) {
                    last.1 = key.0.clone();
                    last.2 = key.1.clone();
                } else {
                    markers.push((start, key.0.clone(), key.1.clone()));
                }
                current = Some(key);
            }
        }
        positions.extend_from_slice(&c.verts);
        triangles.extend_from_slice(&c.tris);
        bad_faces += c.bad_faces;
        bad_vertices += c.bad_vertices;
    }
    if bad_vertices > 0 {
        warnings.push(format!("OBJ: {bad_vertices} malformed vertex line(s) skipped"));
    }
    if bad_faces > 0 {
        warnings.push(format!("OBJ: {bad_faces} malformed face(s) skipped"));
    }
    let nt32 = triangles.len() as u32;
    if markers.first().is_none_or(|m| m.0 > 0) {
        markers.insert(0, (0, "default".into(), None));
    }
    let parts = super::parts_from_markers(markers, nt32);
    // Out-of-range indices (BAD or ≥ nv) are dropped later by the loader.
    Ok(Mesh { positions, normals: vec![], triangles, parts })
}

fn parse_chunk(c: &[u8]) -> Chunk {
    let mut out = Chunk { verts: Vec::with_capacity(c.len() / 64), tris: Vec::with_capacity(c.len() / 40), ..Default::default() };
    let mut corners: Vec<Corner> = Vec::with_capacity(8);
    let mut joined: Vec<u8> = Vec::new();
    let mut lines = c.split(|&b| b == b'\n');
    while let Some(mut line) = lines.next() {
        // Backslash continuation: join with following lines.
        let trimmed_end = |l: &[u8]| {
            let mut e = l.len();
            while e > 0 && (l[e - 1] == b'\r' || l[e - 1] == b' ' || l[e - 1] == b'\t') {
                e -= 1;
            }
            e
        };
        let e = trimmed_end(line);
        if e > 0 && line[e - 1] == b'\\' {
            joined.clear();
            joined.extend_from_slice(&line[..e - 1]);
            joined.push(b' ');
            for next in lines.by_ref() {
                let e2 = trimmed_end(next);
                if e2 > 0 && next[e2 - 1] == b'\\' {
                    joined.extend_from_slice(&next[..e2 - 1]);
                    joined.push(b' ');
                } else {
                    joined.extend_from_slice(next);
                    break;
                }
            }
            let owned = std::mem::take(&mut joined);
            handle_line(&owned, &mut out, &mut corners);
            joined = owned;
            continue;
        }
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        handle_line(line, &mut out, &mut corners);
    }
    out
}

#[inline]
fn handle_line(line: &[u8], out: &mut Chunk, corners: &mut Vec<Corner>) {
    let mut t = Tokens::new(line);
    let Some(kw) = t.next() else { return };
    match kw {
        b"v" => {
            let x = t.next().and_then(parse_f32);
            let y = t.next().and_then(parse_f32);
            let z = t.next().and_then(parse_f32);
            match (x, y, z) {
                (Some(x), Some(y), Some(z)) => out.verts.push([x, y, z]),
                _ => {
                    // Keep numbering intact: a broken vertex still occupies its index.
                    out.verts.push([f32::NAN; 3]);
                    out.bad_vertices += 1;
                }
            }
        }
        b"f" | b"fo" => {
            corners.clear();
            let local = out.verts.len() as i64;
            let mut ok = true;
            for tok in t {
                let idx = tok.split(|&b| b == b'/').next().unwrap_or(tok);
                match parse_i64(idx) {
                    Some(i) if i > 0 && i <= u32::MAX as i64 => corners.push(Corner::Abs((i - 1) as u32)),
                    Some(i) if i < 0 => corners.push(Corner::Rel(local + i)),
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok || corners.len() < 3 {
                out.bad_faces += 1;
                return;
            }
            let rel = corners.iter().any(|c| matches!(c, Corner::Rel(_)));
            for i in 1..corners.len() - 1 {
                let tri = [corners[0], corners[i], corners[i + 1]];
                if rel {
                    out.fixups.push((out.tris.len() as u32, tri));
                    out.tris.push([BAD; 3]);
                } else {
                    let a = |c: Corner| if let Corner::Abs(v) = c { v } else { BAD };
                    out.tris.push([a(tri[0]), a(tri[1]), a(tri[2])]);
                }
            }
        }
        b"o" => out.events.push((out.tris.len() as u32, Event::Object(rest_name(line, 1)))),
        b"g" => out.events.push((out.tris.len() as u32, Event::Group(rest_name(line, 1)))),
        b"usemtl" => out.events.push((out.tris.len() as u32, Event::Material(rest_name(line, 6)))),
        _ => {}
    }
}

/// Text after the keyword (`kw_len` bytes after leading whitespace), trimmed.
fn rest_name(line: &[u8], kw_len: usize) -> String {
    let start = line.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(0) + kw_len;
    String::from_utf8_lossy(line.get(start..).unwrap_or(&[])).trim().to_string()
}
