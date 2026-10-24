//! Bounding volume hierarchy over triangles: binned-SAH build (parallel at
//! the top), flattened depth-first, ordered stack traversal.
//!
//! Jewelry meshes are dense (a ZBrush skull ring is 1–5M triangles) but the
//! scene is one static piece, so everything lives in a single BVH and the
//! turntable moves the studio instead of the object.

use crate::geom::{Ray, V3};
use rayon::prelude::*;

#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub min: V3,
    pub max: V3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb { min: V3::splat(f32::INFINITY), max: V3::splat(f32::NEG_INFINITY) };

    #[inline]
    pub fn grow(&mut self, p: V3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    #[inline]
    pub fn union(&self, o: &Aabb) -> Aabb {
        Aabb { min: self.min.min(o.min), max: self.max.max(o.max) }
    }
    #[inline]
    pub fn area(&self) -> f32 {
        let e = (self.max - self.min).max(V3::ZERO);
        2.0 * (e.x * e.y + e.y * e.z + e.z * e.x)
    }
    #[inline]
    pub fn centroid(&self) -> V3 {
        (self.min + self.max) * 0.5
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }
}

/// 32-byte node. Interior: `count == 0`, children at `index` and `index+1`.
/// Leaf: triangles `index..index+count` in [`Bvh::tris`] order.
#[derive(Clone, Copy, Debug)]
#[repr(C, align(32))]
pub struct Node {
    min: [f32; 3],
    index: u32,
    max: [f32; 3],
    count: u32,
}

/// Precomputed triangle for Möller–Trumbore.
#[derive(Clone, Copy, Debug)]
pub struct Tri {
    pub v0: V3,
    pub e1: V3,
    pub e2: V3,
}

#[derive(Clone, Copy, Debug)]
pub struct TriHit {
    pub t: f32,
    pub u: f32,
    pub v: f32,
    /// Index into the original triangle list.
    pub prim: u32,
}

#[derive(Clone)]
pub struct Bvh {
    nodes: Vec<Node>,
    /// The binary tree collapsed to 4-wide nodes for SIMD traversal.
    wide: Vec<Node4>,
    tris: Vec<Tri>,
    /// BVH order → original triangle index.
    prim_ids: Vec<u32>,
}

/// Four children's boxes in structure-of-arrays form, tested at once.
/// `child[i]`: interior → index into `wide`; leaf → `LEAF | first_tri`
/// with the count in `count[i]`; empty slot → `EMPTY`.
#[derive(Clone, Copy, Debug)]
#[repr(C, align(64))]
struct Node4 {
    min_x: glam::Vec4,
    min_y: glam::Vec4,
    min_z: glam::Vec4,
    max_x: glam::Vec4,
    max_y: glam::Vec4,
    max_z: glam::Vec4,
    child: [u32; 4],
    count: [u32; 4],
}

const LEAF: u32 = 1 << 31;
const EMPTY: u32 = u32::MAX;

const BINS: usize = 16;
const MAX_LEAF: usize = 4;

struct BuildPrim {
    bounds: Aabb,
    centroid: V3,
    id: u32,
}

/// A built subtree: nodes with child indices relative to the subtree root.
struct Sub {
    nodes: Vec<Node>,
    prims: Vec<u32>,
}

