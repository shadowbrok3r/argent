use super::*;
use crate::synth;
use std::io::Write;

fn opts() -> LoadOptions {
    LoadOptions::default()
}

fn cube_soup_volume(m: &Mesh) -> f64 {
    synth::signed_volume(m)
}

#[test]
fn binary_stl_with_solid_header() {
    let cube = synth::cuboid([2.0, 3.0, 4.0]);
    let bytes = synth::to_binary_stl(&cube);
    assert!(bytes.starts_with(b"solid"));
    assert_eq!(detect_format(&bytes, Some("obj")), Some(Format::StlBinary));
    let (m, r) = load_bytes(&bytes, "cube", Some("stl"), &opts()).unwrap();
    assert_eq!(r.format, Format::StlBinary);
    assert_eq!(r.raw_triangles, 12);
    assert_eq!(m.triangles.len(), 12);
    // 8 welded corners, each split into 3 flat-shaded copies.
    assert_eq!(r.weld_report.as_ref().unwrap().output_vertices, 8);
    assert_eq!(m.positions.len(), 24);
    assert_eq!(m.normals.len(), 24);
    assert_eq!(m.parts.len(), 1);
    assert_eq!(m.parts[0].name, "cube");
    assert!((cube_soup_volume(&m) - 24.0).abs() < 1e-4);
}

#[test]
fn binary_stl_padded_and_scaled() {
    let cube = synth::cuboid([1.0, 1.0, 1.0]);
    let mut bytes = synth::to_binary_stl(&cube);
    bytes.extend_from_slice(&[0u8; 7]);
    // Header without "solid": lenient binary detection by extension.
    bytes[..5].copy_from_slice(b"BINRY");
    assert_eq!(detect_format(&bytes, Some("stl")), Some(Format::StlBinary));
    let o = LoadOptions { unit_scale: 25.4, ..opts() };
    let (m, r) = load_bytes(&bytes, "inch_cube", Some("stl"), &o).unwrap();
    assert!(r.warnings.iter().any(|w| w.contains("trailing")));
    assert!((m.volume() - 25.4f64.powi(3)).abs() / 25.4f64.powi(3) < 1e-5);
}

#[test]
fn ascii_stl_two_solids_and_quad_facet() {
    let text = "solid band\n\
facet normal 0 0 1\n outer loop\n  vertex 0 0 0\n  vertex 1 0 0\n  vertex 1 1 0\n  vertex 0 1 0\n endloop\nendfacet\n\
endsolid band\n\
SOLID Stone 1\n\
FACET NORMAL 0 0 1\n OUTER LOOP\n  VERTEX 0 0 1\n  VERTEX 1 0 1\n  VERTEX 1 1 1e0\n ENDLOOP\nENDFACET\n\
ENDSOLID Stone 1\n";
    let bytes = text.as_bytes();
    assert_eq!(detect_format(bytes, None), Some(Format::StlAscii));
    let (raw, _) = parse(bytes, Format::StlAscii).unwrap();
    assert_eq!(raw.triangles.len(), 3);
    let o = LoadOptions { weld: false, ..opts() };
    let (m, _) = load_bytes(bytes, "file", Some("stl"), &o).unwrap();
    let names: Vec<&str> = m.parts.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["band", "Stone 1"]);
    assert_eq!(m.parts[0].triangles, 0..2);
    assert_eq!(m.parts[1].triangles, 2..3);
}

#[test]
fn ascii_stl_parallel_chunks_match() {
    // > 8 MB of ASCII so the parser splits into several chunks.
    let mut t = synth::torus(10.0, 2.0, 480, 240);
    synth::unweld(&mut t);
    let mut s = String::from("solid big\n");
    for tri in &t.triangles {
        s.push_str("facet normal 0 0 0\nouter loop\n");
        for &i in tri {
            let p = t.positions[i as usize];
            s.push_str(&format!("vertex {:.7e} {:.7e} {:.7e}\n", p[0], p[1], p[2]));
        }
        s.push_str("endloop\nendfacet\n");
    }
    s.push_str("endsolid big\n");
    assert!(s.len() > 16 << 20, "{}", s.len());
    let (m, r) = load_bytes(s.as_bytes(), "big", None, &opts()).unwrap();
    assert_eq!(r.format, Format::StlAscii);
    assert_eq!(m.triangles.len(), 480 * 240 * 2);
    assert_eq!(r.weld_report.unwrap().output_vertices, 480 * 240);
    assert_eq!(r.normal_report.split_vertices, 0);
    assert_eq!(m.parts.len(), 1);
}

