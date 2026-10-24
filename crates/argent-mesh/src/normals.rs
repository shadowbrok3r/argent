//! Crease-aware smooth normals.
//!
//! For every vertex the incident faces are grouped into *smooth fans*:
//! two faces sharing an edge at that vertex join when the angle between
//! their normals is below the crease angle (and they wind the edge in
//! opposite directions, i.e. consistently). Each fan becomes one output
//! vertex whose normal is the corner-angle-weighted mean of its face normals
//! (Thürmer & Wüthrich), which is independent of how a smooth surface was
//! triangulated. Fans at one vertex whose normals end up nearly identical
//! are merged again, so broken connectivity (slivers, T-junctions,
//! non-manifold seams) never shows up as a shading seam on a smooth surface.
//!
//! Before that, winding is checked: inconsistently wound faces are flipped
//! to agree with their shell (majority wins), and closed shells with negative
//! signed volume (inside-out) are flipped — unless they are enclosed by
//! another shell (an internal cavity of a hollow casting, which must face
//! inward).

use crate::util::{D3, UnionFind, V3, VertexFaces, cross_d, d3, dot_d, face_normals, len_d, retain_triangles, sub_d};
use crate::Mesh;
use rayon::prelude::*;

/// What [`compute`] found and changed.
#[derive(Clone, Debug, Default)]
pub struct NormalReport {
    /// Vertex-connected shells.
    pub shells: usize,
    /// Closed shells that were inside-out and got flipped.
    pub flipped_shells: usize,
    /// Closed shells kept inward-facing because another shell encloses them.
    pub cavity_shells: usize,
    /// Faces flipped to make winding consistent within their shell.
    pub repaired_faces: usize,
    pub boundary_edges: usize,
    pub nonmanifold_edges: usize,
    /// Manifold edges still wound inconsistently after repair (non-orientable).
    pub inconsistent_edges: usize,
    /// Zero-area faces (kept, but they carry no normal weight).
    pub degenerate_faces: usize,
    /// Vertices added by splitting along creases.
    pub split_vertices: usize,
}

/// Compute `mesh.normals` in place: fixes winding, splits vertices along
/// creases sharper than `crease_angle_deg` (0 → faceted), and drops
/// triangles with a repeated index. Triangle order (and so `parts`) is kept.
pub fn compute(mesh: &mut Mesh, crease_angle_deg: f32) -> NormalReport {
    compute_impl(mesh, Crease::Uniform(crease_angle_deg))
}

/// [`compute`] with a crease angle per part, e.g. 0° (faceted) for cut
/// stones and 50° for metal. An edge between two parts uses the smaller
/// angle. Faces outside every part use `crease_deg_for` of a default part.
pub fn compute_with(mesh: &mut Mesh, crease_deg_for: impl Fn(&crate::Part) -> f32) -> NormalReport {
    let ntri = mesh.triangles.len();
    let default = crease_deg_for(&crate::Part { name: String::new(), triangles: 0..0, material: None });
    let mut per_face = vec![default; ntri];
    for p in &mesh.parts {
        let c = crease_deg_for(p);
        let (a, b) = (p.triangles.start as usize, (p.triangles.end as usize).min(ntri));
        if a < b {
            per_face[a..b].fill(c);
        }
    }
    compute_impl(mesh, Crease::PerFace(per_face))
}

/// Re-shade an already shaded mesh with new crease angles: re-welds
/// bit-identical positions (undoing earlier crease splits), then
/// [`compute_with`].
pub fn recompute(mesh: &mut Mesh, crease_deg_for: impl Fn(&crate::Part) -> f32) -> NormalReport {
    crate::weld::weld(mesh, 0.0);
    compute_with(mesh, crease_deg_for)
}

enum Crease {
    Uniform(f32),
    PerFace(Vec<f32>),
}

/// Per-face cosine thresholds (for the fan joins) and merge thresholds.
struct Thresholds {
    uniform: Option<(f64, f64)>,
    face_cos: Vec<f64>,
    face_merge: Vec<f64>,
}

