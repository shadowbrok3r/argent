//! Closest-point BVH, used to measure how far the simplified surface is
//! from the original.

use argent_mesh::Mesh;
use rayon::prelude::*;

type V = [f32; 3];
const LEAF: usize = 4;

#[derive(Clone, Copy, Default)]
struct Node {
    lo: V,
    hi: V,
    /// Leaf: first triangle; inner: index of the left child (right = +1).
    first: u32,
    count: u32,
}

pub(crate) struct Bvh {
    nodes: Vec<Node>,
    tris: Vec<[V; 3]>,
}

#[inline]
fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn dot(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn madd(a: V, b: V, s: f32) -> V {
    [a[0] + b[0] * s, a[1] + b[1] * s, a[2] + b[2] * s]
}

/// Closest point on triangle `t` to `p` (Ericson, Real-Time Collision
/// Detection §5.1.5), returned as the squared distance.
fn tri_dist2(p: V, t: &[V; 3]) -> f32 {
    let [a, b, c] = *t;
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    let q = 'q: {
        if d1 <= 0.0 && d2 <= 0.0 {
            break 'q a;
        }
        let bp = sub(p, b);
        let d3 = dot(ab, bp);
        let d4 = dot(ac, bp);
        if d3 >= 0.0 && d4 <= d3 {
            break 'q b;
        }
        let vc = d1 * d4 - d3 * d2;
        if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
            break 'q madd(a, ab, d1 / (d1 - d3));
        }
        let cp = sub(p, c);
        let d5 = dot(ab, cp);
        let d6 = dot(ac, cp);
        if d6 >= 0.0 && d5 <= d6 {
            break 'q c;
        }
        let vb = d5 * d2 - d1 * d6;
        if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
            break 'q madd(a, ac, d2 / (d2 - d6));
        }
        let va = d3 * d6 - d5 * d4;
        if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
            break 'q madd(b, sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6)));
        }
        let denom = va + vb + vc;
        if denom == 0.0 || denom.is_nan() {
            break 'q a;
        }
        let (v, w) = (vb / denom, vc / denom);
        madd(madd(a, ab, v), ac, w)
    };
    let d = sub(p, q);
    dot(d, d)
}

#[inline]
fn box_dist2(n: &Node, p: V) -> f32 {
    p.iter()
        .zip(n.lo.iter().zip(&n.hi))
        .map(|(x, (lo, hi))| {
            let e = (lo - x).max(x - hi).max(0.0);
            e * e
        })
        .sum()
}

impl Bvh {
    pub(crate) fn new(tris: Vec<[V; 3]>) -> Bvh {
        let n = tris.len();
        if n == 0 {
            return Bvh { nodes: Vec::new(), tris };
        }
        let cent: Vec<V> = tris.iter().map(|t| [0, 1, 2].map(|k| (t[0][k] + t[1][k] + t[2][k]) / 3.0)).collect();
        let mut idx: Vec<u32> = (0..n as u32).collect();
        let mut nodes = vec![Node::default()];
        let mut stack = vec![(0usize, 0usize, n)];
        while let Some((ni, s, e)) = stack.pop() {
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            let mut clo = [f32::INFINITY; 3];
            let mut chi = [f32::NEG_INFINITY; 3];
            for &i in &idx[s..e] {
                for v in &tris[i as usize] {
                    for k in 0..3 {
                        lo[k] = lo[k].min(v[k]);
                        hi[k] = hi[k].max(v[k]);
                    }
                }
                let c = cent[i as usize];
                for k in 0..3 {
                    clo[k] = clo[k].min(c[k]);
                    chi[k] = chi[k].max(c[k]);
                }
            }
            nodes[ni].lo = lo;
            nodes[ni].hi = hi;
            let ext = [chi[0] - clo[0], chi[1] - clo[1], chi[2] - clo[2]];
            let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] {
                0
            } else if ext[1] >= ext[2] {
                1
            } else {
                2
            };
            if e - s <= LEAF || ext[axis].is_nan() || ext[axis] <= 0.0 {
                nodes[ni].first = s as u32;
                nodes[ni].count = (e - s) as u32;
                continue;
            }
            let mid = (s + e) / 2;
            idx[s..e]
                .select_nth_unstable_by(mid - s, |&i, &j| cent[i as usize][axis].total_cmp(&cent[j as usize][axis]));
            let l = nodes.len();
            nodes.push(Node::default());
            nodes.push(Node::default());
            nodes[ni].first = l as u32;
            nodes[ni].count = 0;
            stack.push((l, s, mid));
            stack.push((l + 1, mid, e));
        }
        let tris = idx.iter().map(|&i| tris[i as usize]).collect();
        Bvh { nodes, tris }
    }

    /// Squared distance from `p` to the nearest triangle (∞ when empty).
    pub(crate) fn dist2(&self, p: V) -> f32 {
        let mut best = f32::INFINITY;
        if self.nodes.is_empty() {
            return best;
        }
        let mut stack = [0u32; 96];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let n = &self.nodes[stack[sp] as usize];
            if box_dist2(n, p) >= best {
                continue;
            }
            if n.count > 0 {
                for t in &self.tris[n.first as usize..(n.first + n.count) as usize] {
                    best = best.min(tri_dist2(p, t));
                }
            } else {
                let (l, r) = (n.first, n.first + 1);
                let dl = box_dist2(&self.nodes[l as usize], p);
                let dr = box_dist2(&self.nodes[r as usize], p);
                let (near, far, dfar) = if dl <= dr { (l, r, dr) } else { (r, l, dl) };
                if dfar < best && sp < stack.len() {
                    stack[sp] = far;
                    sp += 1;
                }
                if sp < stack.len() {
                    stack[sp] = near;
                    sp += 1;
                }
            }
        }
        best
    }
}