const OBJ_TEXT: &str = "# test\nmtllib x.mtl\n\
v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\n\
vt 0 0\nvt 1 0\nvn 0 0 1\n\
o Ring\nusemtl gold\n\
f 1 2 3\n\
f 1/1 3/2 4/1\n\
g setting\nusemtl silver\n\
v 0 0 1\nv 1 0 1\nv 1 1 1\nv 0.5 1.5 1\nv 0 1 1\n\
f -5//1 -4//1 -3//1 -2//1 -1//1\n\
usemtl sapphire\n\
f 5/1/1 6/2/1 \\\n 7/1/1\n";

#[test]
fn obj_faces_negative_ngons_parts() {
    let bytes = OBJ_TEXT.as_bytes();
    assert_eq!(detect_format(bytes, None), Some(Format::Obj));
    let (raw, w) = parse(bytes, Format::Obj).unwrap();
    assert!(w.is_empty(), "{w:?}");
    // 2 tris + pentagon (3 tris) + 1 continued tri.
    assert_eq!(raw.triangles.len(), 6);
    assert_eq!(raw.triangles[2], [4, 5, 6]);
    assert_eq!(raw.triangles[4], [4, 7, 8]);
    assert_eq!(raw.triangles[5], [4, 5, 6]);
    let parts: Vec<(&str, Option<&str>, std::ops::Range<u32>)> =
        raw.parts.iter().map(|p| (p.name.as_str(), p.material.as_deref(), p.triangles.clone())).collect();
    assert_eq!(
        parts,
        vec![("Ring", Some("gold"), 0..2), ("Ring/setting", Some("silver"), 2..5), ("Ring/setting", Some("sapphire"), 5..6)]
    );
    let (m, _) = load_bytes(bytes, "obj", None, &opts()).unwrap();
    assert_eq!(m.parts.len(), 3);
    assert_eq!(m.normals.len(), m.positions.len());
}

#[test]
fn obj_large_parallel_with_negative_indices() {
    let t = synth::torus(10.0, 2.0, 600, 300);
    let mut s = String::with_capacity(16 << 20);
    s.push_str("o torus\n");
    // Emit each quad's vertices right before its faces, referenced negatively,
    // plus shared positive-index faces for the rest.
    for p in &t.positions {
        s.push_str(&format!("v {} {} {}\n", p[0], p[1], p[2]));
    }
    for (i, tri) in t.triangles.iter().enumerate() {
        if i % 2 == 0 {
            s.push_str(&format!("f {} {} {}\n", tri[0] + 1, tri[1] + 1, tri[2] + 1));
        } else {
            let n = t.positions.len() as i64;
            let r = |v: u32| v as i64 - n;
            s.push_str(&format!("f {}/1/1 {}/1/1 {}/1/1\n", r(tri[0]), r(tri[1]), r(tri[2])));
        }
    }
    assert!(s.len() > 8 << 20);
    let (raw, _) = parse(s.as_bytes(), Format::Obj).unwrap();
    assert_eq!(raw.triangles, t.triangles);
    let (m, r) = load_bytes(s.as_bytes(), "torus", Some("obj"), &opts()).unwrap();
    assert_eq!(m.triangles.len(), t.triangles.len());
    assert_eq!(r.normal_report.split_vertices, 0);
    assert_eq!(m.parts[0].name, "torus");
}

fn ply_header(format: &str, nv: usize, nf: usize, coord: &str, list: &str) -> String {
    format!(
        "ply\nformat {format} 1.0\ncomment made by test\nobj_info x\n\
element vertex {nv}\nproperty {coord} x\nproperty {coord} y\nproperty {coord} z\nproperty float nx\nproperty uchar red\n\
element face {nf}\nproperty list {list} vertex_indices\nproperty uchar flags\n\
element edge 1\nproperty int vertex1\nproperty int vertex2\nend_header\n"
    )
}

