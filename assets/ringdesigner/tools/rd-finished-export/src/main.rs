//! rd-finished-export: a RingDesigner design as the finished ring at nominal
//! size, plus its stones, for render testing.
//!
//! ```text
//! rd-finished-export <design.ring.json> --out <dir> [--steps 1536x448] [--size 7] [--glb]
//! ```
//!
//! Writes into `<dir>`:
//! - `<slug>_size<N>.stl`: binary STL, millimetres, the metal exactly as
//!   `mesh::try_build` builds it. That is the finished ring the app shows and the
//!   showcase saves as `finished-metal.stl`, not `try_build_pattern`'s casting
//!   pattern (which `ringdesign export` writes to STL) and never shrink-scaled.
//! - `<slug>_size<N>_stones.obj` and `_stones.json` when the design sets stones:
//!   one closed, flat-faceted solid per stone (`o stone_NN`), same frame as the STL.
//! - `<slug>_size<N>.glb` with `--glb`: `ringdesign_core::gltf::write_glb` (the CLI's
//!   writer, gold PBR, metres) on the same finished mesh. Metal only, no stones.
//!
//! The last stdout line is a JSON summary of the export.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, bail, ensure};
use ringdesign_core::{AlphaLibrary, RingSize, gems, gltf, library, mesh, render, stl, stones};

const USAGE: &str = "usage: rd-finished-export <design.ring.json> --out <dir> [--steps THETAxPROFILE] [--size US] [--glb]";

/// Floats per vertex in `gems::built_vertices`: position, normal, colour, colour.
const STRIDE: usize = 12;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut it = args.iter();
    let design_path = it.next().context(USAGE)?.clone();
    let (mut out, mut steps, mut size, mut glb) = (None::<PathBuf>, None::<(usize, usize)>, None::<f64>, false);
    while let Some(flag) = it.next() {
        let mut value = || it.next().with_context(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--out" => out = Some(PathBuf::from(value()?)),
            // The CLI's own GLB writer, always on the finished mesh: `ringdesign export --formats glb`
            // writes the pattern when a design differs from it only by bench-only layers.
            "--glb" => glb = true,
            "--steps" => {
                let v = value()?;
                let (t, p) = v.split_once(['x', 'X']).context("--steps wants THETAxPROFILE, e.g. 1536x448")?;
                steps = Some((t.trim().parse()?, p.trim().parse()?));
            }
            "--size" => size = Some(value()?.parse()?),
            other => bail!("unknown option {other:?}\n{USAGE}"),
        }
    }
    let out = out.context(USAGE)?;
    std::fs::create_dir_all(&out)?;

    // Loaded and baked the way `ringdesign export` does it.
    let mut d = library::load_design(&design_path).with_context(|| design_path.clone())?;
    ensure!(d.graph.is_none(), "{design_path} carries a graph: evaluate it with `ringdesign graph eval` first");
    let mut lib = AlphaLibrary::builtin();
    d.unpack_embedded(&mut lib);
    d.bake_all(&mut lib);
    if let Some(s) = size {
        d.size = RingSize(s);
    }
    if let Some(base) = &d.imported_base {
        base.validate_shape(&d)?;
    }
    let mut params = d.build;
    if let Some((t, p)) = steps {
        params.theta_steps = t;
        params.profile_steps = p;
        params.refine = None;
    }

    let started = Instant::now();
    let built = mesh::try_build(&d, &lib, params)?;
    let build_ms = started.elapsed().as_millis();
    let v = built.mesh.validate();
    let tag = format!("{}_size{}", slug(&d.name), fmt_size(d.size.0));
    let stl_name = format!("{tag}.stl");
    let stl_bytes = stl::write_stl(out.join(&stl_name), &built.mesh, &d.name)?;
    let signed_volume = signed_volume(built.mesh.vertices.iter().map(|p| [p.0, p.1, p.2]), built.mesh.faces.iter().copied());

    let stones_out = write_stones(&d, &lib, &built, &out, &tag, &stl_name)?;
    let glb_out = if glb {
        let name = format!("{tag}.glb");
        let bytes = gltf::write_glb(out.join(&name), &built.mesh, &d.name, render::GOLD)?;
        serde_json::json!({ "file": name, "bytes": bytes })
    } else {
        serde_json::Value::Null
    };

    let summary = serde_json::json!({
        "design": d.name,
        "source": design_path,
        "size": d.size.display(),
        "size_us": d.size.0,
        "steps": format!("{}x{}", params.theta_steps, params.profile_steps),
        "build_ms": build_ms,
        "stl": stl_name,
        "stl_bytes": stl_bytes,
        "triangles": v.triangle_count,
        "vertices": v.vertex_count,
        "watertight": v.watertight,
        "boundary_edges": v.boundary_edges,
        "non_manifold_edges": v.non_manifold_edges,
        "volume_mm3": built.report.volume_mm3,
        "signed_volume_mm3": signed_volume,
        "bounds_mm": built.report.bounds_mm,
        "inner_diameter_mm": built.report.inner_diameter_mm,
        "process": format!("{:?}", d.draft.process),
        // A made setting, stamp or CAD part that failed to resolve is left out and named here.
        "solids": { "resolved": built.solids.resolved, "stamped": built.solids.stamped, "notes": built.solids.notes },
        "parts": {
            "joined": built.parts.joined, "cut": built.parts.cut, "separate": built.parts.separate,
            "references": built.parts.references, "notes": built.parts.notes,
        },
        "stones": stones_out,
        "glb": glb_out,
    });
    println!("{summary}");
    Ok(())
}

