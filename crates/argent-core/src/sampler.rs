//! Low-discrepancy sampling: Owen-scrambled Sobol (Burley 2020) per pixel,
//! with a hashed fallback past the table's dimensions.

/// One camera sample's stream of dimensions.
pub struct Sampler {
    index: u32,
    seed: u32,
    dim: u32,
    rng: u64,
    /// Cached 4-dimension block (Sobol is evaluated four dims at a time).
    block: [f32; 4],
    block_set: u32,
}

const MAX_SOBOL_DIM: u32 = sobol_burley::NUM_DIMENSIONS;

#[inline]
pub fn hash32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

impl Sampler {
    /// `pixel_seed` should be unique per pixel (and per frame for animation).
    #[inline]
    pub fn new(pixel_seed: u32, sample_index: u32) -> Self {
        Sampler {
            index: sample_index,
            seed: pixel_seed,
            dim: 0,
            rng: ((pixel_seed as u64) << 32 | sample_index as u64) ^ 0x9e37_79b9_7f4a_7c15,
            block: [0.0; 4],
            block_set: u32::MAX,
        }
    }

    #[inline]
    pub fn next(&mut self) -> f32 {
        let d = self.dim;
        self.dim += 1;
        if d < MAX_SOBOL_DIM && self.index < (1 << 16) {
            let set = d / 4;
            if set != self.block_set {
                self.block = sobol_burley::sample_4d(self.index, set, self.seed);
                self.block_set = set;
            }
            self.block[(d % 4) as usize].min(0.999_999_94)
        } else {
            self.next_random()
        }
    }

    #[inline]
    pub fn next2(&mut self) -> (f32, f32) {
        (self.next(), self.next())
    }

    /// Plain PCG-ish random, for decisions that don't benefit from stratification.
    #[inline]
    pub fn next_random(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        let x = ((self.rng >> 18) ^ self.rng) >> 27;
        let rot = (self.rng >> 59) as u32;
        let v = (x as u32).rotate_right(rot);
        (v >> 8) as f32 * (1.0 / (1u32 << 24) as f32)
    }

    /// Skip to a fixed dimension block so every bounce uses the same dims.
    #[inline]
    pub fn set_dim(&mut self, d: u32) {
        self.dim = d;
    }
}

/// Tent-filter offset in `[-r, r]` (importance-sampled), from `u` in `[0,1)`.
#[inline]
pub fn tent(u: f32, r: f32) -> f32 {
    let u = 2.0 * u;
    if u < 1.0 { r * ((u).sqrt() - 1.0) } else { r * (1.0 - (2.0 - u).sqrt()) }
}

/// Concentric disk sample (Shirley–Chiu).
#[inline]
pub fn disk(u: f32, v: f32) -> (f32, f32) {
    let a = 2.0 * u - 1.0;
    let b = 2.0 * v - 1.0;
    if a == 0.0 && b == 0.0 {
        return (0.0, 0.0);
    }
    let (r, phi) = if a.abs() > b.abs() {
        (a, std::f32::consts::FRAC_PI_4 * (b / a))
    } else {
        (b, std::f32::consts::FRAC_PI_2 - std::f32::consts::FRAC_PI_4 * (a / b))
    };
    (r * phi.cos(), r * phi.sin())
}

/// Cosine-weighted hemisphere (+Z).
#[inline]
pub fn cosine_hemisphere(u: f32, v: f32) -> glam::Vec3A {
    let (x, y) = disk(u, v);
    let z = (1.0 - x * x - y * y).max(0.0).sqrt();
    glam::Vec3A::new(x, y, z)
}
