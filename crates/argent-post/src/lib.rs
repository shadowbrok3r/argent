//! Everything between the renderer's linear radiance and a file on disk.
//!
//! CONTRACT (other crates compile against these signatures; keep them):
//! - [`Image`] is scene-referred linear Rec.709 (sRGB primaries), f32.
//! - [`denoise`] never fails hard when OIDN is missing: `Auto` falls back.
//! - [`develop`] = exposure → tone map → sRGB OETF → dithered 8-bit.
//! - [`video::VideoWriter`] streams frames to ffmpeg (h264 yuv420p + AAC).
//! - [`qc::forge_reel`] reproduces Forge's reel checks.

pub mod denoise;
pub mod io;
pub mod qc;
pub mod sheet;
pub mod tonemap;
pub mod video;

pub use denoise::{denoise, oidn_available, DenoiseBackend, DenoiseInput};
pub use tonemap::{develop, AgxLook, Grade, ToneMap};

/// Linear HDR image, row-major from the top-left.
#[derive(Clone, Debug, Default)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 3]>,
}

impl Image {
    pub fn new(width: u32, height: u32) -> Self {
        Image { width, height, pixels: vec![[0.0; 3]; (width * height) as usize] }
    }
    #[inline]
    pub fn get(&self, x: u32, y: u32) -> [f32; 3] {
        self.pixels[(y * self.width + x) as usize]
    }
    #[inline]
    pub fn set(&mut self, x: u32, y: u32, v: [f32; 3]) {
        let w = self.width;
        self.pixels[(y * w + x) as usize] = v;
    }
}

/// Display-referred 8-bit sRGB image, RGB interleaved, row-major.
#[derive(Clone, Debug, Default)]
pub struct Rgb8 {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Rgb8 {
    pub fn new(width: u32, height: u32) -> Self {
        Rgb8 { width, height, data: vec![0; (width * height * 3) as usize] }
    }
}
