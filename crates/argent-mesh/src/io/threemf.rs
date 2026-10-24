//! 3MF: a zip (OPC) package. The root model comes from `_rels/.rels`
//! (fallback `3D/3dmodel.model`). `<object>` meshes become parts named from
//! their `name` attribute; `<components>` are expanded recursively,
//! including production-extension `p:path` references into other model
//! files; `<build><item transform=…>` places them; the model `unit` is
//! converted to millimetres; base-material names become `Part::material`
//! (per-triangle `pid`/`p1` split an object into one part per material).

use crate::{Mesh, Part};
use anyhow::{Context, bail};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use std::collections::HashMap;
use std::io::{Cursor, Read};

/// Row-major 3×4 affine: p' = M[:, ..3]·p + M[:, 3].
type Affine = [[f64; 4]; 3];
const IDENTITY: Affine = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];

/// `a ∘ b` (apply `b`, then `a`).
fn compose(a: &Affine, b: &Affine) -> Affine {
    let mut m = [[0.0; 4]; 3];
    for i in 0..3 {
        for j in 0..4 {
            let mut s = if j == 3 { a[i][3] } else { 0.0 };
            for k in 0..3 {
                s += a[i][k] * b[k][j];
            }
            m[i][j] = s;
        }
    }
    m
}

fn det3(m: &Affine) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// 3MF transforms are 12 numbers `m00 m01 m02 m10 m11 m12 m20 m21 m22 m30
/// m31 m32` in row-vector convention: x' = m00·x + m10·y + m20·z + m30, …
fn parse_transform(s: &str) -> Option<Affine> {
    let v: Vec<f64> = s.split_ascii_whitespace().map(|t| t.parse::<f64>()).collect::<Result<_, _>>().ok()?;
    if v.len() != 12 || v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let m = |r: usize, c: usize| v[r * 3 + c];
    Some([
        [m(0, 0), m(1, 0), m(2, 0), m(3, 0)],
        [m(0, 1), m(1, 1), m(2, 1), m(3, 1)],
        [m(0, 2), m(1, 2), m(2, 2), m(3, 2)],
    ])
}

fn unit_to_mm(u: &str) -> Option<f64> {
    Some(match u {
        "micron" => 0.001,
        "millimeter" | "millimetre" => 1.0,
        "centimeter" | "centimetre" => 10.0,
        "inch" => 25.4,
        "foot" => 304.8,
        "meter" | "metre" => 1000.0,
        _ => return None,
    })
}

