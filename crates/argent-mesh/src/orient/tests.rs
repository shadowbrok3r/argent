use super::*;
use crate::synth;

fn deg(a: f32) -> f32 {
    a.to_radians().cos()
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn mat_vec(r: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [dot3(r[0], v), dot3(r[1], v), dot3(r[2], v)]
}

fn shade(mut m: Mesh) -> Mesh {
    crate::normals::compute(&mut m, 50.0);
    m
}

/// Torus band (ID 16.4 mm) + box head at +Y; optional "face" bumps on the
/// head's +Z side.
fn signet(face_bumps: bool) -> Mesh {
    let band = synth::torus(9.5, 1.3, 256, 48);
    let mut head = synth::subdivided_cuboid([9.0, 4.0, 7.0], 16);
    synth::translate(&mut head, [0.0, 11.2, 0.0]);
    let mut parts = vec![band, head];
    if face_bumps {
        for (x, y) in [(-2.5, 10.5), (0.0, 10.5), (2.5, 10.5), (-1.5, 12.2), (1.5, 12.2), (0.0, 11.4)] {
            let mut b = synth::icosphere(0.8, 3);
            synth::translate(&mut b, [x, y, 3.5]);
            parts.push(b);
        }
    }
    shade(synth::combine(parts, "ring"))
}

fn check_pose(o: &Orientation, rot: [[f32; 3]; 3], t: [f32; 3], up: [f32; 3], front: [f32; 3], tol_deg: f32) {
    let y = o.pose.apply_vector(mat_vec(rot, up));
    let z = o.pose.apply_vector(mat_vec(rot, front));
    assert!(y[1] > deg(tol_deg), "up maps to {y:?}\n{:#?}", o.notes);
    assert!(z[2] > deg(tol_deg), "front maps to {z:?}\n{:#?}", o.notes);
    let r = o.pose.rotation;
    let det = r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1]) - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
        + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0]);
    assert!((det - 1.0).abs() < 1e-4, "det {det}");
    let _ = t;
}

#[test]
fn signet_with_face_recovered_under_rotation() {
    for (i, (axis, angle, t)) in [
        ([0.3f32, -0.5, 0.8], 1.1f32, [5.0f32, -3.0, 12.0]),
        ([1.0, 0.2, 0.0], 2.3, [-40.0, 7.0, 1.0]),
        ([0.0, 1.0, 0.0], 1.5707964, [0.0, 0.0, 0.0]),
    ]
    .into_iter()
    .enumerate()
    {
        let mut m = signet(true);
        let rot = synth::rotation(axis, angle);
        synth::transform(&mut m, rot, t);
        let o = detect(&m);
        assert_eq!(o.kind, PieceKind::Ring, "case {i}: {:#?}", o.notes);
        let br = o.bore_radius.unwrap();
        assert!((br - 8.2).abs() < 0.15, "case {i}: bore radius {br}");
        check_pose(&o, rot, t, [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], 3.0);
        let c = o.pose.apply_point(t);
        assert!(dot3(c, c).sqrt() < 0.25, "case {i}: centre {c:?}\n{:#?}", o.notes);
        assert!(o.confidence > 0.6, "case {i}: confidence {}", o.confidence);
    }
}

#[test]
fn signet_face_flipped_follows_bumps() {
    let mut m = signet(true);
    // Mirror the bumps to the -Z side by rotating the whole ring 180° about Y:
    // the face then looks along mesh -Z, which must still map to +Z.
    let rot = synth::rotation([0.0, 1.0, 0.0], std::f32::consts::PI);
    synth::transform(&mut m, rot, [0.0; 3]);
    let o = detect(&m);
    assert_eq!(o.kind, PieceKind::Ring);
    check_pose(&o, rot, [0.0; 3], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], 3.0);
}