impl Thresholds {
    fn of_deg(deg: f32) -> (f64, f64) {
        let d = deg.clamp(0.05, 180.0);
        ((d as f64).to_radians().cos(), ((0.5 * d).clamp(0.05, 15.0) as f64).to_radians().cos())
    }
    fn new(c: &Crease) -> Thresholds {
        match c {
            Crease::Uniform(d) => Thresholds { uniform: Some(Self::of_deg(*d)), face_cos: vec![], face_merge: vec![] },
            Crease::PerFace(v) => {
                let (face_cos, face_merge) = v.par_iter().map(|d| Self::of_deg(*d)).unzip();
                Thresholds { uniform: None, face_cos, face_merge }
            }
        }
    }
    /// Join threshold for an edge between faces f and g (the stricter wins).
    #[inline]
    fn join(&self, f: u32, g: u32) -> f64 {
        match self.uniform {
            Some((c, _)) => c,
            None => self.face_cos[f as usize].max(self.face_cos[g as usize]),
        }
    }
    #[inline]
    fn merge(&self, faces: &[u32]) -> f64 {
        match self.uniform {
            Some((_, m)) => m,
            None => faces.iter().map(|&f| self.face_merge[f as usize]).fold(f64::MIN, f64::max),
        }
    }
}

fn compute_impl(mesh: &mut Mesh, crease: Crease) -> NormalReport {
    let mut rep = NormalReport::default();
    let keep: Vec<bool> =
        mesh.triangles.par_iter().map(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]).collect();
    let crease = match crease {
        Crease::PerFace(v) if keep.iter().any(|k| !k) => {
            Crease::PerFace(v.into_iter().zip(&keep).filter(|(_, k)| **k).map(|(c, _)| c).collect())
        }
        c => c,
    };
    retain_triangles(mesh, &keep);
    let nv = mesh.positions.len();
    if mesh.triangles.is_empty() {
        mesh.normals = vec![[0.0, 0.0, 1.0]; nv];
        return rep;
    }
    let th = Thresholds::new(&crease);

    let vf = VertexFaces::build(nv, &mesh.triangles);
    let (mut fnorm, areas) = face_normals(&mesh.positions, &mesh.triangles);
    rep.degenerate_faces = areas.iter().filter(|a| **a <= 0.0).count();

    let mut pass = cluster_pass(&mesh.positions, &mesh.triangles, &vf, &fnorm, &th);
    if pass.inconsistent > 0 {
        let flipped = repair_winding(&mut mesh.triangles, &vf);
        if flipped > 0 {
            rep.repaired_faces = flipped;
            fnorm = face_normals(&mesh.positions, &mesh.triangles).0;
            pass = cluster_pass(&mesh.positions, &mesh.triangles, &vf, &fnorm, &th);
        }
    }
    rep.boundary_edges = pass.boundary;
    rep.nonmanifold_edges = pass.nonmanifold;
    rep.inconsistent_edges = pass.inconsistent;

    // ---- shells & inside-out correction ---------------------------------
    let shells = Shells::build(mesh);
    rep.shells = shells.count;
    let flip_shell = shells.decide_flips(mesh, &pass.vertex_boundary, &mut rep);
    if flip_shell.iter().any(|f| *f) {
        let comp = &shells.comp;
        mesh.triangles.par_iter_mut().for_each(|t| {
            if flip_shell[comp[t[0] as usize] as usize] {
                t.swap(1, 2);
            }
        });
        // Negate the fan normals of every vertex in a flipped shell.
        let first = &pass.first;
        let mut normals = std::mem::take(&mut pass.normals);
        for v in 0..nv {
            let c = comp[v];
            if c != u32::MAX && flip_shell[c as usize] {
                for n in &mut normals[first[v] as usize..first[v + 1] as usize] {
                    *n = [-n[0], -n[1], -n[2]];
                }
            }
        }
        pass.normals = normals;
    }

    // ---- assemble split vertices ----------------------------------------
    let first = &pass.first;
    let mut extra_base = vec![0u32; nv + 1];
    for v in 0..nv {
        let cnt = first[v + 1] - first[v];
        extra_base[v + 1] = extra_base[v] + cnt.saturating_sub(1);
    }
    let extras = extra_base[nv] as usize;
    rep.split_vertices = extras;
    let mut normals: Vec<V3> = (0..nv)
        .into_par_iter()
        .with_min_len(8192)
        .map(|v| if first[v + 1] > first[v] { pass.normals[first[v] as usize] } else { [0.0, 0.0, 1.0] })
        .collect();
    normals.reserve(extras);
    mesh.positions.reserve(extras);
    for v in 0..nv {
        let (a, b) = (first[v] as usize, first[v + 1] as usize);
        if b > a + 1 {
            for c in a + 1..b {
                normals.push(pass.normals[c]);
                mesh.positions.push(mesh.positions[v]);
            }
        }
    }
    let labels = &pass.labels;
    let tris_src = mesh.triangles.clone();
    mesh.triangles.par_iter_mut().with_min_len(4096).enumerate().for_each(|(f, t)| {
        for k in 0..3 {
            let v = tris_src[f][k] as usize;
            let faces = vf.of(v);
            let j = faces.binary_search(&(f as u32)).expect("face in its vertex's fan");
            let label = labels[vf.offsets[v] as usize + j] as u32;
            t[k] = if label == 0 { v as u32 } else { nv as u32 + extra_base[v] + label - 1 };
        }
    });
    mesh.normals = normals;
    rep
}