/// Minimal XML entity decoding for attribute text (names, paths).
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(j) = rest.find(';') else { break };
        let ent = &rest[1..j];
        let rep = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match rep {
            Some(c) => {
                out.push(c);
                rest = &rest[j + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[derive(Default)]
struct MeshObj {
    verts: Vec<[f32; 3]>,
    tris: Vec<[u32; 3]>,
    /// Per-triangle (pid, p1) override index into `mat_keys`, or u16::MAX.
    tri_mat: Vec<u16>,
    mat_keys: Vec<(u32, u32)>,
}

#[derive(Clone)]
struct Component {
    objectid: u32,
    transform: Affine,
    path: Option<String>,
}

enum Kind {
    Mesh(MeshObj),
    Components(Vec<Component>),
    Empty,
}

struct Object {
    name: Option<String>,
    kind: Kind,
    default_mat: Option<(u32, u32)>,
    skip: bool,
}

struct Model {
    unit_mm: Option<f64>,
    objects: HashMap<u32, Object>,
    build: Vec<Component>,
    basemats: HashMap<u32, Vec<String>>,
}

fn attrs(e: &BytesStart<'_>, mut f: impl FnMut(&str, &str)) {
    let mut it = e.attributes();
    it.with_checks(false);
    for a in it.flatten() {
        let key = a.key.local_name();
        // Keep the namespace prefix visible for p:path / pid ambiguity.
        let full = a.key.as_ref();
        let k: &str = if full.starts_with("p:") && key.as_ref() == "path" { "p:path" } else { key.as_ref() };
        f(k, a.value.as_ref());
    }
}

fn parse_model(xml: &str) -> anyhow::Result<Model> {
    let xml = xml.strip_prefix('\u{feff}').unwrap_or(xml);
    let mut r = Reader::from_str(xml);
    let mut model = Model { unit_mm: None, objects: HashMap::new(), build: vec![], basemats: HashMap::new() };
    let mut cur: Option<(u32, Object)> = None;
    let mut in_build = false;
    let mut basemat: Option<(u32, Vec<String>)> = None;
    let mut unit_seen: Option<String> = None;
    loop {
        let ev = r.read_event().map_err(|e| anyhow::anyhow!("3MF XML error at byte {}: {e}", r.buffer_position()))?;
        let (e, empty) = match &ev {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(e) => {
                match e.local_name().as_ref() {
                    "object" => {
                        if let Some((id, o)) = cur.take() {
                            model.objects.insert(id, o);
                        }
                    }
                    "build" => in_build = false,
                    "basematerials" => {
                        if let Some((id, v)) = basemat.take() {
                            model.basemats.insert(id, v);
                        }
                    }
                    _ => {}
                }
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        let name = e.local_name();
        match name.as_ref() {
            "vertex" => {
                if let Some((_, Object { kind: Kind::Mesh(m), .. })) = cur.as_mut() {
                    let mut p = [f32::NAN; 3];
                    attrs(e, |k, v| match k {
                        "x" => p[0] = v.trim().parse().unwrap_or(f32::NAN),
                        "y" => p[1] = v.trim().parse().unwrap_or(f32::NAN),
                        "z" => p[2] = v.trim().parse().unwrap_or(f32::NAN),
                        _ => {}
                    });
                    m.verts.push(p);
                }
            }
            "triangle" => {
                if let Some((_, Object { kind: Kind::Mesh(m), .. })) = cur.as_mut() {
                    let mut t = [u32::MAX; 3];
                    let mut pid: Option<u32> = None;
                    let mut p1: Option<u32> = None;
                    attrs(e, |k, v| match k {
                        "v1" => t[0] = v.trim().parse().unwrap_or(u32::MAX),
                        "v2" => t[1] = v.trim().parse().unwrap_or(u32::MAX),
                        "v3" => t[2] = v.trim().parse().unwrap_or(u32::MAX),
                        "pid" => pid = v.trim().parse().ok(),
                        "p1" => p1 = v.trim().parse().ok(),
                        _ => {}
                    });
                    if pid.is_some() || p1.is_some() {
                        // Resolved against the object's defaults when the mesh is finished.
                        let key = (pid.unwrap_or(u32::MAX), p1.unwrap_or(u32::MAX));
                        let idx = match m.mat_keys.iter().position(|k| *k == key) {
                            Some(i) => i,
                            None => {
                                m.mat_keys.push(key);
                                m.mat_keys.len() - 1
                            }
                        };
                        if m.tri_mat.len() < m.tris.len() {
                            m.tri_mat.resize(m.tris.len(), u16::MAX);
                        }
                        m.tri_mat.push(idx.min(u16::MAX as usize - 1) as u16);
                    } else if !m.tri_mat.is_empty() {
                        m.tri_mat.push(u16::MAX);
                    }
                    m.tris.push(t);
                }
            }
            "object" => {
                let mut id = None;
                let mut oname = None;
                let mut pid = None;
                let mut pindex = None;
                let mut skip = false;
                attrs(e, |k, v| match k {
                    "id" => id = v.trim().parse::<u32>().ok(),
                    "name" => oname = Some(unescape(v)),
                    "pid" => pid = v.trim().parse::<u32>().ok(),
                    "pindex" => pindex = v.trim().parse::<u32>().ok(),
                    "type" => skip = v == "support" || v == "solidsupport",
                    _ => {}
                });
                let id = id.context("3MF: <object> without id")?;
                let obj = Object {
                    name: oname.filter(|n| !n.trim().is_empty()),
                    kind: Kind::Empty,
                    default_mat: pid.map(|p| (p, pindex.unwrap_or(0))),
                    skip,
                };
                if empty {
                    model.objects.insert(id, obj);
                } else {
                    cur = Some((id, obj));
                }
            }
            "mesh" => {
                if let Some((_, o)) = cur.as_mut() {
                    o.kind = Kind::Mesh(MeshObj::default());
                }
            }
            "components" => {
                if let Some((_, o)) = cur.as_mut() {
                    o.kind = Kind::Components(vec![]);
                }
            }
            "component" => {
                if let Some((_, Object { kind: Kind::Components(cs), .. })) = cur.as_mut() {
                    let mut c = Component { objectid: u32::MAX, transform: IDENTITY, path: None };
                    attrs(e, |k, v| match k {
                        "objectid" => c.objectid = v.trim().parse().unwrap_or(u32::MAX),
                        "transform" => c.transform = parse_transform(v).unwrap_or(IDENTITY),
                        "p:path" | "path" => c.path = Some(unescape(v)),
                        _ => {}
                    });
                    cs.push(c);
                }
            }
            "build" => in_build = !empty,
            "item" => {
                if in_build {
                    let mut c = Component { objectid: u32::MAX, transform: IDENTITY, path: None };
                    attrs(e, |k, v| match k {
                        "objectid" => c.objectid = v.trim().parse().unwrap_or(u32::MAX),
                        "transform" => c.transform = parse_transform(v).unwrap_or(IDENTITY),
                        "p:path" | "path" => c.path = Some(unescape(v)),
                        _ => {}
                    });
                    model.build.push(c);
                }
            }
            "basematerials" => {
                let mut id = None;
                attrs(e, |k, v| {
                    if k == "id" {
                        id = v.trim().parse::<u32>().ok();
                    }
                });
                if let Some(id) = id {
                    if empty {
                        model.basemats.insert(id, vec![]);
                    } else {
                        basemat = Some((id, vec![]));
                    }
                }
            }
            "base" => {
                if let Some((_, v)) = basemat.as_mut() {
                    let mut n = String::new();
                    attrs(e, |k, val| {
                        if k == "name" {
                            n = unescape(val);
                        }
                    });
                    v.push(n);
                }
            }
            "model" => {
                attrs(e, |k, v| {
                    if k == "unit" {
                        unit_seen = Some(v.trim().to_ascii_lowercase());
                    }
                });
            }
            _ => {}
        }
    }
    if let Some(u) = unit_seen {
        model.unit_mm = Some(unit_to_mm(&u).with_context(|| format!("3MF: unknown unit {u:?}"))?);
    }
    Ok(model)
}

struct Package {
    archive: zip::ZipArchive<Cursor<Vec<u8>>>,
    /// lowercase path without leading '/' → entry name
    names: HashMap<String, String>,
    models: HashMap<String, Model>,
}

fn norm_path(p: &str) -> String {
    p.trim().trim_start_matches('/').replace('\\', "/").to_ascii_lowercase()
}

impl Package {
    fn read_entry(&mut self, path: &str) -> anyhow::Result<Vec<u8>> {
        let key = norm_path(path);
        let real = self.names.get(&key).with_context(|| format!("3MF: missing part {path}"))?.clone();
        let mut f = self.archive.by_name(&real).with_context(|| format!("3MF: cannot open {real}"))?;
        let mut buf = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut buf).with_context(|| format!("3MF: cannot inflate {real}"))?;
        Ok(buf)
    }

    fn model(&mut self, path: &str) -> anyhow::Result<&Model> {
        let key = norm_path(path);
        if !self.models.contains_key(&key) {
            let bytes = self.read_entry(path)?;
            let text = std::str::from_utf8(&bytes).with_context(|| format!("3MF: {path} is not UTF-8"))?;
            let m = parse_model(text).with_context(|| format!("3MF: parsing {path}"))?;
            self.models.insert(key.clone(), m);
        }
        Ok(&self.models[&key])
    }
}

fn root_model_path(pkg: &mut Package) -> String {
    if let Ok(rels) = pkg.read_entry("_rels/.rels")
        && let Ok(text) = std::str::from_utf8(&rels)
    {
        let mut r = Reader::from_str(text);
        while let Ok(ev) = r.read_event() {
            match ev {
                Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == "Relationship" => {
                    let mut target = None;
                    let mut is_model = false;
                    attrs(&e, |k, v| match k {
                        "Target" => target = Some(unescape(v)),
                        "Type" => is_model = v.ends_with("/3dmodel"),
                        _ => {}
                    });
                    if is_model && let Some(t) = target {
                        return t;
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
    }
    if pkg.names.contains_key("3d/3dmodel.model") {
        return "3D/3dmodel.model".into();
    }
    pkg.names.keys().find(|k| k.ends_with(".model")).cloned().unwrap_or_else(|| "3D/3dmodel.model".into())
}

struct Builder {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    parts: Vec<Part>,
    scale: f64,
    bad_tris: usize,
}

pub(crate) fn parse(bytes: &[u8], warnings: &mut Vec<String>) -> anyhow::Result<Mesh> {
    let archive = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).context("3MF: not a valid zip archive")?;
    let names: HashMap<String, String> =
        archive.file_names().filter_map(|n| n.ok()).map(|n| (norm_path(&n), n.into_owned())).collect();
    let mut pkg = Package { archive, names, models: HashMap::new() };
    let root = root_model_path(&mut pkg);
    let root_model = pkg.model(&root)?;
    let scale = root_model.unit_mm.unwrap_or(1.0);
    let mut build = root_model.build.clone();
    if build.is_empty() {
        // No build section: place every object that no component references.
        let referenced: std::collections::HashSet<u32> = root_model
            .objects
            .values()
            .filter_map(|o| match &o.kind {
                Kind::Components(cs) => Some(cs.iter().filter(|c| c.path.is_none()).map(|c| c.objectid)),
                _ => None,
            })
            .flatten()
            .collect();
        let mut ids: Vec<u32> = root_model.objects.keys().copied().filter(|id| !referenced.contains(id)).collect();
        ids.sort_unstable();
        build = ids.into_iter().map(|id| Component { objectid: id, transform: IDENTITY, path: None }).collect();
        warnings.push("3MF: no <build> items; placing all top-level objects".into());
    }
    let mut b = Builder { positions: vec![], triangles: vec![], parts: vec![], scale, bad_tris: 0 };
    for item in build {
        let path = item.path.clone().unwrap_or_else(|| root.clone());
        instantiate(&mut pkg, &path, item.objectid, item.transform, 0, None, &mut b, warnings)?;
    }
    if b.bad_tris > 0 {
        warnings.push(format!("3MF: {} triangle(s) with invalid vertex indices dropped", b.bad_tris));
    }
    if b.triangles.is_empty() {
        bail!("3MF: no mesh triangles in the build");
    }
    Ok(Mesh { positions: b.positions, normals: vec![], triangles: b.triangles, parts: b.parts })
}

#[allow(clippy::too_many_arguments)]
fn instantiate(
    pkg: &mut Package,
    path: &str,
    objectid: u32,
    xf: Affine,
    depth: usize,
    inherited_name: Option<&str>,
    b: &mut Builder,
    warnings: &mut Vec<String>,
) -> anyhow::Result<()> {
    if depth > 32 {
        bail!("3MF: component nesting deeper than 32 (cycle?)");
    }
    let model = pkg.model(path)?;
    let Some(obj) = model.objects.get(&objectid) else {
        warnings.push(format!("3MF: build/component references missing object {objectid} in {path}"));
        return Ok(());
    };
    if obj.skip {
        return Ok(());
    }
    let name = obj.name.clone().or_else(|| inherited_name.map(str::to_string)).unwrap_or_else(|| format!("object{objectid}"));
    match &obj.kind {
        Kind::Empty => Ok(()),
        Kind::Components(cs) => {
            let cs = cs.clone();
            for c in cs {
                let p = c.path.clone().unwrap_or_else(|| path.to_string());
                instantiate(pkg, &p, c.objectid, compose(&xf, &c.transform), depth + 1, Some(&name), b, warnings)?;
            }
            Ok(())
        }
        Kind::Mesh(m) => {
            let resolve = |key: Option<(u32, u32)>| -> Option<String> {
                let (pid, idx) = key?;
                model.basemats.get(&pid).and_then(|v| v.get(idx as usize)).filter(|n| !n.is_empty()).cloned()
            };
            let default = resolve(obj.default_mat);
            let s = b.scale;
            let base = b.positions.len() as u32;
            for p in &m.verts {
                let q = [p[0] as f64, p[1] as f64, p[2] as f64];
                let mut o = [0f32; 3];
                for i in 0..3 {
                    o[i] = ((xf[i][0] * q[0] + xf[i][1] * q[1] + xf[i][2] * q[2] + xf[i][3]) * s) as f32;
                }
                b.positions.push(o);
            }
            let mirror = det3(&xf) < 0.0;
            let nv = m.verts.len() as u32;
            // Material per triangle (pid/p1 override, falling back to the object's).
            let tri_material = |t: usize| -> Option<String> {
                match m.tri_mat.get(t) {
                    Some(&k) if k != u16::MAX => {
                        let (pid, p1) = m.mat_keys[k as usize];
                        let pid = if pid == u32::MAX { obj.default_mat.map(|d| d.0) } else { Some(pid) };
                        let idx = if p1 == u32::MAX { obj.default_mat.map(|d| d.1).unwrap_or(0) } else { p1 };
                        pid.and_then(|p| resolve(Some((p, idx)))).or_else(|| default.clone())
                    }
                    _ => default.clone(),
                }
            };
            // Group triangles by material, in order of first appearance.
            let mut groups: Vec<(Option<String>, Vec<u32>)> = Vec::new();
            if m.tri_mat.iter().all(|&k| k == u16::MAX) {
                groups.push((default.clone(), (0..m.tris.len() as u32).collect()));
            } else {
                let mut index: HashMap<Option<String>, usize> = HashMap::new();
                for t in 0..m.tris.len() {
                    let mat = tri_material(t);
                    let gi = *index.entry(mat.clone()).or_insert_with(|| {
                        groups.push((mat, vec![]));
                        groups.len() - 1
                    });
                    groups[gi].1.push(t as u32);
                }
            }
            let multi = groups.len() > 1;
            for (mat, tris) in groups {
                let start = b.triangles.len() as u32;
                for t in tris {
                    let tri = m.tris[t as usize];
                    if tri.iter().any(|&i| i >= nv) {
                        b.bad_tris += 1;
                        continue;
                    }
                    let mut g = [tri[0] + base, tri[1] + base, tri[2] + base];
                    if mirror {
                        g.swap(1, 2);
                    }
                    b.triangles.push(g);
                }
                let end = b.triangles.len() as u32;
                if end > start {
                    let pname = match (&mat, multi) {
                        (Some(mn), true) => format!("{name}:{mn}"),
                        _ => name.clone(),
                    };
                    b.parts.push(Part { name: pname, triangles: start..end, material: mat });
                }
            }
            Ok(())
        }
    }
}
