//! PLY: ascii, binary_little_endian, binary_big_endian. Vertex `x/y/z`
//! (any scalar type; other properties skipped), faces from the
//! `vertex_indices` / `vertex_index` list (any count/index types), other
//! elements skipped. Polygons are fanned.

use crate::util::{Tokens, find_bytes, parse_f64};
use crate::Mesh;
use anyhow::{Context, bail};
use rayon::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn parse(s: &str) -> Option<Scalar> {
        Some(match s {
            "char" | "int8" => Scalar::I8,
            "uchar" | "uint8" => Scalar::U8,
            "short" | "int16" => Scalar::I16,
            "ushort" | "uint16" => Scalar::U16,
            "int" | "int32" => Scalar::I32,
            "uint" | "uint32" => Scalar::U32,
            "float" | "float32" => Scalar::F32,
            "double" | "float64" => Scalar::F64,
            _ => return None,
        })
    }
    fn size(self) -> usize {
        match self {
            Scalar::I8 | Scalar::U8 => 1,
            Scalar::I16 | Scalar::U16 => 2,
            Scalar::I32 | Scalar::U32 | Scalar::F32 => 4,
            Scalar::F64 => 8,
        }
    }
}

#[derive(Clone, Debug)]
enum Kind {
    Scalar(Scalar),
    List(Scalar, Scalar),
}

#[derive(Clone, Debug)]
struct Prop {
    name: String,
    kind: Kind,
}

#[derive(Clone, Debug)]
struct Element {
    name: String,
    count: usize,
    props: Vec<Prop>,
}

#[derive(Clone, Copy, PartialEq)]
enum Encoding {
    Ascii,
    Le,
    Be,
}

#[inline]
fn read_scalar(b: &[u8], at: usize, s: Scalar, enc: Encoding) -> Option<f64> {
    let bytes = b.get(at..at + s.size())?;
    macro_rules! num {
        ($t:ty, $n:expr) => {{
            let mut a = [0u8; $n];
            a.copy_from_slice(bytes);
            if enc == Encoding::Be { <$t>::from_be_bytes(a) } else { <$t>::from_le_bytes(a) }
        }};
    }
    Some(match s {
        Scalar::I8 => bytes[0] as i8 as f64,
        Scalar::U8 => bytes[0] as f64,
        Scalar::I16 => num!(i16, 2) as f64,
        Scalar::U16 => num!(u16, 2) as f64,
        Scalar::I32 => num!(i32, 4) as f64,
        Scalar::U32 => num!(u32, 4) as f64,
        Scalar::F32 => num!(f32, 4) as f64,
        Scalar::F64 => num!(f64, 8),
    })
}

