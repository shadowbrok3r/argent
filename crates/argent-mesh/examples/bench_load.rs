//! Load-path benchmark on a synthetic dense ring.
//!
//! `cargo run -p argent-mesh --release --example bench_load -- OUT.stl [MILLION_TRIANGLES]`
//! writes (if missing) a binary STL torus band with ~N million triangles,
//! then loads it three times and prints the timing breakdown.

use argent_mesh::{LoadOptions, load_with_report, orient, synth};
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let out = args.next().ok_or_else(|| anyhow::anyhow!("usage: bench_load OUT.stl [MILLION_TRIANGLES]"))?;
    let millions: f64 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(5.0);
    let path = std::path::Path::new(&out);
    if !path.exists() {
        let tris = millions * 1e6;
        let nv = ((tris / 2.0 / 3.0).sqrt()).round() as usize; // nu = 3·nv
        let t = Instant::now();
        // Comfort-fit band with a wavy outer surface (no creases, ZBrush-like density).
        let mut m = synth::torus_profile(3 * nv, nv, |v| {
            let r = 9.75 + 1.1 * v.cos() + 0.08 * (7.0 * v).sin();
            (r, 3.0 * v.sin())
        });
        synth::unweld(&mut m);
        std::fs::write(path, synth::to_binary_stl(&m))?;
        eprintln!("wrote {} ({} triangles) in {:.2} s", path.display(), m.triangles.len(), t.elapsed().as_secs_f64());
    }
    for run in 0..3 {
        let (mesh, r) = load_with_report(path, &LoadOptions::default())?;
        let t = Instant::now();
        let o = orient::detect(&mesh);
        let to = t.elapsed().as_secs_f64();
        println!(
            "run {run}: {} tris, {} verts | read {:.3} parse {:.3} weld {:.3} normals {:.3} = {:.3} s | orient {:.3} s ({:?}, bore Ø {:.2})",
            mesh.triangles.len(),
            mesh.positions.len(),
            r.read.as_secs_f64(),
            r.parse.as_secs_f64(),
            r.weld.as_secs_f64(),
            r.normals.as_secs_f64(),
            r.total().as_secs_f64(),
            to,
            o.kind,
            2.0 * o.bore_radius.unwrap_or(0.0)
        );
    }
    Ok(())
}