#[test]
fn ply_ascii_and_binary_variants() {
    let cube = synth::cuboid([2.0, 2.0, 2.0]);
    // ASCII: quads as one polygon per face pair would be nicer, but triangles suffice.
    let mut a = ply_header("ascii", 8, 12, "float", "uchar int");
    for p in &cube.positions {
        a.push_str(&format!("{} {} {} 0.5 255\n", p[0], p[1], p[2]));
    }
    for t in &cube.triangles {
        a.push_str(&format!("3 {} {} {} 7\n", t[0], t[1], t[2]));
    }
    a.push_str("0 1\n");
    let (m, _) = parse(a.as_bytes(), Format::Ply).unwrap();
    assert_eq!(m.triangles, cube.triangles);
    assert_eq!(m.positions, cube.positions);

    for (fmt, be) in [("binary_little_endian", false), ("binary_big_endian", true)] {
        let mut b = ply_header(fmt, 8, 12, "double", "ushort uint").into_bytes();
        let put = |b: &mut Vec<u8>, bytes: &[u8]| {
            let mut v = bytes.to_vec();
            if be {
                v.reverse();
            }
            b.extend_from_slice(&v);
        };
        for p in &cube.positions {
            for c in p {
                put(&mut b, &(*c as f64).to_le_bytes());
            }
            put(&mut b, &0.5f32.to_le_bytes());
            b.push(200);
        }
        for t in &cube.triangles {
            put(&mut b, &3u16.to_le_bytes());
            for &i in t {
                put(&mut b, &i.to_le_bytes());
            }
            b.push(1);
        }
        put(&mut b, &0i32.to_le_bytes());
        put(&mut b, &1i32.to_le_bytes());
        assert_eq!(detect_format(&b, None), Some(Format::Ply));
        let (m, _) = parse(&b, Format::Ply).unwrap();
        assert_eq!(m.triangles, cube.triangles, "{fmt}");
        assert_eq!(m.positions, cube.positions, "{fmt}");
        let (m, _) = load_bytes(&b, "plycube", None, &opts()).unwrap();
        assert!((m.volume() - 8.0).abs() < 1e-4);
    }
}

#[test]
fn ply_vertex_index_name_and_polygons() {
    let text = "ply\r\nformat ascii 1.0\r\nelement vertex 5\r\nproperty float x\r\nproperty float y\r\nproperty float z\r\n\
element face 1\r\nproperty list uchar uint vertex_index\r\nend_header\r\n\
0 0 0\r\n1 0 0\r\n1 1 0\r\n0.5 1.5 0\r\n0 1 0\r\n5 0 1 2 3 4\r\n";
    let (m, _) = parse(text.as_bytes(), Format::Ply).unwrap();
    assert_eq!(m.triangles, vec![[0, 1, 2], [0, 2, 3], [0, 3, 4]]);
}

fn mesh_xml(m: &Mesh, pid: Option<&str>) -> String {
    let mut s = String::from("<mesh><vertices>");
    for p in &m.positions {
        s.push_str(&format!("<vertex x=\"{}\" y=\"{}\" z=\"{}\"/>", p[0], p[1], p[2]));
    }
    s.push_str("</vertices><triangles>");
    for (i, t) in m.triangles.iter().enumerate() {
        match pid {
            Some(p) if i % 2 == 1 => s.push_str(&format!("<triangle v1=\"{}\" v2=\"{}\" v3=\"{}\" {p}/>", t[0], t[1], t[2])),
            _ => s.push_str(&format!("<triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>", t[0], t[1], t[2])),
        }
    }
    s.push_str("</triangles></mesh>");
    s
}

fn zip_bytes(files: &[(&str, String)]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, body) in files {
        w.start_file(*name, o).unwrap();
        w.write_all(body.as_bytes()).unwrap();
    }
    w.finish().unwrap().into_inner()
}

const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#;

