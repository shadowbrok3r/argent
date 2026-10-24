//! Garland–Heckbert quadric-error (QEM) edge-collapse simplification.
//!
//! - **Setup (parallel).** Vertices are welded exactly by position *per
//!   part*, so the crease-split copies argent-mesh makes merge again while
//!   two parts never share a vertex (stones stay separate meshes). Every
//!   vertex gets the area-weighted plane quadrics of its faces plus a
//!   weighted plane through each boundary edge, perpendicular to its face.
//!   Quadrics are stored relative to their own vertex, so costs keep full
//!   f64 precision anywhere in the model. Vertices on non-manifold or
//!   inconsistently wound edges, bow-tie vertices, positions shared by two
//!   parts and vertices of frozen parts are locked.
//! - **Placement.** The quadric's exact minimiser when it is well
//!   conditioned, a regularised minimiser grown from the edge midpoint (a
//!   singular quadric — a flat or cylindrical patch — keeps the midpoint
//!   along its free directions), then the midpoint, the best point on the
//!   edge and both endpoints. The cheapest candidate that passes the checks
//!   wins.
//! - **Loop (serial).** A lazy-deletion radix heap keyed by cost. Entries
//!   carry a time stamp and are skipped once either vertex has changed. A
//!   collapse must keep the link condition (no new non-manifold edge, no
//!   duplicate face), must not pinch two boundary points together, and no
//!   surviving neighbour triangle may flip, turn by more than ~75° in one
//!   step or more than 90° from its original orientation, degenerate or
//!   become a worse sliver.

use crate::math::{D3, add, cross, d3, dot, len, len2, max_edge2, normal_area2, scale, sub};
use crate::parts::PartTable;
use argent_mesh::{Mesh, Part};
use rayon::prelude::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const BOUNDARY: u8 = 1;
const LOCKED: u8 = 2;
const DEAD: u8 = 4;

/// Boundary-plane weight per squared edge length (faces weigh their area).
const BOUNDARY_WEIGHT: f64 = 100.0;
/// A surviving neighbour's normal may turn by at most acos(0.25) ≈ 75.5°.
const FOLD_COS: f64 = 0.25;
/// ... and may never turn by more than acos(ORIG_COS) from its *original*
/// orientation, so small turns cannot add up to a flip.
const ORIG_COS: f64 = 0.0;
/// `2·area / longest_edge²` below this is a degenerate triangle.
const DEGENERATE_Q: f64 = 1e-6;
/// Below this a triangle is a sliver, and a collapse may not make it worse.
const NEEDLE_Q: f64 = 1e-3;
/// Tikhonov term of the regularised placement, as a fraction of trace(A).
/// Swept on Aster and Tessera (2.6 M → 150 k): 1e-2 gave the lowest mean
/// deviation (1e-3: +31 %, 1e-1: +3 %, 1: +23 %).
const PINV_EPS: f64 = 1e-2;
/// Tiny length term so zero-cost (flat) collapses go shortest-first.
const LENGTH_REG: f64 = 1e-9;
/// Costs below `NOISE · trace(A) · |edge|²` are rounding noise (= 0).
const NOISE: f64 = 1e-12;
/// Low bits of a heap stamp: collapse time; high bits: first candidate.
const TIME_BITS: u32 = 29;
const TIME_MASK: u32 = (1 << TIME_BITS) - 1;

// ---------------------------------------------------------------------------
// Quadrics and placement
// ---------------------------------------------------------------------------

/// `x > t`, false when either is NaN, so `!gt(x, t)` also rejects NaN.
#[inline]
fn gt(x: f64, t: f64) -> bool {
    x > t
}

/// `E(y) = yᵀAy + 2bᵀy + c`, with `y` measured from the owning vertex.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Quadric {
    /// Symmetric A: xx, xy, xz, yy, yz, zz.
    a: [f64; 6],
    b: D3,
    c: f64,
}

impl Quadric {
    /// Add `w · (n·y + d)²`.
    #[inline]
    fn add_plane(&mut self, n: D3, d: f64, w: f64) {
        let a = &mut self.a;
        a[0] += w * n[0] * n[0];
        a[1] += w * n[0] * n[1];
        a[2] += w * n[0] * n[2];
        a[3] += w * n[1] * n[1];
        a[4] += w * n[1] * n[2];
        a[5] += w * n[2] * n[2];
        self.b = add(self.b, scale(n, w * d));
        self.c += w * d * d;
    }

    #[inline]
    fn plus(&self, o: &Quadric) -> Quadric {
        let mut a = self.a;
        for (x, y) in a.iter_mut().zip(o.a) {
            *x += y;
        }
        Quadric { a, b: add(self.b, o.b), c: self.c + o.c }
    }

    #[inline]
    fn av(&self, y: D3) -> D3 {
        let a = &self.a;
        [
            a[0] * y[0] + a[1] * y[1] + a[2] * y[2],
            a[1] * y[0] + a[3] * y[1] + a[4] * y[2],
            a[2] * y[0] + a[4] * y[1] + a[5] * y[2],
        ]
    }

    #[inline]
    fn eval(&self, y: D3) -> f64 {
        dot(y, self.av(y)) + 2.0 * dot(self.b, y) + self.c
    }