pub(crate) fn parse(b: &[u8], warnings: &mut Vec<String>) -> anyhow::Result<Mesh> {
    let he = find_bytes(b, b"end_header", 0).context("PLY: no end_header")?;
    let mut body = he + "end_header".len();
    // Header ends at the newline after end_header (\n or \r\n).
    while body < b.len() && (b[body] == b' ' || b[body] == b'\t' || b[body] == b'\r') {
        body += 1;
    }
    if body < b.len() && b[body] == b'\n' {
        body += 1;
    }
    let header = std::str::from_utf8(&b[..he]).context("PLY header is not text")?;
    let mut enc = None;
    let mut elements: Vec<Element> = Vec::new();
    for line in header.lines() {
        let mut w = line.split_ascii_whitespace();
        match w.next() {
            Some("format") => {
                enc = Some(match w.next() {
                    Some("ascii") => Encoding::Ascii,
                    Some("binary_little_endian") => Encoding::Le,
                    Some("binary_big_endian") => Encoding::Be,
                    other => bail!("PLY: unknown format {other:?}"),
                })
            }
            Some("element") => {
                let name = w.next().unwrap_or("").to_string();
                let count: usize = w.next().and_then(|c| c.parse().ok()).context("PLY: bad element count")?;
                elements.push(Element { name, count, props: vec![] });
            }
            Some("property") => {
                let el = elements.last_mut().context("PLY: property before element")?;
                let t = w.next().unwrap_or("");
                let kind = if t == "list" {
                    let c = w.next().and_then(Scalar::parse).context("PLY: bad list count type")?;
                    let i = w.next().and_then(Scalar::parse).context("PLY: bad list item type")?;
                    Kind::List(c, i)
                } else {
                    Kind::Scalar(Scalar::parse(t).with_context(|| format!("PLY: unknown property type {t}"))?)
                };
                let name = w.next().unwrap_or("").to_string();
                el.props.push(Prop { name, kind });
            }
            _ => {}
        }
    }
    let enc = enc.context("PLY: missing format line")?;
    let data = &b[body..];
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let mut bad_faces = 0usize;
    match enc {
        Encoding::Ascii => parse_ascii(data, &elements, &mut positions, &mut triangles, &mut bad_faces)?,
        _ => parse_binary(data, &elements, enc, &mut positions, &mut triangles, &mut bad_faces)?,
    }
    if bad_faces > 0 {
        warnings.push(format!("PLY: {bad_faces} face(s) with fewer than 3 or invalid indices skipped"));
    }
    if positions.is_empty() {
        bail!("PLY: no vertex element with x/y/z");
    }
    Ok(Mesh { positions, normals: vec![], triangles, parts: vec![] })
}

fn xyz_index(el: &Element) -> Option<[usize; 3]> {
    let f = |n: &str| el.props.iter().position(|p| p.name == n && matches!(p.kind, Kind::Scalar(_)));
    Some([f("x")?, f("y")?, f("z")?])
}

fn face_index(el: &Element) -> Option<usize> {
    el.props.iter().position(|p| (p.name == "vertex_indices" || p.name == "vertex_index") && matches!(p.kind, Kind::List(..)))
}

/// Out-of-range indices are left for the loader's validity pass (the face
/// element may legally precede the vertex element).
fn push_polygon(idx: &[i64], _nv: usize, triangles: &mut Vec<[u32; 3]>, bad: &mut usize) {
    if idx.len() < 3 || idx.iter().any(|&i| i < 0 || i >= u32::MAX as i64) {
        *bad += 1;
        return;
    }
    for i in 1..idx.len() - 1 {
        triangles.push([idx[0] as u32, idx[i] as u32, idx[i + 1] as u32]);
    }
}