#[test]
fn threemf_units_transforms_components_materials() {
    let band = synth::cuboid([1.0, 1.0, 1.0]);
    let gem = synth::cuboid([0.5, 0.5, 0.5]);
    let model = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<model unit="inch" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
 xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06">
 <metadata name="Title">t &amp; t</metadata>
 <resources>
  <basematerials id="9"><base name="Gold &amp; Co" displaycolor="#FFD700"/><base name="Sapphire" displaycolor="#0000FF"/></basematerials>
  <object id="1" name="Band" type="model" pid="9" pindex="0">{}</object>
  <object id="2" name="Gem" type="model">{}</object>
  <object id="3" name="Assembly" type="model">
   <components>
    <component objectid="1"/>
    <component objectid="2" transform="1 0 0 0 1 0 0 0 1 0 0 2"/>
   </components>
  </object>
  <object id="4" type="support">{}</object>
 </resources>
 <build>
  <item objectid="3" transform="1 0 0 0 1 0 0 0 1 10 0 0"/>
  <item objectid="1" transform="-1 0 0 0 1 0 0 0 1 -10 0 0"/>
  <item objectid="4"/>
 </build>
</model>"##,
        mesh_xml(&band, Some("pid=\"9\" p1=\"1\"")),
        mesh_xml(&gem, None),
        mesh_xml(&gem, None)
    );
    let z = zip_bytes(&[
        ("[Content_Types].xml", "<Types/>".into()),
        ("_rels/.rels", RELS.into()),
        ("3D/3dmodel.model", model),
    ]);
    assert_eq!(detect_format(&z, None), Some(Format::ThreeMf));
    let (raw, w) = parse(&z, Format::ThreeMf).unwrap();
    assert!(w.is_empty(), "{w:?}");
    let names: Vec<(&str, Option<&str>)> = raw.parts.iter().map(|p| (p.name.as_str(), p.material.as_deref())).collect();
    assert_eq!(
        names,
        vec![
            ("Band:Gold & Co", Some("Gold & Co")),
            ("Band:Sapphire", Some("Sapphire")),
            ("Gem", None),
            ("Band:Gold & Co", Some("Gold & Co")),
            ("Band:Sapphire", Some("Sapphire")),
        ]
    );
    // Gem placed at z = 2 in, item at x = 10 in → (254, 0, 50.8) mm.
    let gem_part = &raw.parts[2];
    let mut c = [0.0f64; 3];
    let mut n = 0.0;
    for t in &raw.triangles[gem_part.triangles.start as usize..gem_part.triangles.end as usize] {
        for &i in t {
            for k in 0..3 {
                c[k] += raw.positions[i as usize][k] as f64;
            }
            n += 1.0;
        }
    }
    let c = c.map(|v| v / n);
    assert!((c[0] - 254.0).abs() < 1e-3 && c[1].abs() < 1e-3 && (c[2] - 50.8).abs() < 1e-3, "{c:?}");
    // Mirrored band must still be outward: total volume positive and = 2 bands + gem.
    let (m, _) = load_bytes(&z, "asm", None, &opts()).unwrap();
    let inch3 = 25.4f64.powi(3);
    let expect = (1.0 + 1.0 + 0.125) * inch3;
    let v = synth::signed_volume(&m);
    assert!((v - expect).abs() / expect < 1e-4, "{v} vs {expect}");
}

#[test]
fn threemf_production_external_model() {
    let gem = synth::cuboid([1.0, 1.0, 1.0]);
    let root = r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
 xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06">
 <resources>
  <object id="5" name="Wrapper"><components><component objectid="7" p:path="/3D/Objects/gem.model" transform="2 0 0 0 2 0 0 0 2 0 0 0"/></components></object>
 </resources>
 <build><item objectid="5"/></build>
</model>"#;
    let ext = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
 <resources><object id="7" name="stone">{}</object></resources><build/>
</model>"#,
        mesh_xml(&gem, None)
    );
    let z = zip_bytes(&[("_rels/.rels", RELS.into()), ("3D/3dmodel.model", root.into()), ("3D/Objects/gem.model", ext)]);
    let (m, _) = load_bytes(&z, "x", Some("3mf"), &opts()).unwrap();
    assert_eq!(m.parts.len(), 1);
    // single part → file stem, material none
    assert_eq!(m.parts[0].name, "x");
    assert!((m.volume() - 8.0).abs() < 1e-4);
    let (raw, _) = parse(&z, Format::ThreeMf).unwrap();
    assert_eq!(raw.parts[0].name, "stone");
}

#[test]
fn load_from_disk_names_part_by_stem() {
    let dir = std::env::temp_dir().join(format!("argent-mesh-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("signet_ring.weird");
    let mut s = synth::icosphere(5.0, 2);
    synth::unweld(&mut s);
    std::fs::write(&path, synth::to_binary_stl(&s)).unwrap();
    let m = crate::load(&path, &opts()).unwrap();
    assert_eq!(m.parts[0].name, "signet_ring");
    let many = crate::load_many(&[path.clone(), path.clone()], &opts()).unwrap();
    assert_eq!(many.parts.len(), 2);
    assert_eq!(many.triangles.len(), 2 * m.triangles.len());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn garbage_is_rejected() {
    assert!(detect_format(b"hello world, not a mesh at all\x00\x01", None).is_none());
    assert!(load_bytes(b"solid x\nendsolid x\n", "e", Some("stl"), &opts()).is_err());
}