#[test]
fn plain_band_is_a_ring() {
    let mut m = shade(synth::torus(9.0, 1.0, 256, 32));
    let rot = synth::rotation([0.6, 0.0, 0.8], 0.7);
    synth::transform(&mut m, rot, [1.0, 2.0, 3.0]);
    let o = detect(&m);
    assert_eq!(o.kind, PieceKind::Ring, "{:#?}", o.notes);
    assert!((o.bore_radius.unwrap() - 8.0).abs() < 0.12);
    let z = o.pose.apply_vector(mat_vec(rot, [0.0, 0.0, 1.0]));
    assert!(z[2].abs() > deg(2.0), "{z:?}");
    assert!(o.notes.iter().any(|n| n.contains("plain band")), "{:#?}", o.notes);
}

fn pendant() -> Mesh {
    let disc = synth::cylinder(12.0, 2.0, 160);
    let mut bail = synth::torus(2.2, 0.7, 96, 24);
    synth::translate(&mut bail, [0.0, 13.6, 0.0]);
    let mut parts = vec![disc, bail];
    for (x, y, r) in [(0.0, 0.0, 2.0), (5.0, 3.0, 1.2), (-5.0, 3.0, 1.2), (4.0, -5.0, 1.5), (-4.0, -5.0, 1.5), (0.0, 7.0, 1.0)] {
        let mut b = synth::icosphere(r, 3);
        synth::translate(&mut b, [x, y, 1.0]);
        parts.push(b);
    }
    shade(synth::combine(parts, "pendant"))
}

#[test]
fn pendant_bail_up_relief_front() {
    for (axis, angle) in [([0.2f32, 0.9, -0.3], 0.8f32), ([1.0, 0.0, 0.0], std::f32::consts::PI), ([0.0, 0.0, 1.0], 2.0)] {
        let mut m = pendant();
        let rot = synth::rotation(axis, angle);
        let t = [3.0, -8.0, 2.0];
        synth::transform(&mut m, rot, t);
        let o = detect(&m);
        assert_eq!(o.kind, PieceKind::Pendant, "{:#?}", o.notes);
        check_pose(&o, rot, t, [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], 4.0);
        assert!(o.notes.iter().any(|n| n.starts_with("bail")), "{:#?}", o.notes);
    }
}

#[test]
fn pendant_without_bail_points_narrow_end_up() {
    // A teardrop-ish shape: wide disc with a small disc above it, no hole.
    let big = synth::cylinder(10.0, 2.0, 96);
    let mut small = synth::cylinder(4.0, 2.0, 64);
    synth::translate(&mut small, [0.0, 11.0, 0.0]);
    let mut m = shade(synth::combine(vec![big, small], "drop"));
    let rot = synth::rotation([0.3, 0.3, 0.9], 2.5);
    synth::transform(&mut m, rot, [0.0; 3]);
    let o = detect(&m);
    assert_eq!(o.kind, PieceKind::Pendant, "{:#?}", o.notes);
    let y = o.pose.apply_vector(mat_vec(rot, [0.0, 1.0, 0.0]));
    assert!(y[1] > deg(5.0), "{y:?} {:#?}", o.notes);
}

#[test]
fn chunky_box_is_other_with_principal_axes() {
    let mut m = shade(synth::subdivided_cuboid([10.0, 20.0, 8.0], 8));
    let rot = synth::rotation([0.5, -0.2, 0.4], 0.9);
    synth::transform(&mut m, rot, [4.0, 4.0, 4.0]);
    let o = detect(&m);
    assert_eq!(o.kind, PieceKind::Other, "{:#?}", o.notes);
    let y = o.pose.apply_vector(mat_vec(rot, [0.0, 1.0, 0.0]));
    let z = o.pose.apply_vector(mat_vec(rot, [0.0, 0.0, 1.0]));
    assert!(y[1].abs() > deg(2.0) && z[2].abs() > deg(2.0), "{y:?} {z:?}");
    let c = o.pose.apply_point([4.0, 4.0, 4.0]);
    assert!(dot3(c, c).sqrt() < 1e-3);
}

