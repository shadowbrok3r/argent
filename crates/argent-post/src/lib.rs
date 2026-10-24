//! Everything between the renderer's linear radiance and a file on disk.
//!
//! CONTRACT (other crates compile against these signatures; keep them):
//! - [`Image`] is scene-referred linear Rec.709 (sRGB primaries), f32.
//! - [`denoise()`] never fails hard when OIDN is missing: `Auto` falls back.
//!   [`denoise_with`] adds OIDN quality / aux-prefilter options.
//! - [`develop`] = exposure → tone map → sRGB OETF → dithered 8-bit.
//! - [`video::VideoWriter`] streams frames to ffmpeg (h264 yuv420p BT.709 + AAC).
//! - [`qc::forge_reel`] reproduces Forge's reel checks ([`qc::forge_reel_full`]
//!   adds Forge's frozen-frame, loudness and true-peak checks).
//! - [`sheet::overlay_hook`] burns in reel hooks, wrapping only at spaces.
//!
//! Runtime needs: `ffmpeg`/`ffprobe` on `PATH` (or `ARGENT_FFMPEG` /
//! `ARGENT_FFPROBE`) for video and QC; OIDN 2.x optional (`ARGENT_OIDN_DIR`
//! or `~/.local/opt/oidn-*/`). The EB Garamond font is embedded (SIL OFL 1.1,
//! `assets/fonts/FONT-LICENSE.txt`).

mod color;
pub mod denoise;
pub mod io;
pub mod qc;
pub mod sheet;
mod text;
pub mod tonemap;
pub mod video;

pub use color::{srgb_decode, srgb_encode};
pub use denoise::{
    denoise, denoise_with, oidn_available, oidn_version, DenoiseBackend, DenoiseInput, DenoiseOptions, DenoiseQuality,
};
pub use qc::{forge_reel, forge_reel_full, QcReport};
pub use sheet::{contact_sheet, draw_text, overlay_hook, Align, HookStyle, SheetStyle, Tile};
pub use tonemap::{develop, develop_f32, tonemap_pixel, AgxLook, Grade, ToneMap};
pub use video::{encode_frames, Audio, VideoSpec, VideoWriter};

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
