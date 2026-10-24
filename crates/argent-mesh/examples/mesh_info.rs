//! `cargo run -p argent-mesh --release --example mesh_info -- [options] FILE...`
//!
//! Prints counts, bounds, area, volume, casting-weight estimates, parts and
//! the detected orientation (with reasoning) for each file.
//!
//! Options: `--merge` (load all files as one mesh, one part per file),
//! `--no-weld`, `--crease DEG`, `--scale S` (e.g. 25.4 for inches),
//! `--patina` (also compute the patina mask with the built-in BVH).

use argent_mesh::{LoadOptions, Mesh, cavity, load_with_report, orient};
use std::path::PathBuf;
use std::time::Instant;

const DENSITIES: [(&str, f64); 3] = [("sterling", 10.36), ("14k gold", 13.07), ("bronze", 8.8)];

fn main() -> anyhow::Result<()> {
    let mut opts = LoadOptions::default();
    let mut files: Vec<PathBuf> = Vec::new();
    let (mut merge, mut patina) = (false, false);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--merge" => merge = true,
            "--patina" => patina = true,
            "--no-weld" => opts.weld = false,
            "--crease" => opts.crease_angle_deg = args.next().unwrap_or_default().parse()?,
            "--scale" => opts.unit_scale = args.next().unwrap_or_default().parse()?,
            "-h" | "--help" => {
                eprintln!("usage: mesh_info [--merge] [--no-weld] [--crease DEG] [--scale S] [--patina] FILE...");
                return Ok(());
            }
            _ => files.push(a.into()),
        }
    }
    if files.is_empty() {
        anyhow::bail!("no input files (try --help)");
    }
    if merge {
        let t = Instant::now();
        let mesh = argent_mesh::load_many(&files, &opts)?;
        println!("== merged {} file(s), loaded in {:.3} s", files.len(), t.elapsed().as_secs_f64());
        report(&mesh, patina);
        return Ok(());
    }
    for f in &files {
        println!("== {}", f.display());
        let (mesh, r) = match load_with_report(f, &opts) {
            Ok(x) => x,
            Err(e) => {
                println!("   error: {e:#}");
                continue;
            }
        };
        println!("   format     {} ({:.1} MB)", r.format, r.file_bytes as f64 / 1e6);
        println!(
            "   load       read {:.3} s · parse {:.3} s · weld {:.3} s · normals {:.3} s · total {:.3} s",
            r.read.as_secs_f64(),
            r.parse.as_secs_f64(),
            r.weld.as_secs_f64(),
            r.normals.as_secs_f64(),
            r.total().as_secs_f64()
        );
        let welded = r.weld_report.as_ref().map(|w| w.output_vertices).unwrap_or(r.raw_vertices);
        println!(
            "   raw        {} triangles, {} vertices → welded {} vertices, +{} crease splits",
            group(r.raw_triangles),
            group(r.raw_vertices),
            group(welded),
            group(r.normal_report.split_vertices)
        );
        let n = &r.normal_report;
        println!(
            "   topology   {} shell(s) · {} open edges · {} non-manifold · {} degenerate · flipped shells {} · repaired faces {}",
            n.shells, n.boundary_edges, n.nonmanifold_edges, n.degenerate_faces, n.flipped_shells, n.repaired_faces
        );
        for w in &r.warnings {
            println!("   warning    {w}");
        }
        report(&mesh, patina);
    }
    Ok(())
}

fn report(mesh: &Mesh, patina: bool) {
    let (lo, hi) = mesh.bounds();
    println!("   triangles  {}   vertices {}", group(mesh.triangles.len()), group(mesh.positions.len()));
    println!(
        "   bounds     ({:.2}, {:.2}, {:.2}) .. ({:.2}, {:.2}, {:.2})  size {:.2} × {:.2} × {:.2} mm",
        lo[0],
        lo[1],
        lo[2],
        hi[0],
        hi[1],
        hi[2],
        hi[0] - lo[0],
        hi[1] - lo[1],
        hi[2] - lo[2]
    );
    let area = mesh.area();
    let vol = mesh.volume();
    println!("   area       {area:.1} mm²   volume {vol:.1} mm³ ({:.3} cm³)", vol / 1000.0);
    let w: Vec<String> = DENSITIES.iter().map(|(n, d)| format!("{n} {:.2} g", vol / 1000.0 * d)).collect();
    println!("   weight     {}", w.join(" · "));
    println!("   parts      {}", mesh.parts.len());
    for p in mesh.parts.iter().take(12) {
        println!(
            "     - {} [{} tris]{}",
            p.name,
            group((p.triangles.end - p.triangles.start) as usize),
            p.material.as_ref().map(|m| format!(" material={m}")).unwrap_or_default()
        );
    }
    if mesh.parts.len() > 12 {
        println!("     … {} more", mesh.parts.len() - 12);
    }
    let t = Instant::now();
    let o = orient::detect(mesh);
    let dt = t.elapsed().as_secs_f64();
    let bore = o
        .bore_radius
        .map(|r| format!(", bore Ø {:.2} mm (US {:.2})", 2.0 * r, orient::us_ring_size(2.0 * r)))
        .unwrap_or_default();
    println!("   orient     {:?}, confidence {:.2}{bore}  ({dt:.3} s)", o.kind, o.confidence);
    for n in &o.notes {
        println!("     · {n}");
    }
    let r = o.pose.rotation;
    let axis_name = |v: [f32; 3]| {
        let (k, val) = (0..3).map(|k| (k, v[k])).max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap();
        format!("{}{}{}", if val >= 0.0 { "+" } else { "-" }, ["X", "Y", "Z"][k], if val.abs() < 0.999 { format!(" ({:.3})", val.abs()) } else { String::new() })
    };
    println!(
        "   pose       new X = mesh {}, new Y = mesh {}, new Z = mesh {}; centre ({:.3}, {:.3}, {:.3})",
        axis_name(r[0]),
        axis_name(r[1]),
        axis_name(r[2]),
        o.pose.center[0],
        o.pose.center[1],
        o.pose.center[2]
    );
    if patina {
        let t = Instant::now();
        let bvh = cavity::SimpleBvh::new(mesh);
        let tb = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let m = cavity::patina_mask(mesh, &bvh, &cavity::PatinaSettings::default());
        let tm = t.elapsed().as_secs_f64();
        let mut hist = [0usize; 5];
        for v in &m {
            hist[((v * 5.0) as usize).min(4)] += 1;
        }
        let mean = m.iter().map(|v| *v as f64).sum::<f64>() / m.len().max(1) as f64;
        println!(
            "   patina     bvh {tb:.2} s · mask {tm:.2} s · mean {mean:.3} · histogram (0-.2….8-1) {:?}",
            hist.map(|h| format!("{:.1}%", 100.0 * h as f64 / m.len().max(1) as f64))
        );
    }
}

fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