    /// The same quadric with its origin moved by `t`: `Q'(y) = Q(y + t)`.
    #[inline]
    fn translated(&self, t: D3) -> Quadric {
        let at = self.av(t);
        Quadric { a: self.a, b: add(self.b, at), c: self.c + 2.0 * dot(self.b, t) + dot(t, at) }
    }

    #[inline]
    fn trace(&self) -> f64 {
        self.a[0] + self.a[3] + self.a[5]
    }
}

/// Solve `(A + εI) z = −b − A·m` for the step `z` from `m` (cofactors).
/// With `ε = 0` this is the exact minimiser of a non-singular quadric; with
/// `ε` a small fraction of trace(A) it is a soft truncated pseudo-inverse:
/// well-determined directions get their full Newton step, directions with
/// eigenvalues far below `ε` (flat or cylindrical patches) stay at `m`.
fn solve_from(q: &Quadric, m: D3, eps: f64) -> Option<D3> {
    let am = q.av(m);
    let r = [-(q.b[0] + am[0]), -(q.b[1] + am[1]), -(q.b[2] + am[2])];
    let a = &q.a;
    let (xx, xy, xz, yy, yz, zz) = (a[0] + eps, a[1], a[2], a[3] + eps, a[4], a[5] + eps);
    let c00 = yy * zz - yz * yz;
    let c01 = xz * yz - xy * zz;
    let c02 = xy * yz - xz * yy;
    let det = xx * c00 + xy * c01 + xz * c02;
    let tr = xx + yy + zz;
    // Reject a near-singular matrix: for a positive semi-definite one,
    // det/trace³ ≤ λmin/λmax. (The regularised solve always passes.)
    if !gt(det, 4e-6 * tr * tr * tr) {
        return None;
    }
    let c11 = xx * zz - xz * xz;
    let c12 = xy * xz - xx * yz;
    let c22 = xx * yy - xy * xy;
    let inv = 1.0 / det;
    let z = [
        (c00 * r[0] + c01 * r[1] + c02 * r[2]) * inv,
        (c01 * r[0] + c11 * r[1] + c12 * r[2]) * inv,
        (c02 * r[0] + c12 * r[1] + c22 * r[2]) * inv,
    ];
    let y = add(m, z);
    y.iter().all(|x| x.is_finite()).then_some(y)
}

/// Placement candidates for one edge, cheapest first. Positions are
/// relative to the edge's first vertex `a`; `d = p_b − p_a`.
#[derive(Clone, Copy)]
struct Cands {
    y: [D3; 6],
    c: [f64; 6],
    n: usize,
    reg: f64,
}

impl Cands {
    #[inline]
    fn key(&self, i: usize) -> u32 {
        cost_key(self.c[i] + self.reg)
    }
}

#[inline]
fn cost_key(c: f64) -> u32 {
    let f = c as f32;
    if f.is_nan() { u32::MAX } else { f.max(0.0).to_bits() }
}

fn candidates(q: &Quadric, d: D3) -> Cands {
    let l2 = len2(d);
    let tr = q.trace();
    let floor = NOISE * tr * l2;
    let mut out = Cands { y: [[0.0; 3]; 6], c: [0.0; 6], n: 0, reg: LENGTH_REG * tr * l2 };
    let mut push = |y: D3| {
        let c = q.eval(y);
        out.y[out.n] = y;
        out.c[out.n] = if c.is_nan() {
            f64::INFINITY
        } else if c > floor {
            c
        } else {
            0.0
        };
        out.n += 1;
    };
    let mid = scale(d, 0.5);
    // Exact optimum (if well-conditioned), then the regularised one; a
    // solution more than two edge lengths away is not trusted.
    for eps in [0.0, PINV_EPS * tr] {
        if let Some(y) = solve_from(q, mid, eps)
            && len2(sub(y, mid)) <= 4.0 * l2
        {
            push(y);
        }
    }
    push(mid);
    let alpha = dot(d, q.av(d));
    if alpha > 0.0 {
        let t = (-dot(q.b, d) / alpha).clamp(0.0, 1.0);
        if t > 1e-6 && t < 1.0 - 1e-6 && (t - 0.5).abs() > 1e-6 {
            push(scale(d, t));
        }
    }
    push([0.0; 3]);
    push(d);
    // Stable insertion sort: equal costs keep the preference order above.
    for i in 1..out.n {
        let mut j = i;
        while j > 0 && out.c[j] < out.c[j - 1] {
            out.c.swap(j, j - 1);
            out.y.swap(j, j - 1);
            j -= 1;
        }
    }
    out
}

/// The summed quadric of edge `(a, b)` in `a`'s frame, and its candidates.
#[inline]
fn edge_eval(quad: &[Quadric], pos: &[D3], a: u32, b: u32) -> (Quadric, Cands) {
    let (pa, pb) = (pos[a as usize], pos[b as usize]);
    let q = quad[a as usize].plus(&quad[b as usize].translated(sub(pa, pb)));
    let c = candidates(&q, sub(pb, pa));
    (q, c)
}

// ---------------------------------------------------------------------------
// Heap
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Entry {
    /// `f32` bits of a non-negative cost (monotone as an integer).
    cost: u32,
    a: u32,
    b: u32,
    stamp: u32,
}

