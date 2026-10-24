//! Dev-only: render a real binary STL with a minimal loader (exact weld,
//! area-weighted normals) until argent-mesh's loaders land.
//! `cargo run --release --example real -- file.stl out.png [metal] [studio] [spp] [view] [WxH]`

use argent_core::camera::Lens;
use argent_core::geom::V3;
use argent_core::shot::{grade, render_image, Look, Prepared, Quality};
use argent_core::studio::Studio;
use argent_core::views::View;
use argent_mesh::{Mesh, Part, Pose};
use std::collections::HashMap;
use std::time::Instant;

fn load_stl(path: &str) -> Mesh {
    let data = std::fs::read(path).unwrap();
    let n = u32::from_le_bytes(data[80..84].try_into().unwrap()) as usize;
    let mut m = Mesh::default();
    let mut map: HashMap<[u32; 3], u32> = HashMap::with_capacity(n);
    for i in 0..n {
        let o = 84 + i * 50 + 12;
        let mut tri = [0u32; 3];
        for k in 0..3 {
            let mut p = [0f32; 3];
            for c in 0..3 {
                let b = o + k * 12 + c * 4;
                p[c] = f32::from_le_bytes(data[b..b + 4].try_into().unwrap());
            }
            let key = p.map(f32::to_bits);
            let id = *map.entry(key).or_insert_with(|| {
                m.positions.push(p);
                (m.positions.len() - 1) as u32
            });
            tri[k] = id;
        }
        if tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2] {
            m.triangles.push(tri);
        }
    }
    let mut nrm = vec![V3::ZERO; m.positions.len()];
    for t in &m.triangles {
        let [a, b, c] = t.map(|i| V3::from(m.positions[i as usize]));
        let f = (b - a).cross(c - a);
        for &i in t {
            nrm[i as usize] += f;
        }
    }
    m.normals = nrm.iter().map(|v| v.normalize_or_zero().into()).collect();
    let nt = m.triangles.len() as u32;
    m.parts = vec![Part { name: "metal".into(), triangles: 0..nt, material: None }];
    m
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let file = &a[1];
    let out = a.get(2).cloned().unwrap_or("real.png".into());
    let metal = a.get(3).cloned().unwrap_or("sterling".into());
    let studio = a.get(4).and_then(|s| Studio::from_name(s)).unwrap_or(Studio::Noir);
    let spp: u32 = a.get(5).and_then(|s| s.parse().ok()).unwrap_or(64);
    let view = a.get(6).cloned().unwrap_or("hero".into());
    let (w, h) = a.get(7).and_then(|s| s.split_once('x')).map(|(w, h)| (w.parse().unwrap(), h.parse().unwrap())).unwrap_or((540u32, 960u32));
    let t0 = Instant::now();
    let mesh = load_stl(file);
    println!("loaded {} tris {} verts in {:.2?}", mesh.triangles.len(), mesh.positions.len(), t0.elapsed());
    // RingDesigner: finger axis Z, head +Y already.
    let (lo, hi) = mesh.bounds();
    let pose = Pose { center: [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0], ..Pose::IDENTITY };
    let t1 = Instant::now();
    let p = Prepared::new(mesh, Some(pose));
    println!("prepared in {:.2?}", t1.elapsed());
    let look = Look { metal, studio, ..Default::default() };
    let scene = p.scene(&look);
    let v = View::named(&view).unwrap();
    let cam = p.camera(&v, &Lens::default(), w as f32 / h as f32);
    let mut q = Quality { width: w, height: h, spp, ..Default::default() };
    if let Ok(d) = std::env::var("ARGENT_MAXDEPTH") {
        q.path.max_depth = d.parse().unwrap();
    }
    if std::env::var("ARGENT_NODENOISE").is_ok() {
        q.denoise = false;
    }
    let t2 = Instant::now();
    let img = render_image(&scene, &cam, &q, 1)?;
    println!("rendered {}x{} {} spp in {:.2?}", w, h, spp, t2.elapsed());
    argent_post::io::write_png(std::path::Path::new(&out), &argent_post::develop(&img, &grade(&look)))?;
    Ok(())
}
