//! Automatic pose.
//!
//! Method (all on an area-uniform point sample of the surface, ~20k–300k
//! points, so cost is independent of triangle count):
//! - **Ring**: for ~330 candidate axes (a Fibonacci hemisphere plus the
//!   principal and mesh axes) the projected silhouette is rasterised on a
//!   256² grid and flood-filled from the border; enclosed empty regions are
//!   holes. The finger axis is the view that maximises the largest hole
//!   (Ø 7–90 mm), refined by pattern search. Then in 3D, on the innermost
//!   inward-facing surface near the hole radius: every normal line of a
//!   surface of revolution meets its axis, so axis and centre come from a
//!   robust (Tukey) Gauss–Newton fit of those normal lines (σ ≈ 0 for a true
//!   bore). An axis within 0.5° of a mesh axis (or 8° when σ shows the bore
//!   is not a surface of revolution) is snapped to it: authored frames stay
//!   exact. Bore radius = narrowest distance (1st percentile); axial centre =
//!   mid-plane of the shank's bore.
//!   Head = peak of the angular profile of enclosed volume (signed tetrahedra
//!   from the axis), refined to the best mirror plane through the axis
//!   (voxel occupancy test), snapped to a mesh axis within 10° in authored
//!   frames. Face sign along the finger = the side of the head with more
//!   visible surface detail (∫|κ| from the vertex normals, ratio ≥ 1.35): a
//!   skull's face beats its smooth cranium; otherwise the file's sense.
//! - **Pendant**: thinnest principal axis → Z; a small enclosed hole near the
//!   rim in the front (or side) view, preferably on the long axis, is the
//!   bail → +Y (fallback: narrower end up); the side whose visible surface
//!   has more varied normals and curvature (relief) faces +Z.
//! - **Other**: principal axes, longest along Y, thinnest along Z.
//!
//! Set `ARGENT_ORIENT_DEBUG=1` for a trace of the ring search on stderr.

use crate::util::{hash32, sym_eigen3, unit_f32};
use crate::Mesh;
use glam::{DVec3, Vec3};
use rayon::prelude::*;

/// What the piece is, as far as geometry can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieceKind {
    Ring,
    Pendant,
    Other,
}

/// Canonical frame (millimetres, right-handed, +Y up):
/// - Ring: finger axis along ±Z, the head (signet table, skull, setting —
///   the side of the band carrying the most mass outside the bore) toward +Y,
///   bore centre at the origin. If the head has a "face" that looks along the
///   finger axis (a skull's face, an animal head), it looks toward +Z.
/// - Pendant: thinnest extent along Z with the decorated front toward +Z,
///   bail/loop (a small hole near one end of the longest axis) toward +Y,
///   bounding-box centre at the origin.
/// - Other: principal axes, longest along Y, thinnest along Z.
#[derive(Clone, Debug)]
pub struct Orientation {
    pub kind: PieceKind,
    pub pose: crate::Pose,
    /// Ring inner radius in mm, when a bore was found.
    pub bore_radius: Option<f32>,
    /// 0..1, how sure the classification and facing are.
    pub confidence: f32,
    /// Human-readable reasoning, for `argent info`.
    pub notes: Vec<String>,
}

/// Classify and pose a mesh. Never fails: falls back to principal axes.
pub fn detect(mesh: &crate::Mesh) -> Orientation {
    let (lo, hi) = mesh.bounds();
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
    let fallback = |note: &str| Orientation {
        kind: PieceKind::Other,
        pose: crate::Pose { center: c, ..crate::Pose::IDENTITY },
        bore_radius: None,
        confidence: 0.0,
        notes: vec![note.to_string()],
    };
    if mesh.triangles.is_empty() {
        return fallback("empty mesh: centred only");
    }
    let diag = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt() as f64;
    if !(diag.is_finite() && diag > 0.0) {
        return fallback("degenerate bounds: centred only");
    }
    let area = mesh.area();
    let spacing = 0.6 * diag / 248.0;
    let target = (area / (spacing * spacing)).clamp(20_000.0, 300_000.0) as usize;
    let s = Samples::new(mesh, target);
    if s.p.len() < 100 {
        return fallback("too little surface area to analyse: centred only");
    }
    let pca = Pca::of(&s);
    let mut notes = Vec::new();
    if let Some(o) = detect_ring(mesh, &s, &pca, &mut notes) {
        return o;
    }
    detect_flat(mesh, &s, &pca, notes)
}

fn debug() -> bool {
    std::env::var_os("ARGENT_ORIENT_DEBUG").is_some()
}

/// Ring-size helper: US size from an inner diameter in mm (US 0 = 11.63 mm,
/// +0.8128 mm per size).
pub fn us_ring_size(inner_diameter_mm: f32) -> f32 {
    (inner_diameter_mm - 11.63) / 0.8128
}

// ---------------------------------------------------------------------------
// Surface sample

struct Samples {
    p: Vec<Vec3>,
    /// Face normal of the source triangle.
    n: Vec<Vec3>,
    /// Interpolated vertex normal (face normal when the mesh has none).
    ns: Vec<Vec3>,
    t: Vec<u32>,
    /// Mean distance between neighbouring samples, mm.
    spacing: f64,
}

impl Samples {
    /// Systematic area-proportional sampling: deterministic, stratified along
    /// the triangle order, so large CAD triangles get many samples.
    fn new(mesh: &Mesh, target: usize) -> Samples {
        let areas: Vec<f64> = mesh
            .triangles
            .par_iter()
            .with_min_len(8192)
            .map(|t| {
                let [a, b, c] = t.map(|i| Vec3::from(mesh.positions[i as usize]).as_dvec3());
                0.5 * (b - a).cross(c - a).length()
            })
            .collect();
        let total: f64 = areas.iter().sum();
        let mut out =
            Samples { p: Vec::with_capacity(target + 16), n: Vec::with_capacity(target + 16), ns: Vec::new(), t: Vec::new(), spacing: 0.0 };
        let smooth = mesh.normals.len() == mesh.positions.len();
        if total.is_nan() || total <= 0.0 {
            return out;
        }
        let step = total / target as f64;
        out.spacing = step.sqrt();
        let mut next = 0.5 * step;
        let mut cum = 0.0;
        let mut k: u32 = 0;
        for (ti, &a) in areas.iter().enumerate() {
            let end = cum + a;
            if next < end {
                let t = mesh.triangles[ti];
                let [p0, p1, p2] = t.map(|i| Vec3::from(mesh.positions[i as usize]));
                let n = (p1 - p0).cross(p2 - p0).normalize_or_zero();
                while next < end {
                    let r1 = unit_f32(hash32(k.wrapping_mul(2).wrapping_add(0x9e37)));
                    let r2 = unit_f32(hash32(k.wrapping_mul(2).wrapping_add(0x7f4a_7c15)));
                    let sr = r1.sqrt();
                    let (w0, w1, w2) = (1.0 - sr, sr * (1.0 - r2), sr * r2);
                    let p = p0 * w0 + p1 * w1 + p2 * w2;
                    out.p.push(p);
                    out.n.push(n);
                    let ns = if smooth {
                        let [n0, n1, n2] = t.map(|i| Vec3::from(mesh.normals[i as usize]));
                        (n0 * w0 + n1 * w1 + n2 * w2).try_normalize().unwrap_or(n)
                    } else {
                        n
                    };
                    out.ns.push(ns);
                    out.t.push(ti as u32);
                    k = k.wrapping_add(1);
                    next += step;
                }
            }
            cum = end;
        }
        out
    }
}

