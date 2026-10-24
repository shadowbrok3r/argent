//! STL: binary (parallel record decode) and ASCII (parallel by facet
//! blocks; several `solid`s become parts; polygon facets are fanned).

use crate::util::{Tokens, find_bytes, parse_f32};
use crate::Mesh;
use rayon::prelude::*;

/// `len == 84 + 50·n` — the reliable binary test (binary headers often
/// start with "solid" too).
pub(crate) fn is_exact_binary(b: &[u8]) -> bool {
    if b.len() < 84 {
        return false;
    }
    let n = u32::from_le_bytes([b[80], b[81], b[82], b[83]]) as u64;
    84 + 50 * n == b.len() as u64
}

/// Binary with trailing bytes (some exporters pad): count fits in the file.
pub(crate) fn is_lenient_binary(b: &[u8]) -> bool {
    if b.len() < 84 + 50 {
        return false;
    }
    let n = u32::from_le_bytes([b[80], b[81], b[82], b[83]]) as u64;
    n > 0 && 84 + 50 * n <= b.len() as u64 && !looks_ascii(b)
}

/// ASCII STL: printable text with `facet`/`vertex` keywords early on.
pub(crate) fn looks_ascii(b: &[u8]) -> bool {
    let head = &b[..b.len().min(4096)];
    let printable = head.iter().all(|&c| c == b'\n' || c == b'\r' || c == b'\t' || (0x20..0x7f).contains(&c) || c >= 0x80);
    printable && (find_bytes(head, b"facet", 0).is_some() || find_bytes(head, b"endsolid", 0).is_some() || head.len() < 4096 && find_bytes(head, b"solid", 0).is_some())
}

pub(crate) fn parse_binary(b: &[u8], warnings: &mut Vec<String>) -> anyhow::Result<Mesh> {
    if b.len() < 84 {
        anyhow::bail!("binary STL shorter than its 84-byte header");
    }
    let declared = u32::from_le_bytes([b[80], b[81], b[82], b[83]]) as usize;
    let available = (b.len() - 84) / 50;
    let n = if declared <= available {
        if 84 + 50 * declared != b.len() {
            warnings.push(format!("binary STL has {} trailing byte(s) after {declared} triangles", b.len() - 84 - 50 * declared));
        }
        declared
    } else {
        warnings.push(format!("binary STL truncated: header says {declared} triangles, file holds {available}"));
        available
    };
    let recs = &b[84..84 + 50 * n];
    let mut positions = vec![[0f32; 3]; 3 * n];
    positions.par_chunks_mut(3).zip(recs.par_chunks_exact(50)).with_min_len(4096).for_each(|(out, rec)| {
        for (k, o) in out.iter_mut().enumerate() {
            for (a, c) in o.iter_mut().enumerate() {
                let off = 12 + 12 * k + 4 * a;
                *c = f32::from_le_bytes([rec[off], rec[off + 1], rec[off + 2], rec[off + 3]]);
            }
        }
    });
    let triangles: Vec<[u32; 3]> =
        (0..n as u32).into_par_iter().with_min_len(8192).map(|t| [3 * t, 3 * t + 1, 3 * t + 2]).collect();
    Ok(Mesh { positions, normals: vec![], triangles, parts: vec![] })
}

struct AsciiChunk {
    pos: Vec<[f32; 3]>,
    /// (triangle index within the chunk, solid name)
    solids: Vec<(u32, String)>,
    bad_numbers: usize,
}

pub(crate) fn parse_ascii(b: &[u8], warnings: &mut Vec<String>) -> anyhow::Result<Mesh> {
    // Split after "endfacet" tokens so no facet straddles two chunks.
    let n = b.len();
    let k = (n / (8 << 20)).clamp(1, 1024);
    let mut bounds = vec![0usize];
    for i in 1..k {
        let from = (n / k * i).max(*bounds.last().unwrap());
        if let Some(p) = find_bytes(b, b"endfacet", from) {
            let e = p + 8;
            if e < n && e > *bounds.last().unwrap() {
                bounds.push(e);
            }
        }
    }
    bounds.push(n);
    let chunks: Vec<AsciiChunk> = bounds.par_windows(2).map(|w| parse_ascii_chunk(&b[w[0]..w[1]])).collect();
    let total: usize = chunks.iter().map(|c| c.pos.len()).sum();
    let mut positions = Vec::with_capacity(total);
    let mut markers: Vec<(u32, String, Option<String>)> = Vec::new();
    let mut bad = 0;
    for c in chunks {
        let base = (positions.len() / 3) as u32;
        for (t, name) in c.solids {
            markers.push((base + t, name, None));
        }
        positions.extend_from_slice(&c.pos);
        bad += c.bad_numbers;
    }
    if bad > 0 {
        warnings.push(format!("ASCII STL: {bad} malformed vertex line(s) skipped"));
    }
    let nt = (positions.len() / 3) as u32;
    let triangles: Vec<[u32; 3]> = (0..nt).into_par_iter().map(|t| [3 * t, 3 * t + 1, 3 * t + 2]).collect();
    // Unnamed / duplicate solid names still separate parts.
    let mut named: Vec<(u32, String, Option<String>)> = Vec::new();
    for (i, (s, name, m)) in markers.into_iter().enumerate() {
        let name = if name.is_empty() { format!("solid{i}") } else { name };
        named.push((s, name, m));
    }
    if named.first().is_none_or(|m| m.0 > 0) {
        named.insert(0, (0, "solid".into(), None));
    }
    let parts = super::parts_from_markers(named, nt);
    Ok(Mesh { positions, normals: vec![], triangles, parts })
}

fn parse_ascii_chunk(c: &[u8]) -> AsciiChunk {
    let mut out = AsciiChunk { pos: Vec::with_capacity(c.len() / 80), solids: Vec::new(), bad_numbers: 0 };
    let mut facet: Vec<[f32; 3]> = Vec::with_capacity(4);
    let mut tok = Tokens::new(c);
    while let Some(t) = tok.next() {
        if t.eq_ignore_ascii_case(b"vertex") {
            let x = tok.next().and_then(parse_f32);
            let y = tok.next().and_then(parse_f32);
            let z = tok.next().and_then(parse_f32);
            match (x, y, z) {
                (Some(x), Some(y), Some(z)) => facet.push([x, y, z]),
                _ => out.bad_numbers += 1,
            }
        } else if t.eq_ignore_ascii_case(b"endloop") || t.eq_ignore_ascii_case(b"endfacet") {
            if facet.len() >= 3 {
                for i in 1..facet.len() - 1 {
                    out.pos.extend_from_slice(&[facet[0], facet[i], facet[i + 1]]);
                }
            }
            facet.clear();
        } else if t.eq_ignore_ascii_case(b"solid") {
            let name = String::from_utf8_lossy(tok.rest_of_line()).trim().to_string();
            out.solids.push(((out.pos.len() / 3) as u32, name));
        } else if t.eq_ignore_ascii_case(b"endsolid") {
            tok.rest_of_line();
        }
    }
    out
}