/// Per-face unit normals (zero for degenerate faces).
pub fn face_normals_of(mesh: &Mesh) -> Vec<[f32; 3]> {
    face_normals(&mesh.positions, &mesh.triangles).0
}

/// Plain angle-weighted vertex normals without splitting or winding fixes
/// (for meshes whose topology must not change).
pub fn smooth_vertex_normals(mesh: &Mesh) -> Vec<[f32; 3]> {
    let (fnorm, _) = face_normals(&mesh.positions, &mesh.triangles);
    let mut acc = vec![[0.0f64; 3]; mesh.positions.len()];
    for (f, t) in mesh.triangles.iter().enumerate() {
        for k in 0..3 {
            let v = t[k] as usize;
            let p = d3(mesh.positions[v]);
            let ea = sub_d(d3(mesh.positions[t[(k + 1) % 3] as usize]), p);
            let eb = sub_d(d3(mesh.positions[t[(k + 2) % 3] as usize]), p);
            let ang = len_d(cross_d(ea, eb)).atan2(dot_d(ea, eb));
            for i in 0..3 {
                acc[v][i] += ang * fnorm[f][i] as f64;
            }
        }
    }
    acc.into_iter().map(normalize_or_z).collect()
}

fn normalize_or_z(a: D3) -> V3 {
    let l = len_d(a);
    if l > 1e-300 { [(a[0] / l) as f32, (a[1] / l) as f32, (a[2] / l) as f32] } else { [0.0, 0.0, 1.0] }
}

// ---------------------------------------------------------------------------
// Fan clustering

struct ClusterPass {
    /// Fan label per CSR entry (vertex-major, face order within a vertex).
    labels: Vec<u16>,
    /// `first[v]..first[v+1]` indexes `normals` for vertex v's fans.
    first: Vec<u32>,
    normals: Vec<V3>,
    /// Boundary edges counted at their lower-index endpoint.
    vertex_boundary: Vec<u32>,
    boundary: usize,
    nonmanifold: usize,
    inconsistent: usize,
}

#[derive(Default)]
struct Scratch {
    ent: Vec<(u32, u16, bool)>,
    parent: Vec<u16>,
    angle: Vec<f64>,
    label: Vec<u16>,
    sums: Vec<D3>,
    weight: Vec<f64>,
    cparent: Vec<u16>,
    remap: Vec<u16>,
}

fn find16(p: &mut [u16], mut x: u16) -> u16 {
    while p[x as usize] != x {
        let g = p[p[x as usize] as usize];
        p[x as usize] = g;
        x = g;
    }
    x
}

fn union16(p: &mut [u16], a: u16, b: u16) {
    let ra = find16(p, a);
    let rb = find16(p, b);
    if ra != rb {
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        p[hi as usize] = lo;
    }
}

struct ChunkOut {
    counts: Vec<u16>,
    normals: Vec<V3>,
    vertex_boundary: Vec<u32>,
    boundary: usize,
    nonmanifold: usize,
    inconsistent: usize,
}

const CHUNK: usize = 4096;
const MAX_FAN: usize = 60_000;