struct Pca {
    mean: DVec3,
    /// Descending variance.
    axes: [DVec3; 3],
    var: [f64; 3],
}

impl Pca {
    fn of(s: &Samples) -> Pca {
        let n = s.p.len() as f64;
        let mean = s.p.iter().fold(DVec3::ZERO, |a, p| a + p.as_dvec3()) / n;
        let mut m = [[0.0f64; 3]; 3];
        for p in &s.p {
            let d = p.as_dvec3() - mean;
            let d = [d.x, d.y, d.z];
            for i in 0..3 {
                for j in 0..3 {
                    m[i][j] += d[i] * d[j];
                }
            }
        }
        for row in &mut m {
            for v in row.iter_mut() {
                *v /= n;
            }
        }
        let (var, vecs) = sym_eigen3(m);
        Pca { mean, axes: vecs.map(DVec3::from_array), var }
    }
}

/// Orthonormal basis (u, v) ⟂ n (Duff et al. 2017).
fn basis(n: DVec3) -> (DVec3, DVec3) {
    let sign = 1.0f64.copysign(n.z);
    let a = -1.0 / (sign + n.z);
    let b = n.x * n.y * a;
    (DVec3::new(1.0 + sign * n.x * n.x * a, sign * b, -sign * n.x), DVec3::new(b, sign + n.y * n.y * a, -n.y))
}