/// Lazy-deletion priority queue: a monotone radix heap (Ahuja, Mehlhorn,
/// Orlin & Tarjan) over the `u32` cost keys. Bucket `i > 0` holds keys whose
/// highest bit differing from the last popped key is bit `i − 1`; popping
/// redistributes the lowest non-empty bucket around its minimum. Every
/// operation is a sequential `Vec` push or pop, so it stays cache-friendly
/// at ten million entries, where a binary heap's sift-down misses cache on
/// every level. A key below the last popped one (rare: a collapse made a
/// neighbouring edge cheaper than anything queued) is clamped up to it, so
/// it simply comes out next.
struct RadixHeap {
    last: u32,
    buckets: Vec<Vec<Entry>>,
}

impl RadixHeap {
    fn with_entries(entries: Vec<Entry>) -> RadixHeap {
        let mut h = RadixHeap { last: 0, buckets: (0..33).map(|_| Vec::new()).collect() };
        for e in entries {
            h.push(e);
        }
        h
    }

    #[inline]
    fn bucket(last: u32, key: u32) -> usize {
        (32 - (key ^ last).leading_zeros()) as usize
    }

    #[inline]
    fn push(&mut self, mut e: Entry) {
        e.cost = e.cost.max(self.last);
        self.buckets[Self::bucket(self.last, e.cost)].push(e);
    }

    /// Make bucket 0 (keys equal to `last`, the minimum) non-empty.
    fn refill(&mut self) -> bool {
        if !self.buckets[0].is_empty() {
            return true;
        }
        let Some(i) = (1..33).find(|&i| !self.buckets[i].is_empty()) else { return false };
        let mut b = std::mem::take(&mut self.buckets[i]);
        self.last = b.iter().map(|e| e.cost).min().unwrap_or(self.last);
        for e in b.drain(..) {
            self.buckets[Self::bucket(self.last, e.cost)].push(e);
        }
        self.buckets[i] = b;
        true
    }

    #[inline]
    fn pop(&mut self) -> Option<Entry> {
        if !self.refill() {
            return None;
        }
        self.buckets[0].pop()
    }

    /// Smallest queued key.
    fn min_key(&mut self) -> Option<u32> {
        self.refill().then_some(self.last)
    }
}

// ---------------------------------------------------------------------------
// Simplifier
// ---------------------------------------------------------------------------

/// What [`simplify_with_stats`] did.
#[derive(Clone, Debug, Default)]
pub struct SimplifyStats {
    /// Edge collapses performed.
    pub collapses: usize,
    /// Heap entries skipped because a vertex changed after they were queued.
    pub stale: usize,
    /// Collapses refused by the link condition or boundary pinching.
    pub rejected_topology: usize,
    /// Collapses refused because a neighbour would flip, fold or degenerate.
    pub rejected_fold: usize,
    /// Welded vertices that may not move (non-manifold, bow-tie, part seam,
    /// frozen part).
    pub locked_vertices: usize,
    pub boundary_vertices: usize,
    /// Largest quadric error of a performed collapse (area-weighted mm²·mm²).
    pub max_qem_error: f64,
    /// Weld, adjacency, quadrics and initial costs.
    pub setup: Duration,
    /// The collapse loop.
    pub collapse: Duration,
}

pub(crate) struct Simplifier {
    /// Positions are stored relative to `center` (bounding-box centre).
    center: D3,
    pos: Vec<D3>,
    quad: Vec<Quadric>,
    tris: Vec<[u32; 3]>,
    /// Unit normal of every face as it was in the input (zero if degenerate).
    onorm: Vec<[f32; 3]>,
    tri_part: Vec<u32>,
    alive: Vec<bool>,
    alive_tris: usize,
    /// Vertex → faces: `refs[vstart[v] .. vstart[v] + vcount[v]]`; a
    /// survivor's list is rewritten at the end, dead faces are skipped.
    vstart: Vec<u32>,
    vcount: Vec<u32>,
    refs: Vec<u32>,
    refs_limit: usize,
    flags: Vec<u8>,
    stamp: Vec<u32>,
    /// Collapse provenance: a removed vertex points at its survivor.
    parent: Vec<u32>,
    heap: RadixHeap,
    time: u32,
    input_to_weld: Vec<u32>,
    names: Vec<String>,
    materials: Vec<Option<String>>,
    fa: Vec<u32>,
    fb: Vec<u32>,
    link: Vec<u32>,
    nbuf: Vec<u32>,
    lbuf: Vec<u32>,
    pub(crate) stats: SimplifyStats,
}

#[derive(Default)]
struct SetupScratch {
    ev: Vec<(u32, u32, bool)>,
    uf: Vec<u32>,
}

#[inline]
fn canon(x: f32) -> u32 {
    if x == 0.0 { 0 } else { x.to_bits() }
}

fn uf_find(p: &mut [u32], mut x: u32) -> u32 {
    while p[x as usize] != x {
        p[x as usize] = p[p[x as usize] as usize];
        x = p[x as usize];
    }
    x
}

fn uf_union(p: &mut [u32], a: u32, b: u32) -> bool {
    let (ra, rb) = (uf_find(p, a), uf_find(p, b));
    if ra == rb {
        return false;
    }
    p[ra.max(rb) as usize] = ra.min(rb);
    true
}