fn cluster_pass(pos: &[V3], tris: &[[u32; 3]], vf: &VertexFaces, fnorm: &[V3], th: &Thresholds) -> ClusterPass {
    let nv = pos.len();
    let mut labels = vec![0u16; vf.faces.len()];
    let nchunks = nv.div_ceil(CHUNK);
    let mut slices: Vec<&mut [u16]> = Vec::with_capacity(nchunks);
    let mut rest: &mut [u16] = &mut labels[..];
    for c in 0..nchunks {
        let v0 = c * CHUNK;
        let v1 = (v0 + CHUNK).min(nv);
        let len = (vf.offsets[v1] - vf.offsets[v0]) as usize;
        let (a, b) = std::mem::take(&mut rest).split_at_mut(len);
        slices.push(a);
        rest = b;
    }
    let outs: Vec<ChunkOut> = slices
        .into_par_iter()
        .enumerate()
        .map_init(Scratch::default, |s, (c, slice)| {
            let v0 = c * CHUNK;
            let v1 = (v0 + CHUNK).min(nv);
            let base = vf.offsets[v0] as usize;
            let mut out = ChunkOut {
                counts: Vec::with_capacity(v1 - v0),
                normals: Vec::with_capacity(v1 - v0),
                vertex_boundary: vec![0; v1 - v0],
                boundary: 0,
                nonmanifold: 0,
                inconsistent: 0,
            };
            for v in v0..v1 {
                let r = vf.range(v);
                let lab = &mut slice[r.start - base..r.end - base];
                let cnt = cluster_vertex(v as u32, vf.of(v), pos, tris, fnorm, th, s, lab, &mut out, v - v0);
                out.counts.push(cnt);
            }
            out
        })
        .collect();
    let mut first = Vec::with_capacity(nv + 1);
    first.push(0u32);
    let mut normals = Vec::with_capacity(nv + nv / 8);
    let mut vertex_boundary = Vec::with_capacity(nv);
    let (mut boundary, mut nonmanifold, mut inconsistent) = (0, 0, 0);
    for o in outs {
        for c in o.counts {
            let last = *first.last().unwrap();
            first.push(last + c as u32);
        }
        normals.extend_from_slice(&o.normals);
        vertex_boundary.extend_from_slice(&o.vertex_boundary);
        boundary += o.boundary;
        nonmanifold += o.nonmanifold;
        inconsistent += o.inconsistent;
    }
    ClusterPass { labels, first, normals, vertex_boundary, boundary, nonmanifold, inconsistent }
}