fn fibonacci_hemisphere(n: usize) -> Vec<DVec3> {
    let ga = std::f64::consts::PI * (3.0 - 5.0f64.sqrt());
    (0..n)
        .map(|i| {
            let z = (i as f64 + 0.5) / n as f64;
            let r = (1.0 - z * z).sqrt();
            let phi = ga * i as f64;
            DVec3::new(r * phi.cos(), r * phi.sin(), z)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Silhouette raster and holes

struct Raster {
    g: usize,
    cell: f64,
    x0: f64,
    y0: f64,
    u: DVec3,
    v: DVec3,
    /// 0 = empty, 1 = solid (after a 1-cell dilation).
    occ: Vec<u8>,
}

#[derive(Clone, Debug)]
struct Hole {
    cells: usize,
    /// Area in mm², corrected for the dilation (≈ +1 cell of radius).
    area: f64,
    /// Centroid in plane coordinates (mm along u, v).
    cx: f64,
    cy: f64,
}

impl Hole {
    fn diameter(&self) -> f64 {
        2.0 * (self.area / std::f64::consts::PI).sqrt()
    }
}

impl Raster {
    fn new(s: &Samples, axis: DVec3, g: usize) -> Raster {
        let (u, v) = basis(axis.normalize());
        let (uf, vf) = (u.as_vec3(), v.as_vec3());
        let (mut x_lo, mut x_hi, mut y_lo, mut y_hi) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in &s.p {
            let x = p.dot(uf);
            let y = p.dot(vf);
            x_lo = x_lo.min(x);
            x_hi = x_hi.max(x);
            y_lo = y_lo.min(y);
            y_hi = y_hi.max(y);
        }
        let ext = ((x_hi - x_lo).max(y_hi - y_lo) as f64).max(1e-6);
        let cell = ext / (g - 8) as f64;
        let x0 = 0.5 * (x_lo + x_hi) as f64 - cell * g as f64 * 0.5;
        let y0 = 0.5 * (y_lo + y_hi) as f64 - cell * g as f64 * 0.5;
        let mut occ = vec![0u8; g * g];
        let inv = 1.0 / cell;
        for p in &s.p {
            let ix = ((p.dot(uf) as f64 - x0) * inv) as usize;
            let iy = ((p.dot(vf) as f64 - y0) * inv) as usize;
            if ix < g && iy < g {
                occ[iy * g + ix] = 1;
            }
        }
        let mut d = occ.clone();
        for iy in 1..g - 1 {
            for ix in 1..g - 1 {
                if occ[iy * g + ix] == 1 {
                    for jy in iy - 1..=iy + 1 {
                        d[jy * g + ix - 1] = 1;
                        d[jy * g + ix] = 1;
                        d[jy * g + ix + 1] = 1;
                    }
                }
            }
        }
        Raster { g, cell, x0, y0, u, v, occ: d }
    }

    fn center_of(&self, ix: usize, iy: usize) -> (f64, f64) {
        (self.x0 + (ix as f64 + 0.5) * self.cell, self.y0 + (iy as f64 + 0.5) * self.cell)
    }

    /// Enclosed empty regions, largest first; also returns the solid-cell
    /// centroid and count.
    fn holes(&self) -> (Vec<Hole>, (f64, f64), usize) {
        let g = self.g;
        let mut lab = self.occ.clone();
        let mut stack: Vec<usize> = Vec::new();
        for i in 0..g {
            for c in [i, (g - 1) * g + i, i * g, i * g + g - 1] {
                if lab[c] == 0 {
                    lab[c] = 2;
                    stack.push(c);
                }
            }
        }
        let flood = |lab: &mut Vec<u8>, stack: &mut Vec<usize>, mark: u8, acc: &mut dyn FnMut(usize)| {
            while let Some(c) = stack.pop() {
                acc(c);
                let (x, y) = (c % g, c / g);
                if x > 0 && lab[c - 1] == 0 {
                    lab[c - 1] = mark;
                    stack.push(c - 1);
                }
                if x + 1 < g && lab[c + 1] == 0 {
                    lab[c + 1] = mark;
                    stack.push(c + 1);
                }
                if y > 0 && lab[c - g] == 0 {
                    lab[c - g] = mark;
                    stack.push(c - g);
                }
                if y + 1 < g && lab[c + g] == 0 {
                    lab[c + g] = mark;
                    stack.push(c + g);
                }
            }
        };
        flood(&mut lab, &mut stack, 2, &mut |_| {});
        let mut holes = Vec::new();
        let (mut sx, mut sy, mut sn) = (0.0, 0.0, 0usize);
        for c in 0..g * g {
            match lab[c] {
                1 => {
                    let (x, y) = self.center_of(c % g, c / g);
                    sx += x;
                    sy += y;
                    sn += 1;
                }
                0 => {
                    lab[c] = 3;
                    stack.push(c);
                    let (mut hx, mut hy, mut hn) = (0.0, 0.0, 0usize);
                    flood(&mut lab, &mut stack, 3, &mut |q| {
                        let (x, y) = (q % g, q / g);
                        hx += x as f64;
                        hy += y as f64;
                        hn += 1;
                    });
                    let (cx, cy) = self.center_of(0, 0);
                    let r = (hn as f64 / std::f64::consts::PI).sqrt() + 1.0;
                    holes.push(Hole {
                        cells: hn,
                        area: std::f64::consts::PI * r * r * self.cell * self.cell,
                        cx: cx + hx / hn as f64 * self.cell,
                        cy: cy + hy / hn as f64 * self.cell,
                    });
                }
                _ => {}
            }
        }
        holes.sort_by_key(|h| std::cmp::Reverse(h.cells));
        let centroid = if sn > 0 { (sx / sn as f64, sy / sn as f64) } else { (0.0, 0.0) };
        (holes, centroid, sn)
    }
}

/// Largest bore-sized hole area seen along `axis`.
fn bore_score(s: &Samples, axis: DVec3) -> Option<f64> {
    let r = Raster::new(s, axis, 256);
    let (holes, _, _) = r.holes();
    holes.into_iter().map(|h| h.area).find(|_| true).filter(|a| {
        let d = 2.0 * (a / std::f64::consts::PI).sqrt();
        (7.0..=90.0).contains(&d)
    })
}

// ---------------------------------------------------------------------------
// Ring

struct Bore {
    axis: DVec3,
    center: DVec3,
    /// Narrowest inner radius (1st percentile of bore-surface distances).
    radius: f64,
    coverage: f64,
    roundness: f64,
    samples: usize,
    /// Robust RMS distance of bore normal lines from the axis (mm): ~0 for
    /// a true surface of revolution.
    sigma: f64,
    /// Degrees the fitted axis was rotated to land on an exact mesh axis.
    snapped_deg: Option<f64>,
    /// (angle around the axis, height along it) of the final bore samples.
    bore_pts: Vec<(f64, f64)>,
}

fn refine_bore(s: &Samples, a0: DVec3, c0: DVec3, r0: f64) -> Option<Bore> {
    let mut a = a0.normalize();
    let mut c = c0;
    let mut r = r0;
    // Inward-facing samples in the radial window, then only the innermost
    // surface per angular bin (internal faces of overlapping shells and the
    // undersides of heads lie further out).
    let select = |a: DVec3, c: DVec3, lo: f64, hi: f64| -> Vec<usize> {
        const NB: usize = 90;
        let (u, v) = basis(a);
        let cand: Vec<(usize, f64, usize)> = (0..s.p.len())
            .filter_map(|i| {
                let d = s.p[i].as_dvec3() - c;
                let q = d - a * d.dot(a);
                let rho = q.length();
                if rho > lo && rho < hi && s.n[i].as_dvec3().dot(q) < -0.4 * rho {
                    let b = (((q.dot(v).atan2(q.dot(u)) / std::f64::consts::TAU) + 1.0).fract() * NB as f64) as usize % NB;
                    Some((i, rho, b))
                } else {
                    None
                }
            })
            .collect();
        let mut bmin = [f64::MAX; NB];
        for &(_, rho, b) in &cand {
            bmin[b] = bmin[b].min(rho);
        }
        let allow = (0.06 * lo.max(1.0)).max(0.5);
        cand.into_iter().filter(|&(_, rho, b)| rho <= bmin[b] + allow).map(|(i, _, _)| i).collect()
    };
    let mut sigma = f64::MAX;
    for iter in 0..5 {
        let (lo, hi) = if iter == 0 { (0.7 * r, 1.25 * r + 0.3) } else { (r - (0.1 * r).max(0.6), r + (0.12 * r).max(0.9)) };
        let sel = select(a, c, lo, hi);
        if sel.len() < 40 {
            return None;
        }
        let (na, nc, sg) = fit_axis_normal_lines(s, &sel, a, c);
        sigma = sg;
        if debug() {
            eprintln!("[orient] bore iter {iter}: {} samples, axis {na:.5} (Δ {:.3}° from silhouette), σ {sg:.4} mm", sel.len(), na.dot(a0).clamp(-1.0, 1.0).acos().to_degrees());
        }
        if na.dot(a0) < (25f64).to_radians().cos() {
            return None;
        }
        a = na;
        c = nc;
        let mut rhos: Vec<f64> = sel
            .iter()
            .map(|&i| {
                let d = s.p[i].as_dvec3() - c;
                (d - a * d.dot(a)).length()
            })
            .collect();
        r = percentile(&mut rhos, 0.5);
    }
    let (lo, hi) = (r - (0.1 * r).max(0.6), r + (0.12 * r).max(0.9));
    // Authored frames: an axis within 0.5° of a mesh axis is that axis; a
    // bore that is not a clean surface of revolution (σ large: twisted
    // ribbons, open shanks) trusts a mesh axis up to 8° away.
    let (m, theta) = nearest_mesh_axis(a);
    let mut snapped_deg = None;
    if theta < 0.5f64.to_radians() || (sigma > 0.05 && theta < 8f64.to_radians()) {
        let sel = select(m, c, lo, hi);
        if sel.len() >= 40 {
            a = m;
            // A clean fit already put `c` on the axis (coaxial recesses under a
            // head would bias a circle fit); otherwise re-centre by circle fit.
            if sigma > 0.05 {
                c = kasa_center(s, &sel, a, c).unwrap_or(c);
            }
            snapped_deg = Some(theta.to_degrees());
        }
    }
    let sel = select(a, c, lo, hi);
    if sel.len() < 40 {
        return None;
    }
    let (u, v) = basis(a);
    let mut rhos: Vec<f64> = Vec::with_capacity(sel.len());
    let mut zs: Vec<f64> = Vec::with_capacity(sel.len());
    let mut bore_pts = Vec::with_capacity(sel.len());
    const NB: usize = 36;
    let mut bin_min = [f64::MAX; NB];
    for &i in &sel {
        let d = s.p[i].as_dvec3() - c;
        let z = d.dot(a);
        let (x, y) = (d.dot(u), d.dot(v));
        let rho = (x * x + y * y).sqrt();
        rhos.push(rho);
        zs.push(z);
        let phi = y.atan2(x);
        bore_pts.push((phi, z));
        let b = (((phi / std::f64::consts::TAU) + 1.0).fract() * NB as f64) as usize % NB;
        bin_min[b] = bin_min[b].min(rho);
    }
    let filled: Vec<f64> = bin_min.iter().copied().filter(|v| *v < f64::MAX).collect();
    let coverage = filled.len() as f64 / NB as f64;
    let (mn, mx) = filled.iter().fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let radius = percentile(&mut rhos, 0.01);
    let roundness = (1.0 - (mx - mn) / radius.max(1e-9)).clamp(0.0, 1.0);
    let z_lo = percentile(&mut zs.clone(), 0.02);
    let z_hi = percentile(&mut zs, 0.98);
    let zmid = 0.5 * (z_lo + z_hi);
    c += a * zmid;
    for p in &mut bore_pts {
        p.1 -= zmid;
    }
    Some(Bore { axis: a, center: c, radius, coverage, roundness, samples: sel.len(), sigma, snapped_deg, bore_pts })
}

/// Closest signed mesh axis to `a` and the angle to it (radians).
fn nearest_mesh_axis(a: DVec3) -> (DVec3, f64) {
    let comps = [a.x, a.y, a.z];
    let k = (0..3).max_by(|&i, &j| comps[i].abs().total_cmp(&comps[j].abs())).unwrap();
    let mut m = DVec3::ZERO;
    m[k] = comps[k].signum();
    (m, a.dot(m).clamp(-1.0, 1.0).acos())
}

/// Kåsa circle fit of the samples projected on the plane ⟂ `a` through `c`;
/// returns the circle centre (same height as `c`).
fn kasa_center(s: &Samples, sel: &[usize], a: DVec3, c: DVec3) -> Option<DVec3> {
    let (u, v) = basis(a);
    let (mut sxx, mut sxy, mut syy, mut sx, mut sy, mut n) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut sxz, mut syz, mut sz) = (0.0, 0.0, 0.0);
    for &i in sel {
        let d = s.p[i].as_dvec3() - c;
        let (x, y) = (d.dot(u), d.dot(v));
        let z = x * x + y * y;
        sxx += x * x;
        sxy += x * y;
        syy += y * y;
        sx += x;
        sy += y;
        n += 1.0;
        sxz += x * z;
        syz += y * z;
        sz += z;
    }
    let mat = glam::DMat3::from_cols(DVec3::new(sxx, sxy, sx), DVec3::new(sxy, syy, sy), DVec3::new(sx, sy, n));
    if mat.determinant().abs() < 1e-12 {
        return None;
    }
    let sol = mat.inverse() * DVec3::new(-sxz, -syz, -sz);
    Some(c + u * (-0.5 * sol.x) + v * (-0.5 * sol.y))
}

/// Refine the head angle so the plane through the finger axis and the head
/// is the best mirror plane of the sample cloud (voxel occupancy test).
/// Returns (angle, score there, score at `phi0`).
fn mirror_refine(s: &Samples, c: DVec3, a: DVec3, u: DVec3, v: DVec3, phi0: f64, spacing: f64) -> Option<(f64, f64, f64)> {
    let pts: Vec<DVec3> = s
        .p
        .iter()
        .map(|p| {
            let d = p.as_dvec3() - c;
            DVec3::new(d.dot(u), d.dot(v), d.dot(a))
        })
        .collect();
    let (mut lo, mut hi) = (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN));
    for p in &pts {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    // Symmetric box in x/y so reflections stay inside.
    let rxy = lo.x.abs().max(hi.x.abs()).max(lo.y.abs()).max(hi.y.abs());
    lo.x = -rxy;
    lo.y = -rxy;
    hi.x = rxy;
    hi.y = rxy;
    let ext = (hi - lo).max_element();
    let h = (3.0 * spacing).max(ext / 400.0);
    let dims = ((hi - lo) / h).ceil().as_uvec3() + glam::UVec3::splat(2);
    let total = dims.x as usize * dims.y as usize * dims.z as usize;
    if total > 200_000_000 {
        return None;
    }
    let mut bits = vec![0u64; total.div_ceil(64)];
    let idx = |p: DVec3| -> Option<usize> {
        let q = (p - lo) / h;
        if q.x < 0.0 || q.y < 0.0 || q.z < 0.0 {
            return None;
        }
        let (x, y, z) = (q.x as usize, q.y as usize, q.z as usize);
        if x >= dims.x as usize || y >= dims.y as usize || z >= dims.z as usize {
            return None;
        }
        Some((z * dims.y as usize + y) * dims.x as usize + x)
    };
    for p in &pts {
        if let Some(i) = idx(*p) {
            bits[i >> 6] |= 1 << (i & 63);
        }
    }
    let stride = (pts.len() / 40_000).max(1);
    let test: Vec<DVec3> = pts.iter().step_by(stride).copied().collect();
    let score = |phi: f64| -> f64 {
        let m = glam::DVec2::new(-phi.sin(), phi.cos());
        let mut hit = 0usize;
        for p in &test {
            let d = p.x * m.x + p.y * m.y;
            let q = DVec3::new(p.x - 2.0 * d * m.x, p.y - 2.0 * d * m.y, p.z);
            if let Some(i) = idx(q)
                && bits[i >> 6] & (1 << (i & 63)) != 0
            {
                hit += 1;
            }
        }
        hit as f64 / test.len().max(1) as f64
    };
    let coarse: Vec<(f64, f64)> = (-30..=30)
        .into_par_iter()
        .map(|k| {
            let phi = phi0 + (k as f64).to_radians();
            (phi, score(phi))
        })
        .collect();
    let (p1, _) = coarse.iter().copied().max_by(|x, y| x.1.total_cmp(&y.1))?;
    let fine: Vec<(f64, f64)> = (-30..=30)
        .into_par_iter()
        .map(|k| {
            let phi = p1 + (k as f64 * 0.05).to_radians();
            (phi, score(phi))
        })
        .collect();
    let best = fine.iter().map(|x| x.1).fold(f64::MIN, f64::max);
    let plateau: Vec<f64> = fine.iter().filter(|x| x.1 >= best - 0.002).map(|x| x.0).collect();
    let phi = plateau.iter().sum::<f64>() / plateau.len() as f64;
    Some((phi, best, score(phi0)))
}

/// Axis of a surface of revolution: every surface normal line meets the
/// axis, so minimise the (Tukey-robust) distances between the bore samples'
/// normal lines and the axis line. Gauss–Newton over tilt (2) and offset (2).
fn fit_axis_normal_lines(s: &Samples, sel: &[usize], a0: DVec3, c0: DVec3) -> (DVec3, DVec3, f64) {
    let mut a = a0.normalize();
    let mut c = c0;
    let n = sel.len();
    let resid = |a: DVec3, c: DVec3, out: &mut Vec<f64>| {
        out.clear();
        for &i in sel {
            let m = a.cross(s.ns[i].as_dvec3());
            let l = m.length();
            out.push(if l > 0.2 { (s.p[i].as_dvec3() - c).dot(m) / l } else { 0.0 });
        }
    };
    let mut r0 = Vec::with_capacity(n);
    let mut rk: [Vec<f64>; 4] = Default::default();
    let mut w = vec![1.0f64; n];
    let mut sigma_out = f64::MAX;
    for it in 0..20 {
        let (u, v) = basis(a);
        resid(a, c, &mut r0);
        if it >= 2 {
            let mut abs: Vec<f64> = r0.iter().map(|x| x.abs()).collect();
            let sigma = 1.4826 * percentile(&mut abs, 0.5) + 1e-4;
            sigma_out = sigma;
            let k = 4.685 * sigma;
            for (wi, ri) in w.iter_mut().zip(&r0) {
                let t = ri / k;
                *wi = if t.abs() < 1.0 { (1.0 - t * t).powi(2) } else { 0.0 };
            }
        }
        let h = 1e-6;
        resid((a + u * h).normalize(), c, &mut rk[0]);
        resid((a + v * h).normalize(), c, &mut rk[1]);
        resid(a, c + u * h, &mut rk[2]);
        resid(a, c + v * h, &mut rk[3]);
        let mut jtj = [[0.0f64; 4]; 4];
        let mut jtr = [0.0f64; 4];
        for i in 0..n {
            let wi = w[i];
            if wi == 0.0 {
                continue;
            }
            let j = [(rk[0][i] - r0[i]) / h, (rk[1][i] - r0[i]) / h, (rk[2][i] - r0[i]) / h, (rk[3][i] - r0[i]) / h];
            for p in 0..4 {
                jtr[p] += wi * j[p] * r0[i];
                for q in 0..4 {
                    jtj[p][q] += wi * j[p] * j[q];
                }
            }
        }
        let tr = jtj[0][0] + jtj[1][1] + jtj[2][2] + jtj[3][3];
        for (p, row) in jtj.iter_mut().enumerate() {
            row[p] += 1e-9 * tr + 1e-12;
        }
        let Some(d) = solve4(jtj, jtr.map(|x| -x)) else { break };
        // Keep steps sane (the start is already within a few degrees).
        let tilt = (d[0] * d[0] + d[1] * d[1]).sqrt();
        let scale = if tilt > 0.1 { 0.1 / tilt } else { 1.0 };
        a = (a + (u * d[0] + v * d[1]) * scale).normalize();
        c += (u * d[2] + v * d[3]) * scale;
        if tilt < 1e-8 && (d[2] * d[2] + d[3] * d[3]).sqrt() < 1e-7 {
            break;
        }
    }
    resid(a, c, &mut r0);
    let mut abs: Vec<f64> = r0.iter().map(|x| x.abs()).collect();
    if !abs.is_empty() {
        sigma_out = 1.4826 * percentile(&mut abs, 0.5);
    }
    (a, c, sigma_out)
}

/// Gaussian elimination with partial pivoting for a 4×4 system.
fn solve4(mut m: [[f64; 4]; 4], mut b: [f64; 4]) -> Option<[f64; 4]> {
    for col in 0..4 {
        let piv = (col..4).max_by(|&i, &j| m[i][col].abs().total_cmp(&m[j][col].abs()))?;
        if m[piv][col].abs() < 1e-300 {
            return None;
        }
        m.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..4 {
            let f = m[row][col] / m[col][col];
            for k in col..4 {
                m[row][k] -= f * m[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; 4];
    for row in (0..4).rev() {
        let mut acc = b[row];
        for k in row + 1..4 {
            acc -= m[row][k] * x[k];
        }
        x[row] = acc / m[row][row];
    }
    Some(x)
}

fn percentile(v: &mut [f64], q: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let k = ((v.len() - 1) as f64 * q).round() as usize;
    let (_, x, _) = v.select_nth_unstable_by(k, |a, b| a.total_cmp(b));
    *x
}

fn median(v: &[f64]) -> f64 {
    let mut w = v.to_vec();
    percentile(&mut w, 0.5)
}

fn circular_smooth(v: &[f64], half: usize) -> Vec<f64> {
    let n = v.len();
    (0..n)
        .map(|i| {
            let mut s = 0.0;
            for k in 0..=2 * half {
                s += v[(i + n + k - half) % n];
            }
            s / (2 * half + 1) as f64
        })
        .collect()
}

fn ang_dist(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(std::f64::consts::TAU);
    d.min(std::f64::consts::TAU - d)
}

fn describe(d: DVec3) -> String {
    let comps = [(d.x, "X"), (d.y, "Y"), (d.z, "Z")];
    let (v, name) = comps.iter().copied().max_by(|a, b| a.0.abs().total_cmp(&b.0.abs())).unwrap();
    if v.abs() > 0.985 {
        format!("mesh {}{}", if v > 0.0 { "+" } else { "-" }, name)
    } else {
        format!("mesh ({:+.2}, {:+.2}, {:+.2})", d.x, d.y, d.z)
    }
}

/// Unit vector ⟂ `a` closest to the first mesh axis (Y, Z, X order) that is
/// not nearly parallel to `a` — keeps symmetric pieces close to the file's
/// own orientation.
fn reference_perp(a: DVec3) -> DVec3 {
    for r in [DVec3::Y, DVec3::Z, DVec3::X] {
        let p = r - a * r.dot(a);
        if p.length() > 0.5 {
            return p.normalize();
        }
    }
    basis(a).0
}

/// Sign that keeps `a` pointing along the mesh axis it is most aligned with.
fn reference_sign(a: DVec3) -> f64 {
    let comps = [a.x, a.y, a.z];
    let k = (0..3).max_by(|&i, &j| comps[i].abs().total_cmp(&comps[j].abs())).unwrap();
    if comps[k] >= 0.0 { 1.0 } else { -1.0 }
}

/// Per-sample curvature magnitude (rad/mm) from the vertex normals of its
/// triangle; 1.0 everywhere when the mesh has no normals.
fn curvature_proxy(mesh: &Mesh, tri: u32) -> f64 {
    if mesh.normals.len() != mesh.positions.len() {
        return 1.0;
    }
    let t = mesh.triangles[tri as usize];
    let n = t.map(|i| Vec3::from(mesh.normals[i as usize]).as_dvec3());
    let p = t.map(|i| Vec3::from(mesh.positions[i as usize]).as_dvec3());
    let ang = |a: DVec3, b: DVec3| a.dot(b).clamp(-1.0, 1.0).acos();
    let spread = ang(n[0], n[1]).max(ang(n[1], n[2])).max(ang(n[0], n[2]));
    let l = ((p[1] - p[0]).length() + (p[2] - p[1]).length() + (p[0] - p[2]).length()) / 3.0;
    if l > 0.0 { (spread / l).min(50.0) } else { 0.0 }
}

/// Rows (X, Y, Z) of a proper rotation from target Y and Z directions
/// (re-orthonormalised, so det = +1 up to rounding).
fn rotation_rows(ey: DVec3, ez: DVec3) -> [[f32; 3]; 3] {
    let ez = ez.normalize();
    let ey = (ey - ez * ey.dot(ez)).normalize();
    let ex = ey.cross(ez).normalize();
    [ex.as_vec3().to_array(), ey.as_vec3().to_array(), ez.as_vec3().to_array()]
}

/// For the samples in `idx`, which are visible looking down `-axis` (top,
/// the outermost surface along +axis) and up `+axis` (bottom): a sample is
/// visible when it lies within `tol` of the depth extreme of its raster
/// cell. Removes internal faces of overlapping shells from relief measures.
fn visible_along(s: &Samples, idx: &[usize], axis: DVec3, g: usize) -> (Vec<bool>, Vec<bool>) {
    let (u, v) = basis(axis);
    let pts: Vec<(f64, f64, f64)> = idx
        .iter()
        .map(|&i| {
            let p = s.p[i].as_dvec3();
            (p.dot(u), p.dot(v), p.dot(axis))
        })
        .collect();
    if pts.is_empty() {
        return (vec![], vec![]);
    }
    let (mut xl, mut xh, mut yl, mut yh) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for &(x, y, _) in &pts {
        xl = xl.min(x);
        xh = xh.max(x);
        yl = yl.min(y);
        yh = yh.max(y);
    }
    let cell = ((xh - xl).max(yh - yl) / (g - 2) as f64).max(1e-6);
    let cell_of = |x: f64, y: f64| {
        let ix = (((x - xl) / cell) as usize).min(g - 1);
        let iy = (((y - yl) / cell) as usize).min(g - 1);
        iy * g + ix
    };
    let mut top = vec![f64::MIN; g * g];
    let mut bot = vec![f64::MAX; g * g];
    for &(x, y, z) in &pts {
        let c = cell_of(x, y);
        top[c] = top[c].max(z);
        bot[c] = bot[c].min(z);
    }
    let tol = (2.5 * cell).max(0.1);
    let vis_top = pts.iter().map(|&(x, y, z)| z >= top[cell_of(x, y)] - tol).collect();
    let vis_bot = pts.iter().map(|&(x, y, z)| z <= bot[cell_of(x, y)] + tol).collect();
    (vis_top, vis_bot)
}

const NBINS: usize = 72;
/// Front/back detail ratio needed before a head is said to have a face.
const FACE_RATIO: f64 = 1.35;

fn detect_ring(mesh: &Mesh, s: &Samples, pca: &Pca, notes: &mut Vec<String>) -> Option<Orientation> {
    let mut axes = fibonacci_hemisphere(320);
    axes.extend_from_slice(&pca.axes);
    axes.extend_from_slice(&[DVec3::X, DVec3::Y, DVec3::Z]);
    let scores: Vec<Option<f64>> = axes.par_iter().map(|a| bore_score(s, *a)).collect();
    let (mut best_a, mut best) = axes
        .iter()
        .zip(&scores)
        .filter_map(|(a, sc)| sc.map(|v| (*a, v)))
        .max_by(|x, y| x.1.total_cmp(&y.1))
        .or_else(|| {
            notes.push("no enclosed bore-sized hole (Ø 7–90 mm) in any view: not a ring".into());
            None
        })?;
    // Pattern search on the sphere.
    let mut step = 5f64.to_radians();
    while step > 0.1f64.to_radians() {
        let (u, v) = basis(best_a);
        let cands: Vec<DVec3> = (0..8)
            .map(|k| {
                let t = k as f64 * std::f64::consts::FRAC_PI_4;
                (best_a * step.cos() + (u * t.cos() + v * t.sin()) * step.sin()).normalize()
            })
            .collect();
        let sc: Vec<Option<f64>> = cands.par_iter().map(|a| bore_score(s, *a)).collect();
        match cands.iter().zip(&sc).filter_map(|(a, v)| v.map(|v| (*a, v))).max_by(|x, y| x.1.total_cmp(&y.1)) {
            Some((a, v)) if v > best * (1.0 + 1e-9) => {
                best = v;
                best_a = a;
            }
            _ => step *= 0.5,
        }
    }
    let r = Raster::new(s, best_a, 384);
    let (holes, _, _) = r.holes();
    let hole = holes.first()?.clone();
    if debug() {
        eprintln!("[orient] silhouette axis {best_a:.5}, hole area {best:.2} mm² (Ø≈{:.2})", hole.diameter());
    }
    let along = s.p.iter().map(|p| p.as_dvec3().dot(best_a)).sum::<f64>() / s.p.len() as f64;
    let c0 = r.u * hole.cx + r.v * hole.cy + best_a * along;
    let Some(bore) = refine_bore(s, best_a, c0, 0.5 * hole.diameter()) else {
        notes.push(format!(
            "largest hole Ø {:.1} mm along {} has no inward-facing bore surface: not a ring",
            hole.diameter(),
            describe(best_a)
        ));
        return None;
    };
    let d = 2.0 * bore.radius;
    if !(9.0..=80.0).contains(&d) || bore.coverage < 0.6 || bore.roundness < 0.7 {
        notes.push(format!(
            "hole along {} (Ø {:.1} mm, coverage {:.0}%, roundness {:.2}) is not a finger bore",
            describe(bore.axis),
            d,
            bore.coverage * 100.0,
            bore.roundness
        ));
        return None;
    }
    let a = bore.axis;
    let c = bore.center;
    let (u, v) = basis(a);

    // ---- angular profiles ------------------------------------------------
    let tau = std::f64::consts::TAU;
    let bin_of = |x: f64, y: f64| (((y.atan2(x) / tau) + 1.0).fract() * NBINS as f64) as usize % NBINS;
    let cu = c;
    let vol: Vec<f64> = mesh
        .triangles
        .par_chunks(16384)
        .map(|chunk| {
            let mut h = vec![0.0f64; NBINS];
            for t in chunk {
                let [p0, p1, p2] = t.map(|i| Vec3::from(mesh.positions[i as usize]).as_dvec3() - cu);
                let cen = (p0 + p1 + p2) / 3.0;
                let b = bin_of(cen.dot(u), cen.dot(v));
                h[b] += p0.dot(p1.cross(p2)) / 6.0;
            }
            h
        })
        .reduce(|| vec![0.0; NBINS], |mut x, y| {
            for i in 0..NBINS {
                x[i] += y[i];
            }
            x
        });
    let mut r_out = vec![0.0f64; NBINS];
    let mut z_lo = vec![f64::MAX; NBINS];
    let mut z_hi = vec![f64::MIN; NBINS];
    let mut info: Vec<(f64, f64, f64)> = Vec::with_capacity(s.p.len()); // (phi, rho, z)
    for p in &s.p {
        let dd = p.as_dvec3() - c;
        let (x, y, z) = (dd.dot(u), dd.dot(v), dd.dot(a));
        let rho = (x * x + y * y).sqrt();
        let b = bin_of(x, y);
        r_out[b] = r_out[b].max(rho);
        z_lo[b] = z_lo[b].min(z);
        z_hi[b] = z_hi[b].max(z);
        info.push((y.atan2(x), rho, z));
    }
    let total_vol: f64 = vol.iter().sum();
    let closed = total_vol > 0.0 && vol.iter().all(|x| *x > -0.02 * total_vol / NBINS as f64);
    let r_out_s = circular_smooth(&r_out, 2);
    let prof: Vec<f64> = if closed {
        circular_smooth(&vol, 3)
    } else {
        // Open / inconsistent mesh: surface area beyond the typical outer radius.
        let med_r = median(&r_out_s);
        let mut beyond = vec![0.0f64; NBINS];
        for &(phi, rho, _) in &info {
            let b = (((phi / tau) + 1.0).fract() * NBINS as f64) as usize % NBINS;
            beyond[b] += (rho - med_r).max(0.0) + 0.05;
        }
        circular_smooth(&beyond, 3)
    };
    let med = median(&prof).max(1e-12);
    let (peak_b, peak) = prof.iter().copied().enumerate().max_by(|x, y| x.1.total_cmp(&y.1)).unwrap();
    let prominence = (peak - med) / med;
    let bin_angle = |b: usize| (b as f64 + 0.5) / NBINS as f64 * tau;
    let peak_angle = bin_angle(peak_b);
    let has_head = prominence >= 0.1;
    let mut head_angle = if has_head {
        let mut acc = glam::DVec2::ZERO;
        for (b, &pv) in prof.iter().enumerate() {
            let ang = bin_angle(b);
            if ang_dist(ang, peak_angle) <= 45f64.to_radians() {
                acc += glam::DVec2::new(ang.cos(), ang.sin()) * (pv - med).max(0.0);
            }
        }
        if acc.length() > 0.0 { acc.y.atan2(acc.x) } else { peak_angle }
    } else {
        let rp = reference_perp(a);
        rp.dot(v).atan2(rp.dot(u))
    };
    let mut head_notes: Vec<String> = Vec::new();
    if has_head && let Some((phi, best, at0)) = mirror_refine(s, c, a, u, v, head_angle, s.spacing) {
        let delta = ang_dist(phi, head_angle).to_degrees();
        if best >= 0.85 && best > at0 + 1e-3 && delta > 0.05 {
            head_notes.push(format!("head axis refined {delta:.1}° by mirror symmetry (score {best:.3} vs {at0:.3})"));
            head_angle = phi;
        }
    }
    let mut head = (u * head_angle.cos() + v * head_angle.sin()).normalize();
    if bore.snapped_deg.is_some() {
        // Authored frame: the head sits on a mesh axis too unless it is far off.
        let (m, theta) = nearest_mesh_axis(head);
        if theta > 1e-9 && theta < 10f64.to_radians() && m.dot(a).abs() < 1e-9 {
            head_notes.push(format!("head snapped {:.1}° onto {} (authored frame)", theta.to_degrees(), describe(m)));
            head = m;
            head_angle = head.dot(v).atan2(head.dot(u));
        }
    }
    let head_conf = ((prominence - 0.1) / 0.6).clamp(0.0, 1.0);

    // ---- shank cross-section: reject flat discs with a central hole --------
    let opposite = head_angle + std::f64::consts::PI;
    let shank: Vec<usize> = (0..NBINS).filter(|&b| ang_dist(bin_angle(b), opposite) <= 30f64.to_radians() && z_hi[b] > z_lo[b]).collect();
    let r_shank = median(&shank.iter().map(|&b| r_out[b]).collect::<Vec<_>>());
    let w_shank = median(&shank.iter().map(|&b| z_hi[b] - z_lo[b]).collect::<Vec<_>>());
    let radial = r_shank - bore.radius;
    if radial > 3.0 && radial > 2.5 * w_shank {
        notes.push(format!(
            "flat disc with a Ø {d:.1} mm central hole (rim {radial:.1} mm wide, {w_shank:.1} mm thick): treated as a pendant"
        ));
        return None;
    }

    // ---- axial centre: mid-plane of the shank's bore (a head may extend the
    // inner surface toward one side) ------------------------------------------
    let mut c = c;
    if has_head {
        let mut zs: Vec<f64> =
            bore.bore_pts.iter().filter(|(phi, _)| ang_dist(*phi, opposite) <= 60f64.to_radians()).map(|p| p.1).collect();
        if zs.len() >= 30 {
            let zl = percentile(&mut zs, 0.02);
            let zh = percentile(&mut zs, 0.98);
            c += a * (0.5 * (zl + zh));
        }
    }

    // ---- face along the finger -----------------------------------------
    let head_r_out = r_out_s.iter().copied().fold(0.0, f64::max);
    let min_rho = if prominence >= 0.25 { r_shank * 0.98 } else { bore.radius + 0.5 * (r_shank - bore.radius) };
    let region: Vec<usize> = (0..s.p.len())
        .filter(|&i| {
            let (phi, rho, _) = info[i];
            ang_dist(phi, head_angle) <= 40f64.to_radians() && rho >= min_rho
        })
        .collect();
    let mut face_sign = reference_sign(a);
    let mut face_conf = 0.0;
    let face_note = if !has_head {
        format!("no head, so no face: kept the file's {} sense along the finger", describe(a * face_sign))
    } else if region.len() >= 50 {
        let mut zs: Vec<f64> = region.iter().map(|&i| info[i].2).collect();
        let zl = percentile(&mut zs, 0.02);
        let zh = percentile(&mut zs, 0.98);
        let mid = 0.5 * (zl + zh);
        let band = 0.1 * (zh - zl);
        let (vis_p, vis_m) = visible_along(s, &region, a, 128);
        let (mut dp, mut dm) = (0.0f64, 0.0f64);
        for (k, &i) in region.iter().enumerate() {
            let z = info[i].2;
            if z > mid + band && vis_p[k] {
                dp += curvature_proxy(mesh, s.t[i]);
            } else if z < mid - band && vis_m[k] {
                dm += curvature_proxy(mesh, s.t[i]);
            }
        }
        let eps = 1e-9 + 0.01 * (dp + dm);
        let ratio = (dp + eps) / (dm + eps);
        let rr = if ratio > 1.0 { ratio } else { 1.0 / ratio };
        if rr >= FACE_RATIO {
            face_sign = if ratio > 1.0 { 1.0 } else { -1.0 };
            face_conf = ((rr - FACE_RATIO) / 0.65).clamp(0.0, 1.0);
            format!("face toward {} along the finger: head detail ratio {:.2} front vs back", describe(a * face_sign), rr)
        } else {
            format!(
                "head front/back symmetric (detail ratio {:.2}): kept the file's {} sense along the finger",
                rr,
                describe(a * face_sign)
            )
        }
    } else {
        format!("no head region to read a face from: kept the file's {} sense", describe(a * face_sign))
    };
    let ez = a * face_sign;
    let rotation = rotation_rows(head, ez);
    let size = us_ring_size(d as f32);
    notes.push(format!(
        "ring: bore Ø {:.2} mm (≈ US {:.1}) along {}, roundness {:.2}, {} bore samples",
        d,
        size,
        describe(a),
        bore.roundness,
        bore.samples
    ));
    if has_head {
        notes.push(format!(
            "head toward {}: {} peak {:.2}× the median, reaching {:.1} mm from the axis (shank {:.1} mm)",
            describe(head),
            if closed { "enclosed-volume" } else { "outer-surface" },
            1.0 + prominence,
            head_r_out,
            r_shank
        ));
    } else {
        notes.push(format!(
            "no distinct head (volume peak only {:.2}× median): plain band, kept {} up",
            1.0 + prominence,
            describe(head)
        ));
    }
    notes.extend(head_notes);
    notes.push(face_note);
    if let Some(deg) = bore.snapped_deg {
        notes.push(format!(
            "finger axis snapped {deg:.2}° onto {} (authored frame; bore fit σ {:.3} mm)",
            describe(a),
            bore.sigma
        ));
    }
    notes.push(format!("bore centre at mesh ({:.2}, {:.2}, {:.2})", c.x, c.y, c.z));
    let class_conf = (0.4 + 0.3 * bore.coverage + 0.3 * ((bore.roundness - 0.7) / 0.25).clamp(0.0, 1.0)).clamp(0.0, 1.0);
    let confidence = class_conf * (0.6 + 0.25 * head_conf + 0.15 * face_conf);
    Some(Orientation {
        kind: PieceKind::Ring,
        pose: crate::Pose { center: c.as_vec3().to_array(), rotation },
        bore_radius: Some(bore.radius as f32),
        confidence: confidence as f32,
        notes: std::mem::take(notes),
    })
}

// ---------------------------------------------------------------------------
// Pendant / other

fn detect_flat(mesh: &Mesh, s: &Samples, pca: &Pca, mut notes: Vec<String>) -> Orientation {
    let [e1, e2, e3] = pca.axes;
    let ext = |d: DVec3| {
        let (lo, hi) = s.p.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
            let x = p.as_dvec3().dot(d);
            (lo.min(x), hi.max(x))
        });
        (lo, hi)
    };
    let (l1, h1) = ext(e1);
    let (l2, h2) = ext(e2);
    let (l3, h3) = ext(e3);
    let (x1, x2, x3) = (h1 - l1, h2 - l2, h3 - l3);
    let flat = x3 < 0.6 * x2;

    // ---- bail: small enclosed hole near the rim ---------------------------
    let mut up: Option<(DVec3, String)> = None;
    let front = Raster::new(s, e3, 384);
    let (holes, (scx, scy), _) = front.holes();
    let g = front.g;
    let mut best: Option<(f64, DVec3, f64, f64)> = None;
    for h in holes.iter().filter(|h| (0.5..=12.0).contains(&h.diameter())) {
        let (dx, dy) = (h.cx - scx, h.cy - scy);
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < 1e-6 {
            continue;
        }
        let (ux, uy) = (dx / dist, dy / dist);
        // Silhouette reach in that direction.
        let mut reach: f64 = 0.0;
        for (c, &o) in front.occ.iter().enumerate() {
            if o == 1 {
                let (x, y) = front.center_of(c % g, c / g);
                reach = reach.max((x - scx) * ux + (y - scy) * uy);
            }
        }
        let peripheral = dist / reach.max(1e-9);
        let dir = (front.u * ux + front.v * uy).normalize();
        // Bails sit on the long axis; rim filigree holes rarely do.
        let score = peripheral * (0.6 + 0.4 * dir.dot(e1).abs());
        if peripheral > 0.55 && best.as_ref().is_none_or(|b| score > b.3) {
            best = Some((peripheral, dir, h.diameter(), score));
        }
    }
    if let Some((per, dir, dia, _)) = best {
        let mut d = dir;
        for ax in [e1, -e1, e2, -e2] {
            if d.dot(ax) > 25f64.to_radians().cos() {
                d = ax;
            }
        }
        up = Some((d, format!("bail: Ø {dia:.1} mm loop at {:.0}% of the reach toward {}", per * 100.0, describe(d))));
    }
    if up.is_none() {
        // Side view: a tube/loop whose hole runs across the pendant.
        let side = Raster::new(s, e2, 384);
        let (holes, _, _) = side.holes();
        let mid1 = 0.5 * (l1 + h1);
        for h in holes.iter().filter(|h| (0.5..=12.0).contains(&h.diameter())) {
            let p = side.u * h.cx + side.v * h.cy;
            let t = (p.dot(e1) - mid1) / (0.5 * x1).max(1e-9);
            if t.abs() > 0.55 {
                let d = if t > 0.0 { e1 } else { -e1 };
                up = Some((d, format!("bail: Ø {:.1} mm cross hole near the {} end", h.diameter(), describe(d))));
                break;
            }
        }
    }
    let has_bail = up.is_some();
    let (up, up_note) = up.unwrap_or_else(|| {
        // Narrower end up.
        let (mut wt_lo, mut wt_hi) = ((f64::MAX, f64::MIN), (f64::MAX, f64::MIN));
        for p in &s.p {
            let p = p.as_dvec3();
            let t = (p.dot(e1) - l1) / x1.max(1e-9);
            let w = p.dot(e2);
            if t < 0.15 {
                wt_lo = (wt_lo.0.min(w), wt_lo.1.max(w));
            } else if t > 0.85 {
                wt_hi = (wt_hi.0.min(w), wt_hi.1.max(w));
            }
        }
        let (w_lo, w_hi) = (wt_lo.1 - wt_lo.0, wt_hi.1 - wt_hi.0);
        let d = if w_hi <= w_lo { e1 } else { -e1 };
        (d, format!("no bail found: narrower end ({:.1} vs {:.1} mm) up, toward {}", w_hi.min(w_lo), w_hi.max(w_lo), describe(d)))
    });

    // ---- front: the side with more relief ---------------------------------
    let (mut np, mut nm) = (DVec3::ZERO, DVec3::ZERO);
    let (mut cp, mut cm) = (0.0f64, 0.0f64);
    let (mut kp, mut km) = (0.0f64, 0.0f64);
    let all: Vec<usize> = (0..s.p.len()).collect();
    let (vis_p, vis_m) = visible_along(s, &all, e3, 256);
    for (i, n) in s.n.iter().enumerate() {
        let n = n.as_dvec3();
        let d = n.dot(e3);
        if d > 0.3 && vis_p[i] {
            np += n;
            cp += 1.0;
            kp += curvature_proxy(mesh, s.t[i]);
        } else if d < -0.3 && vis_m[i] {
            nm += n;
            cm += 1.0;
            km += curvature_proxy(mesh, s.t[i]);
        }
    }
    let var_p = if cp > 0.0 { 1.0 - np.length() / cp } else { 0.0 };
    let var_m = if cm > 0.0 { 1.0 - nm.length() / cm } else { 0.0 };
    let eps_k = 1e-9 + 0.01 * (kp + km);
    let r = (((var_p + 0.01) / (var_m + 0.01)) * ((kp + eps_k) / (km + eps_k))).sqrt();
    let (front_dir, front_conf) = if (r - 1.0).abs() < 0.05 {
        let sgn = reference_sign(e3);
        (e3 * sgn, 0.0)
    } else if r > 1.0 {
        (e3, (r.ln() / 2f64.ln()).clamp(0.0, 1.0))
    } else {
        (-e3, ((1.0 / r).ln() / 2f64.ln()).clamp(0.0, 1.0))
    };

    let kind = if has_bail || flat { PieceKind::Pendant } else { PieceKind::Other };
    let (ey, ez) = match kind {
        PieceKind::Pendant => (up, front_dir),
        _ => {
            let y = e1 * reference_sign(e1);
            let z = e3 * reference_sign(e3);
            (y, z)
        }
    };
    let ez = ez.normalize();
    let ey = (ey - ez * ey.dot(ez)).normalize();
    let ex = ey.cross(ez);
    let rotation = rotation_rows(ey, ez);
    // Bounding-box centre in the new frame, expressed in mesh space.
    let rot_d = [ex, ey, ez].map(|r| r.normalize());
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    for p in &mesh.positions {
        let q = Vec3::from(*p).as_dvec3();
        for k in 0..3 {
            let x = rot_d[k].dot(q);
            lo[k] = lo[k].min(x);
            hi[k] = hi[k].max(x);
        }
    }
    let mid = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1]), 0.5 * (lo[2] + hi[2])];
    let center = rot_d[0] * mid[0] + rot_d[1] * mid[1] + rot_d[2] * mid[2];
    let _ = pca.mean;
    let _ = pca.var;
    match kind {
        PieceKind::Pendant => {
            notes.push(format!(
                "pendant: {:.1} × {:.1} × {:.1} mm, thinnest along {}",
                x1,
                x2,
                x3,
                describe(e3)
            ));
            notes.push(up_note);
            notes.push(if front_conf > 0.0 {
                format!(
                    "front toward {}: relief ratio {:.2} (normal spread {:.3} vs {:.3})",
                    describe(front_dir),
                    if r > 1.0 { r } else { 1.0 / r },
                    var_p.max(var_m),
                    var_p.min(var_m)
                )
            } else {
                format!("front/back look alike (ratio {r:.2}): kept the file's {} sense", describe(front_dir))
            });
        }
        _ => {
            notes.push(format!(
                "other: principal axes {:.1} × {:.1} × {:.1} mm, longest along {} → +Y, thinnest along {} → +Z",
                x1,
                x2,
                x3,
                describe(e1),
                describe(e3)
            ));
        }
    }
    let confidence = match kind {
        PieceKind::Pendant => {
            0.35 + if flat { 0.2 } else { 0.0 } + if has_bail { 0.3 } else { 0.0 } + 0.15 * front_conf
        }
        _ => 0.3,
    };
    Orientation {
        kind,
        pose: crate::Pose { center: center.as_vec3().to_array(), rotation },
        bore_radius: None,
        confidence: confidence as f32,
        notes,
    }
}

#[cfg(test)]
mod tests;