/// Quadric and flags of vertex `v` (its faces, sorted ascending).
fn vertex_setup(v: u32, faces: &[u32], tris: &[[u32; 3]], pos: &[D3], s: &mut SetupScratch) -> (Quadric, u8) {
    let mut q = Quadric::default();
    if faces.is_empty() {
        return (q, DEAD);
    }
    let mut flags = 0u8;
    let pv = pos[v as usize];
    s.ev.clear();
    for (li, &f) in faces.iter().enumerate() {
        let t = tris[f as usize];
        let j = if t[0] == v {
            0
        } else if t[1] == v {
            1
        } else {
            2
        };
        let (n, a2) = normal_area2(pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
        if a2 > 0.0 {
            // The face plane passes through v: d = 0 in v's frame.
            q.add_plane(n, 0.0, 0.5 * a2);
        }
        s.ev.push((t[(j + 1) % 3], li as u32, true));
        s.ev.push((t[(j + 2) % 3], li as u32, false));
    }
    s.ev.sort_unstable();
    s.uf.clear();
    s.uf.extend(0..faces.len() as u32);
    let mut unions = 0usize;
    let mut i = 0;
    while i < s.ev.len() {
        let w = s.ev[i].0;
        let mut j = i + 1;
        while j < s.ev.len() && s.ev[j].0 == w {
            j += 1;
        }
        match j - i {
            1 => {
                flags |= BOUNDARY;
                let t = tris[faces[s.ev[i].1 as usize] as usize];
                let (n, a2) = normal_area2(pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
                if a2 > 0.0 {
                    let e = sub(pos[w as usize], pv);
                    let bn = cross(e, n);
                    let l = len(bn);
                    if l > 0.0 {
                        q.add_plane(scale(bn, 1.0 / l), 0.0, BOUNDARY_WEIGHT * len2(e));
                    }
                }
            }
            2 => {
                if s.ev[i].2 == s.ev[i + 1].2 {
                    flags |= LOCKED; // inconsistent winding across this edge
                } else if uf_union(&mut s.uf, s.ev[i].1, s.ev[i + 1].1) {
                    unions += 1;
                }
            }
            _ => flags |= LOCKED, // non-manifold edge
        }
        i = j;
    }
    if faces.len() - unions > 1 {
        flags |= LOCKED; // bow-tie: several fans meet at this vertex
    }
    (q, flags)
}

impl Simplifier {
    /// Weld, build adjacency and quadrics, and queue every edge.
    /// `frozen[p]` keeps part `p` verbatim.
    pub(crate) fn new(mesh: &Mesh, table: &PartTable, frozen: &[bool]) -> Simplifier {
        let t0 = Instant::now();
        let nv_in = mesh.positions.len();

        // -- exact weld by position bits (-0.0 == +0.0) ----------------------
        let mut order: Vec<([u32; 3], u32)> = mesh
            .positions
            .par_iter()
            .enumerate()
            .filter(|(_, p)| p.iter().all(|x| x.is_finite()))
            .map(|(i, p)| (p.map(canon), i as u32))
            .collect();
        order.par_sort_unstable();
        let mut pos_id = vec![u32::MAX; nv_in];
        let mut n_pos = 0u32;
        for i in 0..order.len() {
            if i > 0 && order[i].0 != order[i - 1].0 {
                n_pos += 1;
            }
            pos_id[order[i].1 as usize] = n_pos;
        }
        if !order.is_empty() {
            n_pos += 1;
        }
        drop(order);

        // -- one welded vertex per (part, position) --------------------------
        let nt_in = mesh.triangles.len();
        let mut owner = vec![u32::MAX; n_pos as usize];
        let mut first_wid = vec![u32::MAX; n_pos as usize];
        let mut shared: HashMap<(u32, u32), u32> = HashMap::new();
        let mut wsrc: Vec<u32> = Vec::with_capacity(nv_in.min(nt_in));
        let mut flags: Vec<u8> = Vec::with_capacity(nv_in.min(nt_in));
        let mut input_to_weld = vec![u32::MAX; nv_in];
        let mut tris: Vec<[u32; 3]> = Vec::with_capacity(nt_in);
        let mut tri_part: Vec<u32> = Vec::with_capacity(nt_in);
        'tri: for (f, t) in mesh.triangles.iter().enumerate() {
            let part = table.tri_part[f];
            let lock = if frozen.get(part as usize).copied().unwrap_or(false) { LOCKED } else { 0 };
            let mut w = [0u32; 3];
            for k in 0..3 {
                let vi = t[k] as usize;
                if vi >= nv_in || pos_id[vi] == u32::MAX {
                    continue 'tri;
                }
                let pid = pos_id[vi] as usize;
                w[k] = if owner[pid] == u32::MAX {
                    owner[pid] = part;
                    let id = wsrc.len() as u32;
                    wsrc.push(vi as u32);
                    flags.push(lock);
                    first_wid[pid] = id;
                    id
                } else if owner[pid] == part {
                    first_wid[pid]
                } else {
                    // Two parts touch here: keep both copies fixed so no crack opens.
                    flags[first_wid[pid] as usize] |= LOCKED;
                    *shared.entry((part, pid as u32)).or_insert_with(|| {
                        let id = wsrc.len() as u32;
                        wsrc.push(vi as u32);
                        flags.push(LOCKED);
                        id
                    })
                };
            }
            for k in 0..3 {
                let vi = t[k] as usize;
                if input_to_weld[vi] == u32::MAX {
                    input_to_weld[vi] = w[k];
                }
            }
            if w[0] != w[1] && w[1] != w[2] && w[0] != w[2] {
                tris.push(w);
                tri_part.push(part);
            }
        }
        drop((owner, first_wid, pos_id, shared));
        let nw = wsrc.len();
        let nt = tris.len();

        // -- positions about the bounding-box centre --------------------------
        let mut lo = [f64::MAX; 3];
        let mut hi = [f64::MIN; 3];
        for &i in &wsrc {
            let p = d3(mesh.positions[i as usize]);
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let center = if nw > 0 { scale(add(lo, hi), 0.5) } else { [0.0; 3] };
        let pos: Vec<D3> = wsrc.par_iter().map(|&i| sub(d3(mesh.positions[i as usize]), center)).collect();
        drop(wsrc);

        // -- vertex → face lists (faces ascending) ----------------------------
        let mut vcount = vec![0u32; nw];
        for t in &tris {
            for &v in t {
                vcount[v as usize] += 1;
            }
        }
        let mut vstart = vec![0u32; nw];
        let mut acc = 0u32;
        for v in 0..nw {
            vstart[v] = acc;
            acc += vcount[v];
        }
        let mut refs = vec![0u32; acc as usize];
        let mut cursor = vstart.clone();
        for (f, t) in tris.iter().enumerate() {
            for &v in t {
                refs[cursor[v as usize] as usize] = f as u32;
                cursor[v as usize] += 1;
            }
        }
        drop(cursor);

        // -- quadrics and flags (parallel) -------------------------------------
        let setup: Vec<(Quadric, u8)> = (0..nw)
            .into_par_iter()
            .with_min_len(512)
            .map_init(SetupScratch::default, |s, v| {
                let r = vstart[v] as usize..(vstart[v] + vcount[v]) as usize;
                vertex_setup(v as u32, &refs[r], &tris, &pos, s)
            })
            .collect();
        let mut quad = Vec::with_capacity(nw);
        for (v, (q, fl)) in setup.into_iter().enumerate() {
            quad.push(q);
            flags[v] |= fl;
        }
        let mut stats = SimplifyStats {
            locked_vertices: flags.iter().filter(|f| **f & (LOCKED | DEAD) == LOCKED).count(),
            boundary_vertices: flags.iter().filter(|f| **f & BOUNDARY != 0).count(),
            ..Default::default()
        };

        // -- every free edge, costed (parallel) --------------------------------
        let entries: Vec<Entry> = (0..nw as u32)
            .into_par_iter()
            .with_min_len(512)
            .flat_map_iter(|v| {
                let mut out = Vec::new();
                if flags[v as usize] & (LOCKED | DEAD) == 0 {
                    let r = vstart[v as usize] as usize..(vstart[v as usize] + vcount[v as usize]) as usize;
                    let mut nb: Vec<u32> = Vec::with_capacity(16);
                    for &f in &refs[r] {
                        for &w in &tris[f as usize] {
                            if w > v && flags[w as usize] & (LOCKED | DEAD) == 0 {
                                nb.push(w);
                            }
                        }
                    }
                    nb.sort_unstable();
                    nb.dedup();
                    out.reserve(nb.len());
                    for w in nb {
                        let (_, c) = edge_eval(&quad, &pos, v, w);
                        out.push(Entry { cost: c.key(0), a: v, b: w, stamp: 0 });
                    }
                }
                out
            })
            .collect();
        let heap = RadixHeap::with_entries(entries);
        let onorm: Vec<[f32; 3]> = tris
            .par_iter()
            .map(|t| {
                let (n, _) = normal_area2(pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
                [n[0] as f32, n[1] as f32, n[2] as f32]
            })
            .collect();
        stats.setup = t0.elapsed();
        let refs_limit = refs.len() * 3 + (1 << 20);
        Simplifier {
            center,
            pos,
            quad,
            alive: vec![true; nt],
            alive_tris: nt,
            tris,
            onorm,
            tri_part,
            vstart,
            vcount,
            refs,
            refs_limit,
            stamp: vec![0; nw],
            parent: vec![u32::MAX; nw],
            flags,
            heap,
            time: 0,
            input_to_weld,
            names: table.names.clone(),
            materials: table.materials.clone(),
            fa: Vec::new(),
            fb: Vec::new(),
            link: Vec::new(),
            nbuf: Vec::new(),
            lbuf: Vec::new(),
            stats,
        }
    }

    pub(crate) fn alive_triangles(&self) -> usize {
        self.alive_tris
    }

    #[inline]
    fn range(&self, v: u32) -> std::ops::Range<usize> {
        let s = self.vstart[v as usize] as usize;
        s..s + self.vcount[v as usize] as usize
    }

    /// Collapse edges, cheapest first, until at most `target` triangles are
    /// left or no legal collapse remains. Can be called again with a smaller
    /// target to continue.
    pub(crate) fn run(&mut self, target: usize) {
        let t0 = Instant::now();
        while self.alive_tris > target && self.time < TIME_MASK {
            let Some(e) = self.heap.pop() else { break };
            let (a, b) = (e.a, e.b);
            let (au, bu) = (a as usize, b as usize);
            let t = e.stamp & TIME_MASK;
            if (self.flags[au] | self.flags[bu]) & DEAD != 0 || self.stamp[au] > t || self.stamp[bu] > t {
                self.stats.stale += 1;
                continue;
            }
            if !self.topology_ok(a, b) {
                self.stats.rejected_topology += 1;
                continue;
            }
            let (q, c) = edge_eval(&self.quad, &self.pos, a, b);
            let skip = ((e.stamp >> TIME_BITS) as usize).min(c.n);
            let pa = self.pos[au];
            let Some(i) = (skip..c.n).find(|&i| self.fold_ok(a, b, add(pa, c.y[i]))) else {
                self.stats.rejected_fold += 1;
                continue;
            };
            if i > skip {
                // The cheapest placement folds: requeue at the real cost if a
                // cheaper edge is waiting.
                let key = c.key(i);
                if key > e.cost && self.heap.min_key().is_some_and(|top| top < key) {
                    self.heap.push(Entry { cost: key, a, b, stamp: t | ((i as u32) << TIME_BITS) });
                    continue;
                }
            }
            self.collapse(a, b, c.y[i], &q, c.c[i]);
        }
        self.stats.collapse += t0.elapsed();
    }

    fn topology_ok(&mut self, a: u32, b: u32) -> bool {
        let mut fa = std::mem::take(&mut self.fa);
        let mut fb = std::mem::take(&mut self.fb);
        let mut link = std::mem::take(&mut self.link);
        let ok = self.topology_check(a, b, &mut fa, &mut fb, &mut link);
        self.fa = fa;
        self.fb = fb;
        self.link = link;
        ok
    }

    /// Fills `fa`/`fb` with the faces of `a`/`b` that survive the collapse
    /// and checks the link condition.
    fn topology_check(&self, a: u32, b: u32, fa: &mut Vec<u32>, fb: &mut Vec<u32>, link: &mut Vec<u32>) -> bool {
        fa.clear();
        fb.clear();
        let mut shared = 0usize;
        let mut opp = [u32::MAX; 2];
        for i in self.range(a) {
            let f = self.refs[i];
            if !self.alive[f as usize] {
                continue;
            }
            let t = self.tris[f as usize];
            if t.contains(&b) {
                if shared < 2 {
                    opp[shared] = t[0] ^ t[1] ^ t[2] ^ a ^ b;
                }
                shared += 1;
            } else {
                fa.push(f);
            }
        }
        if shared == 0 || shared > 2 || (shared == 2 && opp[0] == opp[1]) {
            return false;
        }
        // An interior edge joining two boundary vertices would pinch the surface.
        if shared == 2 && self.flags[a as usize] & BOUNDARY != 0 && self.flags[b as usize] & BOUNDARY != 0 {
            return false;
        }
        for i in self.range(b) {
            let f = self.refs[i];
            if self.alive[f as usize] && !self.tris[f as usize].contains(&a) {
                fb.push(f);
            }
        }
        // Vertex links: the only common neighbours may be the opposite vertices.
        link.clear();
        for &f in fa.iter() {
            link.extend(self.tris[f as usize].iter().copied().filter(|&x| x != a));
        }
        link.sort_unstable();
        link.dedup();
        for &f in fb.iter() {
            for &x in &self.tris[f as usize] {
                if x != b && x != opp[0] && x != opp[1] && link.binary_search(&x).is_ok() {
                    return false;
                }
            }
        }
        // Edge links: (a, c, d) and (b, c, d) both present = a tetrahedron.
        if shared == 2 {
            let (c, d) = (opp[0], opp[1]);
            let has = |faces: &[u32]| {
                faces.iter().any(|&f| {
                    let t = self.tris[f as usize];
                    t.contains(&c) && t.contains(&d)
                })
            };
            if has(fa) && has(fb) {
                return false;
            }
        }
        true
    }

    /// No surviving neighbour may flip, turn past `FOLD_COS`, degenerate or
    /// become a worse sliver when `a` and `b` move to `p`.
    fn fold_ok(&self, a: u32, b: u32, p: D3) -> bool {
        for &f in self.fa.iter().chain(self.fb.iter()) {
            let t = self.tris[f as usize];
            let v = t.map(|i| self.pos[i as usize]);
            let mut w = v;
            for k in 0..3 {
                if t[k] == a || t[k] == b {
                    w[k] = p;
                }
            }
            let n0 = cross(sub(v[1], v[0]), sub(v[2], v[0]));
            let l0 = len(n0);
            let e0 = max_edge2(&v);
            if !gt(l0, DEGENERATE_Q * e0) {
                continue; // already degenerate: nothing to protect
            }
            let n1 = cross(sub(w[1], w[0]), sub(w[2], w[0]));
            let l1 = len(n1);
            let e1 = max_edge2(&w);
            if !gt(l1, DEGENERATE_Q * e1) || dot(n0, n1) < FOLD_COS * l0 * l1 {
                return false;
            }
            let o = self.onorm[f as usize];
            if dot(n1, [o[0] as f64, o[1] as f64, o[2] as f64]) < ORIG_COS * l1 {
                return false;
            }
            let q1 = l1 / e1;
            if q1 < NEEDLE_Q && q1 < l0 / e0 {
                return false;
            }
        }
        true
    }

    /// Collapse `b` into `a` at `pos[a] + y`; `q` is the edge quadric in
    /// `a`'s frame.
    fn collapse(&mut self, a: u32, b: u32, y: D3, q: &Quadric, cost: f64) {
        let (k, r) = (a, b);
        let (ku, ru) = (k as usize, r as usize);
        let mut list = std::mem::take(&mut self.lbuf);
        list.clear();
        for i in self.range(k) {
            let f = self.refs[i] as usize;
            if !self.alive[f] {
                continue;
            }
            if self.tris[f].contains(&r) {
                self.alive[f] = false;
                self.alive_tris -= 1;
            } else {
                list.push(f as u32);
            }
        }
        for i in self.range(r) {
            let f = self.refs[i] as usize;
            if !self.alive[f] {
                continue;
            }
            for c in self.tris[f].iter_mut() {
                if *c == r {
                    *c = k;
                }
            }
            list.push(f as u32);
        }
        self.pos[ku] = add(self.pos[ku], y);
        self.quad[ku] = q.translated(y);
        self.flags[ku] |= self.flags[ru] & BOUNDARY;
        self.flags[ru] |= DEAD;
        self.parent[ru] = k;
        if self.refs.len() + list.len() > self.refs_limit {
            self.compact_refs();
        }
        self.vstart[ku] = self.refs.len() as u32;
        self.vcount[ku] = list.len() as u32;
        self.refs.extend_from_slice(&list);
        self.lbuf = list;
        self.time += 1;
        self.stamp[ku] = self.time;
        self.stats.collapses += 1;
        if cost > self.stats.max_qem_error {
            self.stats.max_qem_error = cost;
        }
        self.push_edges(k);
    }

    /// Re-cost every free edge around `k`.
    fn push_edges(&mut self, k: u32) {
        let mut nb = std::mem::take(&mut self.nbuf);
        nb.clear();
        for i in self.range(k) {
            let f = self.refs[i] as usize;
            nb.extend(self.tris[f].iter().copied().filter(|&w| w != k));
        }
        nb.sort_unstable();
        nb.dedup();
        for &w in &nb {
            if self.flags[w as usize] & (LOCKED | DEAD) != 0 {
                continue;
            }
            let (a, b) = (k.min(w), k.max(w));
            let (_, c) = edge_eval(&self.quad, &self.pos, a, b);
            self.heap.push(Entry { cost: c.key(0), a, b, stamp: self.time });
        }
        self.nbuf = nb;
    }

    fn compact_refs(&mut self) {
        let mut out = Vec::with_capacity(self.alive_tris * 3 + 1024);
        for v in 0..self.pos.len() {
            if self.flags[v] & DEAD != 0 {
                self.vcount[v] = 0;
                continue;
            }
            let start = out.len();
            for i in self.range(v as u32) {
                let f = self.refs[i];
                if self.alive[f as usize] {
                    out.push(f);
                }
            }
            self.vstart[v] = start as u32;
            self.vcount[v] = (out.len() - start) as u32;
        }
        self.refs = out;
        self.refs_limit = self.refs_limit.max(self.refs.len() * 2 + (1 << 20));
    }

    /// The current mesh (parts in input order, each a contiguous run of
    /// triangles and of vertices; smooth normals) and the map from every
    /// input vertex to its output vertex (`u32::MAX` when unreferenced).
    pub(crate) fn snapshot(&self) -> (Mesh, Vec<u32>) {
        let nw = self.pos.len();
        let nparts = self.names.len();
        let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); nparts];
        for (f, &alive) in self.alive.iter().enumerate() {
            if alive {
                buckets[self.tri_part[f] as usize].push(f as u32);
            }
        }
        let mut new_id = vec![u32::MAX; nw];
        let mut positions: Vec<[f32; 3]> = Vec::new();
        let mut triangles: Vec<[u32; 3]> = Vec::with_capacity(self.alive_tris);
        let mut parts = Vec::with_capacity(nparts);
        for (p, faces) in buckets.iter().enumerate() {
            let start = triangles.len() as u32;
            for &f in faces {
                let t = self.tris[f as usize];
                let mut o = [0u32; 3];
                for k in 0..3 {
                    let v = t[k] as usize;
                    if new_id[v] == u32::MAX {
                        new_id[v] = positions.len() as u32;
                        let q = add(self.pos[v], self.center);
                        positions.push([q[0] as f32, q[1] as f32, q[2] as f32]);
                    }
                    o[k] = new_id[v];
                }
                triangles.push(o);
            }
            parts.push(Part {
                name: self.names[p].clone(),
                triangles: start..triangles.len() as u32,
                material: self.materials[p].clone(),
            });
        }
        let normals = crate::normals::smooth_normals(&positions, &triangles);
        let root = roots(&self.parent);
        let map = self
            .input_to_weld
            .par_iter()
            .map(|&w| if w == u32::MAX { u32::MAX } else { new_id[root[w as usize] as usize] })
            .collect();
        (Mesh { positions, normals, triangles, parts }, map)
    }
}

/// Final survivor of every vertex (path-compressed walk of `parent`).
fn roots(parent: &[u32]) -> Vec<u32> {
    let n = parent.len();
    let mut res = vec![u32::MAX; n];
    let mut chain = Vec::new();
    for v in 0..n {
        if res[v] != u32::MAX {
            continue;
        }
        chain.clear();
        let mut x = v as u32;
        let r = loop {
            if res[x as usize] != u32::MAX {
                break res[x as usize];
            }
            let p = parent[x as usize];
            if p == u32::MAX {
                break x;
            }
            chain.push(x);
            x = p;
        };
        for &y in &chain {
            res[y as usize] = r;
        }
        res[r as usize] = r;
    }
    res
}

/// Simplify `mesh` to at most `target_triangles` triangles with
/// Garland–Heckbert quadric error metrics.
///
/// Returns the simplified mesh — welded per part (no crease splits), smooth
/// angle-weighted normals, the same `parts` in the same order (each a
/// contiguous run of triangles and of vertices) — and, for every input
/// vertex, the output vertex it collapsed into (`u32::MAX` if it is not
/// used by any triangle). Carry per-vertex attributes with
/// [`carry_scalars`].
///
/// Parts never share a vertex, so stones stay separate meshes. Parts that
/// [`crate::classify_part`] calls a gem and that have at most 5,000
/// triangles are kept verbatim (cut facets are not worth a collapse), as
/// long as together they hold at most a quarter of the target. The target
/// may be missed when no legal collapse is left (locked or tiny parts).
pub fn simplify(mesh: &Mesh, target_triangles: u32) -> (Mesh, Vec<u32>) {
    let (m, map, _) = simplify_with_stats(mesh, target_triangles);
    (m, map)
}

/// [`simplify`], also returning what the simplifier did and how long it took.
pub fn simplify_with_stats(mesh: &Mesh, target_triangles: u32) -> (Mesh, Vec<u32>, SimplifyStats) {
    let table = PartTable::new(mesh, "mesh");
    let kinds = crate::material::classify_parts(&table);
    let frozen = crate::frozen_parts(&kinds, &table.counts, target_triangles as usize);
    let mut s = Simplifier::new(mesh, &table, &frozen);
    s.run(target_triangles as usize);
    let (m, map) = s.snapshot();
    (m, map, s.stats.clone())
}

/// Carry a per-vertex scalar through a [`simplify`] map: every output
/// vertex gets the mean of the input values that collapsed into it (the
/// average over the merged cluster); output vertices nothing maps to get 0.
pub fn carry_scalars(map: &[u32], values: &[f32], out_len: usize) -> Vec<f32> {
    let mut sum = vec![0f64; out_len];
    let mut cnt = vec![0u32; out_len];
    for (&m, &v) in map.iter().zip(values) {
        if (m as usize) < out_len && v.is_finite() {
            sum[m as usize] += v as f64;
            cnt[m as usize] += 1;
        }
    }
    sum.iter().zip(&cnt).map(|(s, &c)| if c > 0 { (s / c as f64) as f32 } else { 0.0 }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_quadric_finds_the_corner() {
        // Three orthogonal planes through (1, 2, 3), in a frame at the origin.
        let mut q = Quadric::default();
        for (n, d) in [([1.0, 0.0, 0.0], -1.0), ([0.0, 1.0, 0.0], -2.0), ([0.0, 0.0, 1.0], -3.0)] {
            q.add_plane(n, d, 1.0);
        }
        let c = candidates(&q, [2.0, 2.0, 2.0]);
        let y = c.y[0];
        assert!(len(sub(y, [1.0, 2.0, 3.0])) < 1e-9, "{y:?}");
        assert!(c.c[0] < 1e-12);
    }

    #[test]
    fn singular_quadric_keeps_the_midpoint() {
        // One plane z = 0: any point on it is optimal; expect the midpoint.
        let mut q = Quadric::default();
        q.add_plane([0.0, 0.0, 1.0], 0.0, 1.0);
        let c = candidates(&q, [2.0, 0.0, 0.0]);
        assert!(len(sub(c.y[0], [1.0, 0.0, 0.0])) < 1e-12, "{:?}", c.y[0]);
        // Shifted frame: same answer once translated.
        let qt = q.translated([0.0, 0.0, 0.5]);
        assert!((qt.eval([0.0; 3]) - 0.25).abs() < 1e-12);
    }

    #[test]
    fn radix_heap_pops_in_order() {
        let mut keys: Vec<u32> = (0..5000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 7) | 1).collect();
        let e = |c: u32| Entry { cost: c, a: 0, b: 0, stamp: 0 };
        let mut h = RadixHeap::with_entries(keys.iter().map(|&c| e(c)).collect());
        keys.sort_unstable();
        let mut out = Vec::new();
        while let Some(x) = h.pop() {
            out.push(x.cost);
            if out.len() == 100 {
                h.push(e(keys[300])); // later key: still in order
                h.push(e(0)); // below the floor: clamped, comes out next
            }
        }
        assert_eq!(out.len(), 5002);
        assert!(out.windows(2).all(|w| w[0] <= w[1]), "monotone");
        assert_eq!(out[100], out[99], "clamped key pops right away");
    }

    #[test]
    fn roots_follow_chains() {
        let parent = vec![u32::MAX, 0, 1, u32::MAX, 3, 2];
        assert_eq!(roots(&parent), vec![0, 0, 0, 3, 3, 0]);
    }
}