#[inline]
fn hash32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

#[inline]
fn unit(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / (1u32 << 24) as f32)
}

/// One-sided deviation of `simp` from `orig`, part by part: every original
/// vertex plus one random point per original triangle, each measured to
/// the nearest simplified triangle of the same part. Returns (max, mean) in
/// the meshes' units.
pub(crate) fn deviation(orig: &Mesh, orig_part: &[u32], simp: &Mesh) -> (f32, f32) {
    let bvhs: Vec<Bvh> = simp
        .parts
        .par_iter()
        .map(|p| {
            let r = p.triangles.start as usize..(p.triangles.end as usize).min(simp.triangles.len());
            Bvh::new(simp.triangles[r].iter().map(|t| t.map(|i| simp.positions[i as usize])).collect())
        })
        .collect();
    let mut vpart = vec![u32::MAX; orig.positions.len()];
    for (t, &p) in orig.triangles.iter().zip(orig_part) {
        for &v in t {
            if let Some(slot) = vpart.get_mut(v as usize)
                && *slot == u32::MAX
            {
                *slot = p;
            }
        }
    }
    let query = |p: V, part: u32| -> Option<f64> {
        let d = bvhs.get(part as usize)?.dist2(p);
        d.is_finite().then(|| (d as f64).sqrt())
    };
    let fold = |(m, s, n): (f64, f64, u64), d: Option<f64>| match d {
        Some(d) => (m.max(d), s + d, n + 1),
        None => (m, s, n),
    };
    let merge = |a: (f64, f64, u64), b: (f64, f64, u64)| (a.0.max(b.0), a.1 + b.1, a.2 + b.2);
    let verts = orig
        .positions
        .par_iter()
        .zip(vpart.par_iter())
        .with_min_len(4096)
        .map(|(p, &part)| if part == u32::MAX { None } else { query(*p, part) })
        .fold(|| (0.0, 0.0, 0u64), fold)
        .reduce(|| (0.0, 0.0, 0), merge);
    let faces = orig
        .triangles
        .par_iter()
        .zip(orig_part.par_iter())
        .enumerate()
        .with_min_len(4096)
        .map(|(f, (t, &part))| {
            let h = hash32(f as u32 ^ 0x9e37_79b9);
            let (mut u, mut v) = (unit(h), unit(hash32(h)));
            if u + v > 1.0 {
                (u, v) = (1.0 - u, 1.0 - v);
            }
            let [a, b, c] = t.map(|i| orig.positions.get(i as usize).copied().unwrap_or([f32::NAN; 3]));
            let p = [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * u + (c[k] - a[k]) * v);
            if p.iter().all(|x| x.is_finite()) { query(p, part) } else { None }
        })
        .fold(|| (0.0, 0.0, 0u64), fold)
        .reduce(|| (0.0, 0.0, 0), merge);
    let (m, s, n) = merge(verts, faces);
    (m as f32, if n > 0 { (s / n as f64) as f32 } else { 0.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_triangle() {
        let tris = vec![
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [[5.0, 5.0, 1.0], [6.0, 5.0, 1.0], [5.0, 6.0, 1.0]],
        ];
        let b = Bvh::new(tris);
        assert!((b.dist2([0.25, 0.25, 2.0]) - 4.0).abs() < 1e-6);
        assert!((b.dist2([5.2, 5.2, 0.0]) - 1.0).abs() < 1e-6);
        assert!((b.dist2([-1.0, 0.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!((b.dist2([1.0, 1.0, 0.0]) - 0.5).abs() < 1e-6);
    }
}