/// Every stone `gems::built_vertices` draws, split into one welded solid per
/// stone, matched to its record by nearest girdle centre, written as OBJ + JSON.
fn write_stones(
    d: &ringdesign_core::RingDesign,
    lib: &AlphaLibrary,
    built: &ringdesign_core::BuildResult,
    out: &std::path::Path,
    tag: &str,
    stl_name: &str,
) -> anyhow::Result<serde_json::Value> {
    let soup = gems::built_vertices(d, lib, built);
    if soup.is_empty() {
        return Ok(serde_json::Value::Null);
    }
    // Weld on a 0.1 µm grid (the showcase examples' key).
    let mut index: HashMap<[i64; 3], u32> = HashMap::new();
    let mut verts: Vec<[f32; 3]> = Vec::new();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    let mut tints: Vec<[f32; 3]> = Vec::new();
    for tri in soup.chunks_exact(3 * STRIDE) {
        let f: [u32; 3] = std::array::from_fn(|k| {
            let p = [tri[k * STRIDE], tri[k * STRIDE + 1], tri[k * STRIDE + 2]];
            let key = p.map(|c| (c as f64 * 1e4).round() as i64);
            *index.entry(key).or_insert_with(|| {
                verts.push(p);
                (verts.len() - 1) as u32
            })
        });
        if f[0] == f[1] || f[1] == f[2] || f[0] == f[2] {
            continue;
        }
        tris.push(f);
        tints.push([tri[6], tri[7], tri[8]]);
    }
    // Components by shared edges, so stones that merely touch at a point stay apart.
    let mut parent: Vec<usize> = (0..tris.len()).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let mut first_on_edge: HashMap<(u32, u32), usize> = HashMap::new();
    for (t, f) in tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (f[k], f[(k + 1) % 3]);
            let e = (a.min(b), a.max(b));
            match first_on_edge.get(&e) {
                Some(&o) => {
                    let (ra, rb) = (find(&mut parent, t), find(&mut parent, o));
                    if ra != rb {
                        parent[ra] = rb;
                    }
                }
                None => {
                    first_on_edge.insert(e, t);
                }
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut slot: HashMap<usize, usize> = HashMap::new();
    for t in 0..tris.len() {
        let r = find(&mut parent, t);
        let g = *slot.entry(r).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[g].push(t);
    }

    let records = stones::all_stone_frames_built(d, built);
    let mut obj = String::new();
    let _ = writeln!(obj, "# {} : stones, size {}, millimetres, same frame as {stl_name}", d.name, d.size.display());
    let _ = writeln!(obj, "# One object per stone; each is a closed, flat-faceted solid wound outward.");
    let _ = writeln!(obj, "# Render with facet normals (no smoothing). Details per stone in {tag}_stones.json.");
    let mut entries = Vec::new();
    let mut base = 1usize;
    let mut total_tris = 0usize;
    for (k, g) in groups.iter().enumerate() {
        // Local vertex list and outward winding.
        let mut local: HashMap<u32, usize> = HashMap::new();
        let mut lv: Vec<[f32; 3]> = Vec::new();
        let mut lf: Vec<[usize; 3]> = Vec::new();
        for &t in g {
            let f = tris[t].map(|i| {
                *local.entry(i).or_insert_with(|| {
                    lv.push(verts[i as usize]);
                    lv.len() - 1
                })
            });
            lf.push(f);
        }
        let vol = signed_volume(lv.iter().copied(), lf.iter().map(|f| f.map(|i| i as u32)));
        let flipped = vol < 0.0;
        if flipped {
            for f in &mut lf {
                f.swap(1, 2);
            }
        }
        let mut edge_uses: HashMap<(usize, usize), u32> = HashMap::new();
        for f in &lf {
            for e in 0..3 {
                let (a, b) = (f[e], f[(e + 1) % 3]);
                *edge_uses.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let open_edges = edge_uses.values().filter(|&&n| n != 2).count();
        let n = lv.len() as f64;
        let c = lv.iter().fold([0.0f64; 3], |a, p| [a[0] + p[0] as f64 / n, a[1] + p[1] as f64 / n, a[2] + p[2] as f64 / n]);
        let nearest = records
            .iter()
            .map(|(st, fr)| (st, fr, dist(c, fr.girdle)))
            .min_by(|a, b| a.2.total_cmp(&b.2));
        let name = format!("stone_{k:02}");
        let _ = writeln!(obj, "o {name}");
        let _ = writeln!(obj, "usemtl gem");
        for p in &lv {
            let _ = writeln!(obj, "v {:.6} {:.6} {:.6}", p[0], p[1], p[2]);
        }
        for f in &lf {
            let _ = writeln!(obj, "f {} {} {}", f[0] + base, f[1] + base, f[2] + base);
        }
        base += lv.len();
        total_tris += lf.len();
        let tint = tints[g[0]];
        let mut e = serde_json::json!({
            "object": name,
            "triangles": lf.len(),
            "closed": open_edges == 0,
            "open_edges": open_edges,
            "rewound": flipped,
            "volume_mm3": vol.abs(),
            "centroid_mm": c,
            "tint_linear_rgb": tint,
        });
        if let Some((st, fr, dist_mm)) = nearest {
            let m = e.as_object_mut().unwrap();
            m.insert("label".into(), st.label.clone().into());
            m.insert("cut".into(), format!("{:?}", st.gem.cut).into());
            m.insert("form".into(), format!("{:?}", st.gem.form).into());
            m.insert("w_mm".into(), st.gem.w_mm.into());
            m.insert("l_mm".into(), st.gem.l_mm.into());
            m.insert("carats".into(), st.gem.carats().into());
            m.insert("girdle_mm".into(), fr.girdle.to_vec().into());
            m.insert("table_normal".into(), fr.normal.to_vec().into());
            m.insert("matched_record_distance_mm".into(), dist_mm.into());
        }
        entries.push(e);
    }
    let obj_name = format!("{tag}_stones.obj");
    std::fs::write(out.join(&obj_name), obj.as_bytes())?;
    let doc = serde_json::json!({
        "design": d.name,
        "size": d.size.display(),
        "units": "mm",
        "metal_mesh": stl_name,
        "stones_mesh": obj_name,
        "records": records.len(),
        "stones": entries,
    });
    std::fs::write(out.join(format!("{tag}_stones.json")), serde_json::to_vec_pretty(&doc)?)?;
    Ok(serde_json::json!({
        "obj": obj_name,
        "count": groups.len(),
        "records": records.len(),
        "triangles": total_tris,
        "all_closed": doc["stones"].as_array().unwrap().iter().all(|s| s["closed"] == true),
    }))
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Signed enclosed volume, mm³: positive when the faces wind outward.
fn signed_volume(verts: impl Iterator<Item = [f32; 3]>, faces: impl Iterator<Item = [u32; 3]>) -> f64 {
    let v: Vec<[f64; 3]> = verts.map(|p| p.map(f64::from)).collect();
    faces
        .map(|f| {
            let [a, b, c] = f.map(|i| v[i as usize]);
            (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0])) / 6.0
        })
        .sum()
}

/// `ringdesign export`'s file-name slug.
fn slug(name: &str) -> String {
    let s = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if s.is_empty() { "ring".into() } else { s }
}

/// `ringdesign export`'s size tag: the US number, or a bangle's bore in mm.
fn fmt_size(v: f64) -> String {
    let size = RingSize(v);
    let (value, unit) = if size.is_bangle() { (size.inner_diameter_mm(), "mm") } else { (v, "") };
    let s = format!("{value:.2}");
    format!("{}{unit}", s.trim_end_matches('0').trim_end_matches('.'))
}