#[allow(clippy::too_many_arguments)]
fn cluster_vertex(
    v: u32,
    faces: &[u32],
    pos: &[V3],
    tris: &[[u32; 3]],
    fnorm: &[V3],
    th: &Thresholds,
    s: &mut Scratch,
    labels: &mut [u16],
    out: &mut ChunkOut,
    local_v: usize,
) -> u16 {
    let k = faces.len();
    if k == 0 {
        return 0;
    }
    let pv = d3(pos[v as usize]);
    s.ent.clear();
    s.angle.clear();
    for (j, &f) in faces.iter().enumerate() {
        let t = tris[f as usize];
        let slot = if t[0] == v { 0 } else if t[1] == v { 1 } else { 2 };
        let a = t[(slot + 1) % 3];
        let b = t[(slot + 2) % 3];
        let ea = sub_d(d3(pos[a as usize]), pv);
        let eb = sub_d(d3(pos[b as usize]), pv);
        s.angle.push(len_d(cross_d(ea, eb)).atan2(dot_d(ea, eb)));
        if k <= MAX_FAN {
            s.ent.push((a, j as u16, true));
            s.ent.push((b, j as u16, false));
        }
    }
    if k > MAX_FAN {
        // Pathological fan: one smooth normal, no splitting.
        let mut sum = [0.0f64; 3];
        for (j, &f) in faces.iter().enumerate() {
            let n = fnorm[f as usize];
            for i in 0..3 {
                sum[i] += s.angle[j] * n[i] as f64;
            }
        }
        labels.fill(0);
        out.normals.push(normalize_or_z(sum));
        return 1;
    }
    s.ent.sort_unstable();
    s.parent.clear();
    s.parent.extend(0..k as u16);
    let fn64 = |f: u32| -> D3 {
        let n = fnorm[f as usize];
        [n[0] as f64, n[1] as f64, n[2] as f64]
    };
    let mut i = 0;
    while i < s.ent.len() {
        let w = s.ent[i].0;
        let mut e = i + 1;
        while e < s.ent.len() && s.ent[e].0 == w {
            e += 1;
        }
        let m = e - i;
        if v < w {
            match m {
                1 => {
                    out.boundary += 1;
                    out.vertex_boundary[local_v] += 1;
                }
                2 => {
                    if s.ent[i].2 == s.ent[i + 1].2 {
                        out.inconsistent += 1;
                    }
                }
                _ => out.nonmanifold += 1,
            }
        }
        if m >= 2 {
            for x in i..e {
                for y in x + 1..e {
                    let (ex, ey) = (s.ent[x], s.ent[y]);
                    if ex.2 == ey.2 || ex.1 == ey.1 {
                        continue;
                    }
                    let (fx, fy) = (faces[ex.1 as usize], faces[ey.1 as usize]);
                    let d = dot_d(fn64(fx), fn64(fy));
                    if d >= th.join(fx, fy) {
                        union16(&mut s.parent, ex.1, ey.1);
                    }
                }
            }
        }
        i = e;
    }
    // Compact fan labels in face order.
    s.label.clear();
    s.label.resize(k, u16::MAX);
    s.remap.clear();
    s.remap.resize(k, u16::MAX);
    let mut nclusters = 0u16;
    for j in 0..k {
        let r = find16(&mut s.parent, j as u16) as usize;
        if s.remap[r] == u16::MAX {
            s.remap[r] = nclusters;
            nclusters += 1;
        }
        s.label[j] = s.remap[r];
    }
    let accumulate = |s: &mut Scratch, n: usize| {
        s.sums.clear();
        s.sums.resize(n, [0.0; 3]);
        s.weight.clear();
        s.weight.resize(n, 0.0);
        for j in 0..k {
            let l = s.label[j] as usize;
            let nf = fn64(faces[j]);
            let a = s.angle[j];
            for c in 0..3 {
                s.sums[l][c] += a * nf[c];
            }
            s.weight[l] += a * len_d(nf);
        }
    };
    accumulate(s, nclusters as usize);
    if nclusters > 1 {
        let cos_merge = th.merge(faces);
        // Merge fans with (nearly) the same normal, and fans with no
        // usable normal (only degenerate faces) into the heaviest fan.
        let n = nclusters as usize;
        s.cparent.clear();
        s.cparent.extend(0..nclusters);
        let unit: Vec<Option<D3>> = s
            .sums
            .iter()
            .map(|a| {
                let l = len_d(*a);
                (l > 1e-12).then(|| [a[0] / l, a[1] / l, a[2] / l])
            })
            .collect();
        let heaviest = (0..n).filter(|&c| unit[c].is_some()).max_by(|&a, &b| s.weight[a].total_cmp(&s.weight[b]));
        for a in 0..n {
            match (unit[a], heaviest) {
                (None, Some(h)) => union16(&mut s.cparent, a as u16, h as u16),
                (Some(na), _) => {
                    for b in a + 1..n {
                        if let Some(nb) = unit[b]
                            && dot_d(na, nb) >= cos_merge
                        {
                            union16(&mut s.cparent, a as u16, b as u16);
                        }
                    }
                }
                _ => {}
            }
        }
        s.remap.clear();
        s.remap.resize(n, u16::MAX);
        let mut m = 0u16;
        for c in 0..n {
            let r = find16(&mut s.cparent, c as u16) as usize;
            if s.remap[r] == u16::MAX {
                s.remap[r] = m;
                m += 1;
            }
        }
        if (m as usize) < n {
            for j in 0..k {
                let c = s.label[j] as usize;
                let r = find16(&mut s.cparent, c as u16) as usize;
                s.label[j] = s.remap[r];
            }
            nclusters = m;
            accumulate(s, nclusters as usize);
        }
    }
    labels.copy_from_slice(&s.label[..k]);
    for c in 0..nclusters as usize {
        out.normals.push(normalize_or_z(s.sums[c]));
    }
    nclusters
}

// ---------------------------------------------------------------------------
// Winding repair

/// Flip faces so every manifold edge is wound consistently within an
/// edge-connected component; per component, the original majority
/// orientation wins. Returns the number of faces flipped.
fn repair_winding(tris: &mut [[u32; 3]], vf: &VertexFaces) -> usize {
    let nt = tris.len();
    let mut flip = vec![false; nt];
    let mut seen = vec![false; nt];
    let mut stack: Vec<u32> = Vec::new();
    let mut comp: Vec<u32> = Vec::new();
    let mut total = 0usize;
    for seed in 0..nt {
        if seen[seed] {
            continue;
        }
        seen[seed] = true;
        stack.push(seed as u32);
        comp.clear();
        let mut nflip = 0usize;
        while let Some(f) = stack.pop() {
            let f = f as usize;
            comp.push(f as u32);
            let t = tris[f];
            for e in 0..3 {
                let (mut a, mut b) = (t[e], t[(e + 1) % 3]);
                if flip[f] {
                    std::mem::swap(&mut a, &mut b);
                }
                let mut nb = usize::MAX;
                let mut count = 0;
                for &g in vf.of(a as usize) {
                    let g = g as usize;
                    if g != f && tris[g].contains(&b) {
                        count += 1;
                        nb = g;
                    }
                }
                if count != 1 || seen[nb] {
                    continue;
                }
                let tg = tris[nb];
                let has_ab = (0..3).any(|k| tg[k] == a && tg[(k + 1) % 3] == b);
                seen[nb] = true;
                flip[nb] = has_ab;
                nflip += has_ab as usize;
                stack.push(nb as u32);
            }
        }
        if nflip * 2 > comp.len() {
            for &f in &comp {
                flip[f as usize] = !flip[f as usize];
            }
            nflip = comp.len() - nflip;
        }
        total += nflip;
    }
    for (t, &fl) in tris.iter_mut().zip(&flip) {
        if fl {
            t.swap(1, 2);
        }
    }
    total
}

