//! Debug view of the auto-oriented mesh: front (+Z), back (−Z), top (+Y)
//! and side (+X) orthographic views in a 2×2 PPM.
//!
//! `cargo run -p argent-mesh --release --example mesh_views -- IN OUT.ppm [--scale S] [--raw]`
//! (`--raw` skips orientation, showing the file's own frame).

use argent_mesh::{LoadOptions, cavity, orient};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or_else(|| anyhow::anyhow!("usage: mesh_views IN OUT.ppm [--scale S] [--raw]"))?;
    let out = args.next().ok_or_else(|| anyhow::anyhow!("missing OUT.ppm"))?;
    let mut opts = LoadOptions::default();
    let mut raw = false;
    let mut patina = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--scale" => opts.unit_scale = args.next().unwrap_or_default().parse()?,
            "--raw" => raw = true,
            "--patina" => patina = true,
            _ => {}
        }
    }
    let mut mesh = argent_mesh::load(std::path::Path::new(&input), &opts)?;
    if !raw {
        let o = orient::detect(&mesh);
        for n in &o.notes {
            eprintln!("· {n}");
        }
        mesh.apply_pose(&o.pose);
    }
    let mask = if patina {
        let bvh = cavity::SimpleBvh::new(&mesh);
        Some(cavity::patina_mask(&mesh, &bvh, &cavity::PatinaSettings::default()))
    } else {
        None
    };
    let s = 512usize;
    let (lo, hi) = mesh.bounds();
    let half = (0..3).map(|k| lo[k].abs().max(hi[k].abs())).fold(0.0f32, f32::max) * 1.05;
    // (right, up, toward-viewer) per view.
    let views: [([f32; 3], [f32; 3], [f32; 3]); 4] = [
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),   // front
        ([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]), // back
        ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),  // top
        ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),  // side (+X)
    ];
    let mut img = vec![[40u8, 40, 48]; 4 * s * s];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    for (vi, (r, u, f)) in views.iter().enumerate() {
        let (ox, oy) = ((vi % 2) * s, (vi / 2) * s);
        let mut depth = vec![f32::MIN; s * s];
        let to_px = |p: [f32; 3]| {
            let x = (dot(p, *r) / half * 0.5 + 0.5) * s as f32;
            let y = (0.5 - dot(p, *u) / half * 0.5) * s as f32;
            (x, y, dot(p, *f))
        };
        for t in &mesh.triangles {
            let pts = t.map(|i| to_px(mesh.positions[i as usize]));
            let ns = t.map(|i| mesh.normals.get(i as usize).copied().unwrap_or([0.0, 0.0, 1.0]));
            let (x0, x1) = (pts.iter().map(|p| p.0).fold(f32::MAX, f32::min), pts.iter().map(|p| p.0).fold(f32::MIN, f32::max));
            let (y0, y1) = (pts.iter().map(|p| p.1).fold(f32::MAX, f32::min), pts.iter().map(|p| p.1).fold(f32::MIN, f32::max));
            let area = (pts[1].0 - pts[0].0) * (pts[2].1 - pts[0].1) - (pts[2].0 - pts[0].0) * (pts[1].1 - pts[0].1);
            if area.abs() < 1e-12 {
                continue;
            }
            for py in (y0.floor().max(0.0) as usize)..=(y1.ceil().min(s as f32 - 1.0) as usize) {
                for px in (x0.floor().max(0.0) as usize)..=(x1.ceil().min(s as f32 - 1.0) as usize) {
                    let (qx, qy) = (px as f32 + 0.5, py as f32 + 0.5);
                    let w0 = ((pts[1].0 - qx) * (pts[2].1 - qy) - (pts[2].0 - qx) * (pts[1].1 - qy)) / area;
                    let w1 = ((pts[2].0 - qx) * (pts[0].1 - qy) - (pts[0].0 - qx) * (pts[2].1 - qy)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                        continue;
                    }
                    let z = w0 * pts[0].2 + w1 * pts[1].2 + w2 * pts[2].2;
                    let k = py * s + px;
                    if z > depth[k] {
                        depth[k] = z;
                        let n = [0, 1, 2].map(|c| w0 * ns[0][c] + w1 * ns[1][c] + w2 * ns[2][c]);
                        let l = (dot(n, n)).sqrt().max(1e-9);
                        let n = n.map(|c| c / l);
                        // Key light from upper-left-front of the view, plus a rim of facing ratio.
                        let key = [-0.4 * r[0] + 0.6 * u[0] + 0.7 * f[0], -0.4 * r[1] + 0.6 * u[1] + 0.7 * f[1], -0.4 * r[2] + 0.6 * u[2] + 0.7 * f[2]];
                        let kl = dot(key, key).sqrt();
                        let d = (dot(n, key) / kl).max(0.0);
                        let facing = dot(n, *f).abs();
                        let mut v = (0.15 + 0.65 * d + 0.2 * facing).min(1.0);
                        if let Some(m) = &mask {
                            let pm = w0 * m[t[0] as usize] + w1 * m[t[1] as usize] + w2 * m[t[2] as usize];
                            v *= 1.0 - 0.85 * pm;
                        }
                        img[(oy + py) * 2 * s + ox + px] = [(v * 235.0) as u8, (v * 205.0) as u8, (v * 150.0) as u8];
                    }
                }
            }
        }
        // Axis ticks: a red dot at +Y top centre of every view and a label bar.
        for k in 0..6 {
            img[(oy + 4 + k) * 2 * s + ox + s / 2] = [255, 60, 60];
        }
    }
    let mut ppm = format!("P6\n{} {}\n255\n", 2 * s, 2 * s).into_bytes();
    for px in img {
        ppm.extend_from_slice(&px);
    }
    std::fs::write(&out, ppm)?;
    eprintln!("wrote {out} (front | back / top | side)");
    Ok(())
}