fn parse_binary(
    d: &[u8],
    elements: &[Element],
    enc: Encoding,
    positions: &mut Vec<[f32; 3]>,
    triangles: &mut Vec<[u32; 3]>,
    bad: &mut usize,
) -> anyhow::Result<()> {
    let mut at = 0usize;
    let truncated = || anyhow::anyhow!("PLY: binary body truncated");
    for el in elements {
        let fixed: Option<usize> = el
            .props
            .iter()
            .map(|p| match p.kind {
                Kind::Scalar(s) => Some(s.size()),
                Kind::List(..) => None,
            })
            .sum();
        let is_vertex = el.name == "vertex" && positions.is_empty();
        let is_face = el.name == "face" && face_index(el).is_some();
        if is_vertex && let (Some(stride), Some(xyz)) = (fixed, xyz_index(el)) {
            // Fixed-size records: decode in parallel.
            let end = at.checked_add(stride * el.count).filter(|&e| e <= d.len()).ok_or_else(truncated)?;
            let mut offs = [0usize; 3];
            let mut types = [Scalar::F32; 3];
            for k in 0..3 {
                let mut o = 0;
                for p in &el.props[..xyz[k]] {
                    if let Kind::Scalar(s) = p.kind {
                        o += s.size();
                    }
                }
                offs[k] = o;
                if let Kind::Scalar(s) = el.props[xyz[k]].kind {
                    types[k] = s;
                }
            }
            let recs = &d[at..end];
            *positions = recs
                .par_chunks_exact(stride)
                .with_min_len(8192)
                .map(|r| {
                    let mut p = [0f32; 3];
                    for k in 0..3 {
                        p[k] = read_scalar(r, offs[k], types[k], enc).unwrap_or(f64::NAN) as f32;
                    }
                    p
                })
                .collect();
            at = end;
            continue;
        }
        if let Some(stride) = fixed
            && !is_vertex
            && !is_face
        {
            at = at.checked_add(stride * el.count).filter(|&e| e <= d.len()).ok_or_else(truncated)?;
            continue;
        }
        // Variable-size records: walk them.
        let xyz = if is_vertex { xyz_index(el) } else { None };
        let fidx = if is_face { face_index(el) } else { None };
        if is_vertex {
            positions.reserve(el.count);
        }
        if is_face {
            triangles.reserve(el.count);
        }
        let mut poly: Vec<i64> = Vec::with_capacity(8);
        let nv = positions.len();
        let mut p = [0f32; 3];
        for _ in 0..el.count {
            for (pi, prop) in el.props.iter().enumerate() {
                match prop.kind {
                    Kind::Scalar(s) => {
                        if let Some(xyz) = xyz {
                            for k in 0..3 {
                                if xyz[k] == pi {
                                    p[k] = read_scalar(d, at, s, enc).ok_or_else(truncated)? as f32;
                                }
                            }
                        }
                        at += s.size();
                    }
                    Kind::List(cs, is) => {
                        let n = read_scalar(d, at, cs, enc).ok_or_else(truncated)?;
                        at += cs.size();
                        let n = if n.is_finite() && n >= 0.0 { n as usize } else { 0 };
                        if fidx == Some(pi) {
                            poly.clear();
                            for _ in 0..n {
                                poly.push(read_scalar(d, at, is, enc).ok_or_else(truncated)? as i64);
                                at += is.size();
                            }
                        } else {
                            at = at.checked_add(n * is.size()).ok_or_else(truncated)?;
                        }
                    }
                }
            }
            if xyz.is_some() {
                positions.push(p);
            }
            if fidx.is_some() {
                push_polygon(&poly, nv, triangles, bad);
            }
        }
        if at > d.len() {
            return Err(truncated());
        }
    }
    Ok(())
}

fn parse_ascii(
    d: &[u8],
    elements: &[Element],
    positions: &mut Vec<[f32; 3]>,
    triangles: &mut Vec<[u32; 3]>,
    bad: &mut usize,
) -> anyhow::Result<()> {
    let mut tok = Tokens::new(d);
    let mut next = || tok.next().and_then(parse_f64).ok_or_else(|| anyhow::anyhow!("PLY: ascii body truncated or malformed"));
    let mut poly: Vec<i64> = Vec::with_capacity(8);
    for el in elements {
        let is_vertex = el.name == "vertex" && positions.is_empty();
        let xyz = if is_vertex { xyz_index(el) } else { None };
        let fidx = if el.name == "face" { face_index(el) } else { None };
        let nv = positions.len();
        for _ in 0..el.count {
            let mut p = [0f32; 3];
            for (pi, prop) in el.props.iter().enumerate() {
                match prop.kind {
                    Kind::Scalar(_) => {
                        let v = next()?;
                        if let Some(xyz) = xyz {
                            for k in 0..3 {
                                if xyz[k] == pi {
                                    p[k] = v as f32;
                                }
                            }
                        }
                    }
                    Kind::List(..) => {
                        let n = next()?;
                        let n = if n.is_finite() && n >= 0.0 { n as usize } else { 0 };
                        if fidx == Some(pi) {
                            poly.clear();
                            for _ in 0..n {
                                poly.push(next()? as i64);
                            }
                        } else {
                            for _ in 0..n {
                                next()?;
                            }
                        }
                    }
                }
            }
            if xyz.is_some() {
                positions.push(p);
            }
            if fidx.is_some() {
                push_polygon(&poly, nv, triangles, bad);
            }
        }
    }
    Ok(())
}