// ---------------------------------------------------------------------------
// Shells

struct Shells {
    /// Shell id per vertex (u32::MAX for unreferenced vertices).
    comp: Vec<u32>,
    count: usize,
}

impl Shells {
    fn build(mesh: &Mesh) -> Shells {
        let nv = mesh.positions.len();
        let mut uf = UnionFind::new(nv);
        let mut used = vec![false; nv];
        for t in &mesh.triangles {
            uf.union(t[0], t[1]);
            uf.union(t[0], t[2]);
            used[t[0] as usize] = true;
            used[t[1] as usize] = true;
            used[t[2] as usize] = true;
        }
        let mut comp = vec![u32::MAX; nv];
        let mut id_of_root = vec![u32::MAX; nv];
        let mut count = 0u32;
        for v in 0..nv {
            if !used[v] {
                continue;
            }
            let r = uf.find(v as u32) as usize;
            if id_of_root[r] == u32::MAX {
                id_of_root[r] = count;
                count += 1;
            }
            comp[v] = id_of_root[r];
        }
        Shells { comp, count: count as usize }
    }

    /// Decide which shells to flip (inside-out closed shells that are not
    /// cavities of an enclosing shell).
    fn decide_flips(&self, mesh: &Mesh, vertex_boundary: &[u32], rep: &mut NormalReport) -> Vec<bool> {
        let ns = self.count;
        let mut flip = vec![false; ns];
        if ns == 0 {
            return flip;
        }
        let comp = &self.comp;
        // Reference vertex, bbox, face count, boundary edges, signed volume.
        let mut refv = vec![u32::MAX; ns];
        let mut lo = vec![[f32::MAX; 3]; ns];
        let mut hi = vec![[f32::MIN; 3]; ns];
        for (v, &c) in comp.iter().enumerate() {
            if c == u32::MAX {
                continue;
            }
            let c = c as usize;
            if refv[c] == u32::MAX {
                refv[c] = v as u32;
            }
            let p = mesh.positions[v];
            for k in 0..3 {
                lo[c][k] = lo[c][k].min(p[k]);
                hi[c][k] = hi[c][k].max(p[k]);
            }
        }
        let mut faces = vec![0usize; ns];
        let mut vol = vec![0.0f64; ns];
        for t in &mesh.triangles {
            let c = comp[t[0] as usize] as usize;
            faces[c] += 1;
            let o = d3(mesh.positions[refv[c] as usize]);
            let a = sub_d(d3(mesh.positions[t[0] as usize]), o);
            let b = sub_d(d3(mesh.positions[t[1] as usize]), o);
            let d = sub_d(d3(mesh.positions[t[2] as usize]), o);
            vol[c] += dot_d(a, cross_d(b, d)) / 6.0;
        }
        let mut boundary = vec![0u64; ns];
        for (v, &c) in comp.iter().enumerate() {
            if c != u32::MAX {
                boundary[c as usize] += vertex_boundary[v] as u64;
            }
        }
        let closed = |c: usize| boundary[c] as f64 <= 0.01 * 1.5 * faces[c] as f64;
        let candidates: Vec<usize> = (0..ns).filter(|&c| closed(c) && vol[c] < 0.0).collect();
        if candidates.is_empty() {
            return flip;
        }
        if candidates.len() > 256 {
            // Too many to test one by one: follow the overall orientation.
            let total: f64 = (0..ns).filter(|&c| closed(c)).map(|c| vol[c]).sum();
            if total < 0.0 {
                for &c in &candidates {
                    flip[c] = true;
                }
            } else {
                rep.cavity_shells += candidates.len();
            }
            rep.flipped_shells = flip.iter().filter(|f| **f).count();
            return flip;
        }
        let contains = |outer: usize, inner: usize| {
            outer != inner && (0..3).all(|k| lo[outer][k] <= lo[inner][k] && hi[outer][k] >= hi[inner][k])
        };
        // Faces per shell (only when needed for parity tests).
        let mut shell_faces: Option<Vec<Vec<u32>>> = None;
        let mut budget: u64 = 300_000_000;
        for &c in &candidates {
            let enclosers: Vec<usize> = (0..ns).filter(|&o| closed(o) && contains(o, c)).collect();
            if enclosers.is_empty() {
                flip[c] = true;
                continue;
            }
            let sf = shell_faces.get_or_insert_with(|| {
                let mut v: Vec<Vec<u32>> = vec![Vec::new(); ns];
                for (f, t) in mesh.triangles.iter().enumerate() {
                    v[comp[t[0] as usize] as usize].push(f as u32);
                }
                v
            });
            let cost: u64 = enclosers.iter().map(|&o| sf[o].len() as u64).sum();
            if cost > budget {
                // Too expensive to prove: assume an intentional cavity.
                rep.cavity_shells += 1;
                continue;
            }
            budget -= cost;
            let p = d3(mesh.positions[refv[c] as usize]);
            let dir = [0.5773502691896258, 0.5773519, 0.5773486];
            let crossings: usize = enclosers
                .iter()
                .map(|&o| {
                    sf[o]
                        .par_iter()
                        .with_min_len(4096)
                        .filter(|&&f| ray_hits(mesh, mesh.triangles[f as usize], p, dir))
                        .count()
                })
                .sum();
            if crossings % 2 == 1 {
                rep.cavity_shells += 1;
            } else {
                flip[c] = true;
            }
        }
        rep.flipped_shells = flip.iter().filter(|f| **f).count();
        flip
    }
}

