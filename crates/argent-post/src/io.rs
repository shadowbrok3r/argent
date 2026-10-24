//! Image files. TODO(agent): EXR (linear f32/half) writer; 16-bit PNG.

use crate::Rgb8;
use std::path::Path;

pub fn write_png(path: &Path, img: &Rgb8) -> anyhow::Result<()> {
    image::save_buffer(path, &img.data, img.width, img.height, image::ExtendedColorType::Rgb8)?;
    Ok(())
}

pub fn write_jpeg(path: &Path, img: &Rgb8, quality: u8) -> anyhow::Result<()> {
    let f = std::fs::File::create(path)?;
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(f), quality);
    enc.encode(&img.data, img.width, img.height, image::ExtendedColorType::Rgb8)?;
    Ok(())
}

pub fn read_rgb8(path: &Path) -> anyhow::Result<Rgb8> {
    let im = image::open(path)?.to_rgb8();
    Ok(Rgb8 { width: im.width(), height: im.height(), data: im.into_raw() })
}