#[test]
fn flat_washer_is_not_a_ring() {
    // Ø 14 hole in a 40 mm, 2 mm thick disc: a donut pendant, not a ring.
    let prof = |v: f32| {
        // rectangle (radius 7..20, z -1..1) traversed CCW
        let t = v / std::f32::consts::TAU * 4.0;
        let (r0, r1, z0, z1) = (7.0f32, 20.0f32, -1.0f32, 1.0f32);
        match t as u32 {
            0 => (r0 + (r1 - r0) * t, z0),
            1 => (r1, z0 + (z1 - z0) * (t - 1.0)),
            2 => (r1 - (r1 - r0) * (t - 2.0), z1),
            _ => (r0, z1 - (z1 - z0) * (t - 3.0)),
        }
    };
    let m = shade(synth::torus_profile(256, 64, prof));
    let o = detect(&m);
    assert_ne!(o.kind, PieceKind::Ring, "{:#?}", o.notes);
}

/// Skull ring from the NIH skull test asset (if present): the skull's face
/// (+Y in its file, cranium top +Z) is set on a band to look along the
/// finger. Detection must put the head at +Y and the face at +Z, both for
/// the face toward +Z and toward −Z, under arbitrary rotation.
#[test]
fn skull_ring_face_follows_the_face() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/test/vhm_skull_nih3d_3DPX-012260.ply");
    if !path.exists() {
        eprintln!("skipping: {} not present", path.display());
        return;
    }
    let opts = crate::LoadOptions { unit_scale: 4.0, ..Default::default() };
    let skull = crate::load(&path, &opts).unwrap();
    let (lo, hi) = skull.bounds();
    let centre = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
    for face_dir in [1.0f32, -1.0] {
        // Skull frame → ring frame: top (+Z) → +Y, face (+Y) → face_dir·Z.
        let to_ring = if face_dir > 0.0 {
            [[-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]]
        } else {
            [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]]
        };
        let mut head = skull.clone();
        synth::translate(&mut head, [-centre[0], -centre[1], -centre[2]]);
        synth::transform(&mut head, to_ring, [0.0, 10.0 + 0.5 * (hi[2] - lo[2]) - 1.0, 0.0]);
        let band = synth::torus(10.0, 1.6, 384, 48);
        let mut ring = band;
        ring.merge(head);
        let mut ring = shade(ring);
        let rot = synth::rotation([0.4, 0.7, -0.2], 2.0);
        let t = [-3.0, 5.0, 7.0];
        synth::transform(&mut ring, rot, t);
        let o = detect(&ring);
        eprintln!("skull ring, face {face_dir:+}: {:#?}", o.notes);
        if let Some(dir) = std::env::var_os("ARGENT_DUMP_DIR") {
            let p = std::path::Path::new(&dir).join(format!("skull_ring_{}.stl", if face_dir > 0.0 { "pos" } else { "neg" }));
            std::fs::write(p, synth::to_binary_stl(&ring)).unwrap();
        }
        assert_eq!(o.kind, PieceKind::Ring, "{:#?}", o.notes);
        assert!((o.bore_radius.unwrap() - 8.4).abs() < 0.15, "{:?}", o.bore_radius);
        let y = o.pose.apply_vector(mat_vec(rot, [0.0, 1.0, 0.0]));
        let z = o.pose.apply_vector(mat_vec(rot, [0.0, 0.0, face_dir]));
        assert!(y[1] > deg(4.0), "face_dir {face_dir}: up maps to {y:?}\n{:#?}", o.notes);
        assert!(z[2] > deg(2.0), "face_dir {face_dir}: face maps to {z:?}\n{:#?}", o.notes);
        assert!(o.notes.iter().any(|n| n.starts_with("face toward")), "{:#?}", o.notes);
    }
}