/// Ray (origin p, direction d, t > 0) vs triangle, Möller–Trumbore in f64.
fn ray_hits(mesh: &Mesh, t: [u32; 3], p: D3, d: D3) -> bool {
    let a = d3(mesh.positions[t[0] as usize]);
    let b = d3(mesh.positions[t[1] as usize]);
    let c = d3(mesh.positions[t[2] as usize]);
    let e1 = sub_d(b, a);
    let e2 = sub_d(c, a);
    let pv = cross_d(d, e2);
    let det = dot_d(e1, pv);
    if det.abs() < 1e-300 {
        return false;
    }
    let inv = 1.0 / det;
    let tv = sub_d(p, a);
    let u = dot_d(tv, pv) * inv;
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = cross_d(tv, e1);
    let w = dot_d(d, q) * inv;
    if w < 0.0 || u + w > 1.0 {
        return false;
    }
    dot_d(e2, q) * inv > 0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth;
    use crate::util::dot;

    fn len(a: [f32; 3]) -> f32 {
        dot(a, a).sqrt()
    }

    #[test]
    fn sphere_normals_are_radial() {
        let mut m = synth::icosphere(5.0, 4);
        synth::unweld(&mut m);
        crate::weld::weld(&mut m, 1e-6);
        let r = compute(&mut m, 50.0);
        assert_eq!(r.split_vertices, 0, "{r:?}");
        assert_eq!(r.boundary_edges, 0);
        assert_eq!(m.normals.len(), m.positions.len());
        for (p, n) in m.positions.iter().zip(&m.normals) {
            let l = len(*p);
            let radial = [p[0] / l, p[1] / l, p[2] / l];
            assert!(dot(radial, *n) > 0.9995, "{p:?} {n:?}");
        }
    }

    #[test]
    fn cube_creases_keep_faces_flat() {
        let mut m = synth::subdivided_cuboid([2.0, 2.0, 2.0], 3);
        let r = compute(&mut m, 50.0);
        // 8 corners ×3, 12 edges × 2 inner vertices × 2.
        assert_eq!(r.split_vertices, 8 * 2 + 12 * 2, "{r:?}");
        for t in &m.triangles {
            let (fnrm, _) = crate::util::tri_normal_area(&m.positions, *t);
            for &v in t {
                assert!(dot(fnrm, m.normals[v as usize]) > 0.99999, "{:?} vs {:?}", fnrm, m.normals[v as usize]);
            }
        }
        // Positions of split copies coincide exactly with the originals.
        assert!((m.volume() - 8.0).abs() < 1e-4);
    }

    #[test]
    fn faceted_mode_with_zero_crease() {
        let mut m = synth::icosphere(1.0, 1);
        let nf = m.triangles.len();
        compute(&mut m, 0.0);
        assert_eq!(m.positions.len(), nf * 3);
    }

    #[test]
    fn inside_out_shell_is_flipped() {
        let mut m = synth::icosphere(3.0, 3);
        for t in &mut m.triangles {
            t.swap(1, 2);
        }
        let r = compute(&mut m, 50.0);
        assert_eq!(r.flipped_shells, 1, "{r:?}");
        for (p, n) in m.positions.iter().zip(&m.normals) {
            assert!(dot(*p, *n) > 0.0);
        }
    }

    #[test]
    fn hollow_cavity_is_kept_inward() {
        let outer = synth::icosphere(5.0, 3);
        let mut inner = synth::icosphere(3.0, 3);
        for t in &mut inner.triangles {
            t.swap(1, 2);
        }
        let mut m = synth::combine(vec![outer, inner], "hollow");
        let r = compute(&mut m, 50.0);
        assert_eq!(r.flipped_shells, 0, "{r:?}");
        assert_eq!(r.cavity_shells, 1);
        let v = 4.0 / 3.0 * std::f64::consts::PI * (125.0 - 27.0);
        assert!((m.volume() - v).abs() / v < 0.03, "{} vs {v}", m.volume());
    }

    #[test]
    fn inconsistent_faces_are_repaired() {
        let mut m = synth::icosphere(2.0, 3);
        for (i, t) in m.triangles.iter_mut().enumerate() {
            if crate::util::hash32(i as u32).is_multiple_of(7) {
                t.swap(1, 2);
            }
        }
        let r = compute(&mut m, 50.0);
        assert!(r.repaired_faces > 0);
        assert_eq!(r.inconsistent_edges, 0);
        assert_eq!(r.split_vertices, 0, "{r:?}");
        for (p, n) in m.positions.iter().zip(&m.normals) {
            let l = len(*p);
            assert!(dot([p[0] / l, p[1] / l, p[2] / l], *n) > 0.999);
        }
        assert!(crate::synth::signed_volume(&m) > 0.0);
    }

    #[test]
    fn per_part_crease_facets_gems_only() {
        let metal = synth::icosphere(5.0, 3);
        let mut gem = synth::icosphere(1.0, 2);
        synth::translate(&mut gem, [10.0, 0.0, 0.0]);
        let mut m = Mesh::default();
        m.merge(metal);
        m.merge(gem);
        m.parts[0].name = "metal".into();
        m.parts[1].material = Some("gem".into());
        let gem_tris = (m.parts[1].triangles.end - m.parts[1].triangles.start) as usize;
        let r = compute(&mut m, 50.0);
        assert_eq!(r.split_vertices, 0);
        let r = recompute(&mut m, |p| if p.material.as_deref() == Some("gem") { 0.0 } else { 50.0 });
        // Every gem corner gets its own vertex; the metal stays smooth.
        let gem_vertices = 162; // icosphere(…, 2) has 162 vertices
        assert_eq!(r.split_vertices, gem_tris * 3 - gem_vertices, "{r:?}");
        for t in &m.triangles[m.parts[1].triangles.start as usize..] {
            let (fnrm, _) = crate::util::tri_normal_area(&m.positions, *t);
            for &v in t {
                assert!(dot(fnrm, m.normals[v as usize]) > 0.99999);
            }
        }
    }

    #[test]
    fn broken_fan_on_flat_surface_has_no_seam() {
        // Centre vertex whose fan is cut twice by unwelded duplicates (b/b2,
        // e/e2): two fans with identical normals must merge again.
        let mut m = Mesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [-1.0, 0.0, 0.0],
                [0.0, -1.0, 0.0],
                [0.0, -1.0, 0.0],
            ],
            normals: vec![],
            triangles: vec![[0, 1, 2], [0, 3, 4], [0, 4, 5], [0, 6, 1]],
            parts: vec![],
        };
        let r = compute(&mut m, 50.0);
        assert_eq!(r.split_vertices, 0, "{r:?}");
        for n in &m.normals {
            assert!(n[2] > 0.99999);
        }
    }
}