impl Bvh {
    pub fn build(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Bvh {
        let p = |i: u32| V3::from(positions[i as usize]);
        let mut prims: Vec<BuildPrim> = triangles
            .par_iter()
            .enumerate()
            .map(|(i, t)| {
                let mut b = Aabb::EMPTY;
                b.grow(p(t[0]));
                b.grow(p(t[1]));
                b.grow(p(t[2]));
                BuildPrim { centroid: b.centroid(), bounds: b, id: i as u32 }
            })
            .collect();
        let sub = if prims.is_empty() {
            Sub { nodes: vec![Node { min: [0.0; 3], index: 0, max: [0.0; 3], count: 0 }], prims: vec![] }
        } else {
            build_rec(&mut prims)
        };
        let tris = sub
            .prims
            .par_iter()
            .map(|&id| {
                let t = triangles[id as usize];
                let v0 = p(t[0]);
                Tri { v0, e1: p(t[1]) - v0, e2: p(t[2]) - v0 }
            })
            .collect();
        let wide = collapse(&sub.nodes);
        Bvh { nodes: sub.nodes, wide, tris, prim_ids: sub.prims }
    }

    pub fn bounds(&self) -> Aabb {
        let n = &self.nodes[0];
        Aabb { min: V3::from(n.min), max: V3::from(n.max) }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Closest hit with `t` in `(0, t_max)`.
    #[inline]
    pub fn intersect(&self, ray: &Ray, t_max: f32) -> Option<TriHit> {
        if self.tris.is_empty() {
            return None;
        }
        let inv = V3::ONE / ray.d;
        let (ox, oy, oz) = (glam::Vec4::splat(ray.o.x), glam::Vec4::splat(ray.o.y), glam::Vec4::splat(ray.o.z));
        let (ix, iy, iz) = (glam::Vec4::splat(inv.x), glam::Vec4::splat(inv.y), glam::Vec4::splat(inv.z));
        let mut best_t = t_max;
        let mut best: Option<(u32, f32, f32)> = None;
        // (entry, t_near) pairs; entries are wide-node indices or LEAF-tagged ranges.
        let mut stack: [(u32, u32, f32); 192] = [(0, 0, 0.0); 192];
        let mut sp = 0usize;
        stack[sp] = (0, 0, 0.0);
        sp += 1;
        while sp > 0 {
            sp -= 1;
            let (entry, count, tn) = stack[sp];
            if tn > best_t {
                continue;
            }
            if entry & LEAF != 0 {
                let start = (entry & !LEAF) as usize;
                for i in start..start + count as usize {
                    if let Some((t, u, v)) = tri_intersect(&self.tris[i], ray, best_t) {
                        best_t = t;
                        best = Some((i as u32, u, v));
                    }
                }
                continue;
            }
            let n = &self.wide[entry as usize];
            let (tmin, mask) = slab4(n, ox, oy, oz, ix, iy, iz, best_t);
            // Push hit children far-to-near so the nearest pops first.
            let mut order = [(0usize, 0f32); 4];
            let mut k = 0;
            for c in 0..4 {
                if mask & (1 << c) != 0 {
                    let t = tmin[c];
                    let mut j = k;
                    while j > 0 && order[j - 1].1 < t {
                        order[j] = order[j - 1];
                        j -= 1;
                    }
                    order[j] = (c, t);
                    k += 1;
                }
            }
            for &(c, t) in &order[..k] {
                stack[sp] = (n.child[c], n.count[c], t);
                sp += 1;
            }
        }
        best.map(|(i, u, v)| TriHit { t: best_t, u, v, prim: self.prim_ids[i as usize] })
    }

    /// Any hit with `t` in `(0, t_max)`.
    #[inline]
    pub fn occluded(&self, ray: &Ray, t_max: f32) -> bool {
        if self.tris.is_empty() {
            return false;
        }
        let inv = V3::ONE / ray.d;
        let (ox, oy, oz) = (glam::Vec4::splat(ray.o.x), glam::Vec4::splat(ray.o.y), glam::Vec4::splat(ray.o.z));
        let (ix, iy, iz) = (glam::Vec4::splat(inv.x), glam::Vec4::splat(inv.y), glam::Vec4::splat(inv.z));
        let mut stack: [(u32, u32); 192] = [(0, 0); 192];
        let mut sp = 1usize;
        while sp > 0 {
            sp -= 1;
            let (entry, count) = stack[sp];
            if entry & LEAF != 0 {
                let start = (entry & !LEAF) as usize;
                for i in start..start + count as usize {
                    if tri_intersect(&self.tris[i], ray, t_max).is_some() {
                        return true;
                    }
                }
                continue;
            }
            let n = &self.wide[entry as usize];
            let (_, mask) = slab4(n, ox, oy, oz, ix, iy, iz, t_max);
            for c in 0..4 {
                if mask & (1 << c) != 0 {
                    stack[sp] = (n.child[c], n.count[c]);
                    sp += 1;
                }
            }
        }
        false
    }

    /// Reference binary traversal (kept for tests).
    #[cfg(test)]
    fn intersect_binary(&self, ray: &Ray, t_max: f32) -> Option<TriHit> {
        let inv = V3::ONE / ray.d;
        let mut best_t = t_max;
        let mut best: Option<(u32, f32, f32)> = None;
        let mut stack = vec![0u32];
        while let Some(idx) = stack.pop() {
            let node = &self.nodes[idx as usize];
            if slab(node, ray, inv, best_t).is_none() {
                continue;
            }
            if node.count > 0 {
                let start = node.index as usize;
                for i in start..start + node.count as usize {
                    if let Some((t, u, v)) = tri_intersect(&self.tris[i], ray, best_t) {
                        best_t = t;
                        best = Some((i as u32, u, v));
                    }
                }
            } else {
                stack.push(node.index);
                stack.push(node.index + 1);
            }
        }
        best.map(|(i, u, v)| TriHit { t: best_t, u, v, prim: self.prim_ids[i as usize] })
    }
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn slab4(
    n: &Node4,
    ox: glam::Vec4,
    oy: glam::Vec4,
    oz: glam::Vec4,
    ix: glam::Vec4,
    iy: glam::Vec4,
    iz: glam::Vec4,
    t_max: f32,
) -> (glam::Vec4, u32) {
    let tx0 = (n.min_x - ox) * ix;
    let tx1 = (n.max_x - ox) * ix;
    let ty0 = (n.min_y - oy) * iy;
    let ty1 = (n.max_y - oy) * iy;
    let tz0 = (n.min_z - oz) * iz;
    let tz1 = (n.max_z - oz) * iz;
    let tmin = tx0.min(tx1).max(ty0.min(ty1)).max(tz0.min(tz1)).max(glam::Vec4::ZERO);
    let tmax = tx0.max(tx1).min(ty0.max(ty1)).min(tz0.max(tz1)).min(glam::Vec4::splat(t_max));
    (tmin, tmin.cmple(tmax).bitmask())
}

/// Collapse a binary BVH into 4-wide nodes: each wide node takes up to four
/// descendants by repeatedly opening its largest interior child.
fn collapse(nodes: &[Node]) -> Vec<Node4> {
    let mut wide: Vec<Node4> = Vec::with_capacity(nodes.len() / 2 + 1);
    fn area(n: &Node) -> f32 {
        let e = [n.max[0] - n.min[0], n.max[1] - n.min[1], n.max[2] - n.min[2]];
        e[0] * e[1] + e[1] * e[2] + e[2] * e[0]
    }
    // Root: if it's a leaf, wrap it as a single child.
    fn build(nodes: &[Node], root: u32, wide: &mut Vec<Node4>) -> u32 {
        let me = wide.len() as u32;
        wide.push(Node4 {
            min_x: glam::Vec4::splat(f32::INFINITY),
            min_y: glam::Vec4::splat(f32::INFINITY),
            min_z: glam::Vec4::splat(f32::INFINITY),
            max_x: glam::Vec4::splat(f32::NEG_INFINITY),
            max_y: glam::Vec4::splat(f32::NEG_INFINITY),
            max_z: glam::Vec4::splat(f32::NEG_INFINITY),
            child: [EMPTY; 4],
            count: [0; 4],
        });
        let r = &nodes[root as usize];
        let mut kids: Vec<u32> = if r.count > 0 { vec![root] } else { vec![r.index, r.index + 1] };
        while kids.len() < 4 {
            // Open the interior child with the largest surface area.
            let mut best: Option<(usize, f32)> = None;
            for (i, &k) in kids.iter().enumerate() {
                let n = &nodes[k as usize];
                if n.count == 0 {
                    let a = area(n);
                    if best.is_none_or(|b| a > b.1) {
                        best = Some((i, a));
                    }
                }
            }
            let Some((i, _)) = best else { break };
            let n = nodes[kids[i] as usize];
            kids.swap_remove(i);
            kids.push(n.index);
            kids.push(n.index + 1);
        }
        let mut node = wide[me as usize];
        let mut mins = [[f32::INFINITY; 4]; 3];
        let mut maxs = [[f32::NEG_INFINITY; 4]; 3];
        for (c, &k) in kids.iter().enumerate() {
            let n = nodes[k as usize];
            for a in 0..3 {
                mins[a][c] = n.min[a];
                maxs[a][c] = n.max[a];
            }
            if n.count > 0 {
                node.child[c] = LEAF | n.index;
                node.count[c] = n.count;
            } else {
                node.child[c] = u32::MAX - 1; // placeholder, patched below
            }
        }
        node.min_x = glam::Vec4::from_array(mins[0]);
        node.min_y = glam::Vec4::from_array(mins[1]);
        node.min_z = glam::Vec4::from_array(mins[2]);
        node.max_x = glam::Vec4::from_array(maxs[0]);
        node.max_y = glam::Vec4::from_array(maxs[1]);
        node.max_z = glam::Vec4::from_array(maxs[2]);
        wide[me as usize] = node;
        for (c, &k) in kids.iter().enumerate() {
            if nodes[k as usize].count == 0 {
                let w = build(nodes, k, wide);
                wide[me as usize].child[c] = w;
            }
        }
        me
    }
    if nodes.is_empty() {
        return wide;
    }
    build(nodes, 0, &mut wide);
    wide
}

#[inline(always)]
fn slab(n: &Node, ray: &Ray, inv: V3, t_max: f32) -> Option<f32> {
    let t0 = (V3::from(n.min) - ray.o) * inv;
    let t1 = (V3::from(n.max) - ray.o) * inv;
    let tmin = t0.min(t1).max_element().max(0.0);
    let tmax = t0.max(t1).min_element().min(t_max);
    if tmin <= tmax { Some(tmin) } else { None }
}

#[inline(always)]
fn tri_intersect(tri: &Tri, ray: &Ray, t_max: f32) -> Option<(f32, f32, f32)> {
    let p = ray.d.cross(tri.e2);
    let det = tri.e1.dot(p);
    if det.abs() < 1e-14 {
        return None;
    }
    let inv_det = 1.0 / det;
    let s = ray.o - tri.v0;
    let u = s.dot(p) * inv_det;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(tri.e1);
    let v = ray.d.dot(q) * inv_det;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = tri.e2.dot(q) * inv_det;
    if t > 1e-7 && t < t_max { Some((t, u, v)) } else { None }
}

fn leaf(prims: &[BuildPrim], bounds: Aabb) -> Sub {
    Sub {
        nodes: vec![Node { min: bounds.min.into(), index: 0, max: bounds.max.into(), count: prims.len() as u32 }],
        prims: prims.iter().map(|p| p.id).collect(),
    }
}

fn build_rec(prims: &mut [BuildPrim]) -> Sub {
    let mut bounds = Aabb::EMPTY;
    let mut cb = Aabb::EMPTY;
    for p in prims.iter() {
        bounds = bounds.union(&p.bounds);
        cb.grow(p.centroid);
    }
    let n = prims.len();
    if n <= MAX_LEAF {
        return leaf(prims, bounds);
    }
    let ext = cb.max - cb.min;
    let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
    let lo = cb.min[axis];
    let span = ext[axis];
    if span <= 1e-12 {
        // All centroids coincide: split in the middle of the list.
        let mid = n / 2;
        return combine(bounds, prims, mid);
    }
    // Binned SAH.
    let mut bin_b = [Aabb::EMPTY; BINS];
    let mut bin_n = [0usize; BINS];
    let k = BINS as f32 / span;
    let bin_of = |c: f32| (((c - lo) * k) as usize).min(BINS - 1);
    for p in prims.iter() {
        let b = bin_of(p.centroid[axis]);
        bin_n[b] += 1;
        bin_b[b] = bin_b[b].union(&p.bounds);
    }
    let mut left_area = [0f32; BINS - 1];
    let mut left_n = [0usize; BINS - 1];
    let mut acc = Aabb::EMPTY;
    let mut cnt = 0;
    for i in 0..BINS - 1 {
        acc = acc.union(&bin_b[i]);
        cnt += bin_n[i];
        left_area[i] = if cnt > 0 { acc.area() } else { 0.0 };
        left_n[i] = cnt;
    }
    let mut best = (f32::INFINITY, 0usize);
    let mut acc = Aabb::EMPTY;
    let mut cnt = 0;
    for i in (1..BINS).rev() {
        acc = acc.union(&bin_b[i]);
        cnt += bin_n[i];
        let ln = left_n[i - 1];
        if ln == 0 || cnt == 0 {
            continue;
        }
        let cost = left_area[i - 1] * ln as f32 + acc.area() * cnt as f32;
        if cost < best.0 {
            best = (cost, i);
        }
    }
    let leaf_cost = bounds.area() * n as f32;
    if !best.0.is_finite() || (n <= 16 && best.0 >= leaf_cost) {
        if n <= 16 {
            return leaf(prims, bounds);
        }
        return combine(bounds, prims, n / 2);
    }
    let split_bin = best.1;
    // Partition in place.
    let mut i = 0;
    let mut j = n;
    while i < j {
        if bin_of(prims[i].centroid[axis]) < split_bin {
            i += 1;
        } else {
            j -= 1;
            prims.swap(i, j);
        }
    }
    let mid = if i == 0 || i == n { n / 2 } else { i };
    combine(bounds, prims, mid)
}

fn combine(bounds: Aabb, prims: &mut [BuildPrim], mid: usize) -> Sub {
    let n = prims.len();
    let (l, r) = prims.split_at_mut(mid);
    let (ls, rs) = if n > 8192 { rayon::join(|| build_rec(l), || build_rec(r)) } else { (build_rec(l), build_rec(r)) };
    // Layout: root, left child, right child, left subtree rest, right subtree rest.
    let mut nodes = Vec::with_capacity(1 + ls.nodes.len() + rs.nodes.len());
    nodes.push(Node { min: bounds.min.into(), index: 1, max: bounds.max.into(), count: 0 });
    let l_root = 1u32;
    let r_root = 2u32;
    // Place both subtree roots adjacently, then the remaining nodes.
    let l_rest_base = 3u32;
    let r_rest_base = l_rest_base + (ls.nodes.len() as u32 - 1);
    let remap = |node: &Node, base_rest: u32, prim_off: u32| -> Node {
        let mut nd = *node;
        if nd.count > 0 {
            nd.index += prim_off;
        } else {
            // child index i (>=1 in subtree numbering) → rest base + (i - 1)
            nd.index = base_rest + (nd.index - 1);
        }
        nd
    };
    let lp = 0u32;
    let rp = ls.prims.len() as u32;
    nodes.push(remap(&ls.nodes[0], l_rest_base, lp));
    nodes.push(remap(&rs.nodes[0], r_rest_base, rp));
    for nd in &ls.nodes[1..] {
        nodes.push(remap(nd, l_rest_base, lp));
    }
    for nd in &rs.nodes[1..] {
        nodes.push(remap(nd, r_rest_base, rp));
    }
    let _ = (l_root, r_root);
    let mut prims_out = ls.prims;
    prims_out.extend(rs.prims);
    Sub { nodes, prims: prims_out }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(n: usize) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let mut p = vec![];
        let mut t = vec![];
        for j in 0..=n {
            for i in 0..=n {
                let x = i as f32 / n as f32 * 10.0 - 5.0;
                let z = j as f32 / n as f32 * 10.0 - 5.0;
                p.push([x, (x * 0.7).sin() * (z * 0.5).cos(), z]);
            }
        }
        let w = (n + 1) as u32;
        for j in 0..n as u32 {
            for i in 0..n as u32 {
                let a = j * w + i;
                t.push([a, a + 1, a + w]);
                t.push([a + 1, a + w + 1, a + w]);
            }
        }
        (p, t)
    }

    fn brute(p: &[[f32; 3]], t: &[[u32; 3]], ray: &Ray) -> Option<f32> {
        let mut best = f32::INFINITY;
        for tri in t {
            let v0 = V3::from(p[tri[0] as usize]);
            let tr = Tri { v0, e1: V3::from(p[tri[1] as usize]) - v0, e2: V3::from(p[tri[2] as usize]) - v0 };
            if let Some((tt, _, _)) = tri_intersect(&tr, ray, best) {
                best = tt;
            }
        }
        best.is_finite().then_some(best)
    }

    #[test]
    fn bvh_matches_brute_force() {
        let (p, t) = grid(60);
        let bvh = Bvh::build(&p, &t);
        let mut s = 12345u32;
        let mut rnd = || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as f32 / u32::MAX as f32
        };
        for _ in 0..2000 {
            let o = V3::new(rnd() * 12.0 - 6.0, 4.0 + rnd() * 2.0, rnd() * 12.0 - 6.0);
            let d = V3::new(rnd() - 0.5, -1.0, rnd() - 0.5).normalize();
            let ray = Ray::new(o, d);
            let a = bvh.intersect(&ray, f32::INFINITY).map(|h| h.t);
            let a2 = bvh.intersect_binary(&ray, f32::INFINITY).map(|h| h.t);
            assert_eq!(a.is_some(), a2.is_some());
            let b = brute(&p, &t, &ray);
            match (a, b) {
                (Some(x), Some(y)) => assert!((x - y).abs() < 1e-4, "{x} vs {y}"),
                (None, None) => {}
                other => panic!("mismatch {other:?}"),
            }
            assert_eq!(bvh.occluded(&ray, f32::INFINITY), b.is_some());
        }
    }
}
