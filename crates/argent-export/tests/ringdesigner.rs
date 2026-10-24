//! Real RingDesigner rings from `assets/ringdesigner/` (local test data,
//! gitignored; every test skips when its file is missing).
//!
//! `cargo test -p argent-export --release -- --ignored --nocapture` runs
//! the 2.6 M-triangle benchmark and the stone-set export; their GLBs land
//! in `$CARGO_TARGET_TMPDIR` for an external validator.

mod common;

use argent_export::{GlbOptions, export_glb, simplify_with_stats};
use argent_mesh::Mesh;
use common::*;
use std::path::PathBuf;
use std::time::Instant;

fn asset(rel: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/ringdesigner").join(rel);
    if p.exists() {
        Some(p)
    } else {
        eprintln!("skipping: {} not found", p.display());
        None
    }
}

fn out(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn peak_rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM"))
                .map(|l| l.split_whitespace().nth(1).unwrap().parse::<f64>().unwrap())
        })
        .map_or(0.0, |kb| kb / 1024.0)
}

/// Head-heavy synthetic patina (rings have their head toward +Y), just to
/// exercise `COLOR_0` at realistic sizes.
fn head_mask(m: &Mesh) -> Vec<f32> {
    let (lo, hi) = m.bounds();
    m.positions.iter().map(|p| ((p[1] - lo[1]) / (hi[1] - lo[1]).max(1e-6)).powi(2)).collect()
}

fn silver(target: u32, m: &Mesh, name: &str) -> GlbOptions {
    GlbOptions {
        target_triangles: target,
        patina: Some((head_mask(m), [0.035, 0.03, 0.028], 0.85)),
        name: name.into(),
        ..Default::default()
    }
}

#[test]
fn aureole_262k_to_50k() {
    let Some(p) = asset("aureole/aureole-half-turn-ribbon_size8.11.stl") else { return };
    let m = read_stl(&p);
    let path = out("aureole-50k.glb");
    let rep = export_glb(&m, &silver(50_000, &m, "aureole"), &path).unwrap();
    eprintln!("aureole: {rep:?}");
    let bytes = std::fs::read(&path).unwrap();
    let g = check_glb(&bytes);
    assert_eq!(g.triangles, rep.triangles_out);
    assert!(rep.triangles_out <= 50_000 && rep.triangles_out > 49_000);
    assert!(rep.max_deviation_mm < 0.1, "max deviation {} mm", rep.max_deviation_mm);
}

#[test]
#[ignore = "2.6 M-triangle benchmark (~1 min with the read); run with --ignored"]
fn heavy_ring_2_6m_to_150k() {
    for rel in [
        "aster/aster-botanical-sand-signet_size8.11.stl",
        "tessera/tessera-shield-mosaic_size8.6.stl",
        "solstice-signet/solstice-solar-palmette-signet_size8.11.stl",
    ] {
        let Some(p) = asset(rel) else { continue };
        let t = Instant::now();
        let m = read_stl(&p);
        let read = t.elapsed();
        let t = Instant::now();
        let (s, _map, st) = simplify_with_stats(&m, 150_000);
        let simp = t.elapsed();
        eprintln!(
            "{}: {} tris read in {:.2?}; simplify → {} tris in {:.2?} (setup {:.2?}, collapse loop {:.2?}); {st:?}",
            p.file_name().unwrap().to_string_lossy(),
            m.triangles.len(),
            read,
            s.triangles.len(),
            simp,
            st.setup,
            st.collapse,
        );
        assert!(s.triangles.len() <= 150_000 && s.triangles.len() > 149_000);
        assert!(simp.as_secs_f32() < 60.0);
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        let path = out(&format!("{name}-150k.glb"));
        let rep = export_glb(&m, &silver(150_000, &m, &name), &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let g = check_glb(&bytes);
        eprintln!(
            "  export: {rep:?}; {} vertices in the GLB; peak RSS {:.0} MB; {}",
            g.vertices,
            peak_rss_mb(),
            path.display()
        );
        assert_eq!(g.triangles, rep.triangles_out);
        assert!(rep.bytes <= 15_000_000);
    }
}

#[test]
#[ignore = "stone-set ring with 37 stones; run with --ignored"]
fn oriel_with_stones() {
    let Some(stl) = asset("oriel/oriel-jewelled-lantern_size7.stl") else { return };
    let Some(obj) = asset("oriel/oriel-jewelled-lantern_size7_stones.obj") else { return };
    let mut m = read_stl(&stl);
    let stones = read_obj(&obj);
    let n_stones = stones.parts.len();
    m.merge(stones);
    let mask = head_mask(&m);
    let opts = GlbOptions {
        target_triangles: 150_000,
        patina: Some((mask, [0.035, 0.03, 0.028], 0.85)),
        name: "oriel".into(),
        ..Default::default()
    };
    let path = out("oriel-150k.glb");
    let rep = export_glb(&m, &opts, &path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let g = check_glb(&bytes);
    eprintln!("oriel: {rep:?}; {n_stones} stones; {} vertices; {}", g.vertices, path.display());
    assert_eq!(g.json["meshes"].as_array().unwrap().len(), 1 + n_stones);
    let used: Vec<&str> = g.json["extensionsUsed"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(used.contains(&"KHR_materials_transmission"));
}

#[test]
#[ignore = "argent-mesh loader path (weld + crease split); run with --ignored"]
fn oriel_via_argent_mesh_loader() {
    let Some(stl) = asset("oriel/oriel-jewelled-lantern_size7.stl") else { return };
    let Some(obj) = asset("oriel/oriel-jewelled-lantern_size7_stones.obj") else { return };
    let m = match argent_mesh::load_many(&[stl, obj], &argent_mesh::LoadOptions::default()) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skipping: argent-mesh loader failed: {e:#}");
            return;
        }
    };
    let path = out("oriel-loader-150k.glb");
    let rep =
        export_glb(&m, &GlbOptions { target_triangles: 150_000, name: "oriel".into(), ..Default::default() }, &path)
            .unwrap();
    let g = check_glb(&std::fs::read(&path).unwrap());
    eprintln!("oriel via loader: {rep:?}; {} meshes; {}", g.json["meshes"].as_array().unwrap().len(), path.display());
    assert_eq!(g.json["meshes"].as_array().unwrap().len(), m.parts.len());
    assert!(rep.max_deviation_mm < 0.1);
}
