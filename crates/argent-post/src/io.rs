//! Image files: 8-bit PNG/JPEG, 16-bit PNG (developed), OpenEXR (linear).

use crate::tonemap::{Grade, develop_f32};
use crate::{Image, Rgb8};
use std::path::Path;

fn ensure_parent(path: &Path) -> anyhow::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}

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

/// 16-bit sRGB-encoded PNG of `img` developed with `grade` (exact tone
/// mapping, no dither needed at 16 bits).
pub fn write_png16(path: &Path, img: &Image, grade: &Grade) -> anyhow::Result<()> {
    ensure_parent(path)?;
    let data: Vec<u16> =
        develop_f32(img, grade).iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16)).collect();
    let buf = image::ImageBuffer::<image::Rgb<u16>, _>::from_raw(img.width, img.height, data)
        .ok_or_else(|| anyhow::anyhow!("image buffer size mismatch for {}x{}", img.width, img.height))?;
    buf.save_with_format(path, image::ImageFormat::Png)?;
    Ok(())
}

fn rec709_chromaticities() -> exr::meta::attribute::Chromaticities {
    use exr::math::Vec2;
    exr::meta::attribute::Chromaticities {
        red: Vec2(0.64, 0.33),
        green: Vec2(0.30, 0.60),
        blue: Vec2(0.15, 0.06),
        white: Vec2(0.3127, 0.3290),
    }
}

fn write_exr_impl<S>(path: &Path, img: &Image, conv: impl Fn(f32) -> S + Sync) -> anyhow::Result<()>
where
    S: exr::prelude::IntoSample,
{
    use exr::prelude::*;
    ensure_parent(path)?;
    anyhow::ensure!(img.pixels.len() == img.width as usize * img.height as usize, "pixel count mismatch");
    let w = img.width as usize;
    let mut attrs = LayerAttributes::named("");
    attrs.software_name = Some(Text::from("argent"));
    let layer = Layer::new(
        (w, img.height as usize),
        attrs,
        Encoding::SMALL_LOSSLESS,
        SpecificChannels::rgb(|pos: Vec2<usize>| {
            let p = img.pixels[pos.y() * w + pos.x()];
            (conv(p[0]), conv(p[1]), conv(p[2]))
        }),
    );
    let mut image = exr::prelude::Image::from_layer(layer);
    image.attributes.chromaticities = Some(rec709_chromaticities());
    image.write().to_file(path).map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))?;
    Ok(())
}

/// Scene-linear Rec.709 OpenEXR, 32-bit float RGB, ZIP16 (lossless),
/// tagged with Rec.709/D65 chromaticities.
pub fn write_exr(path: &Path, img: &Image) -> anyhow::Result<()> {
    write_exr_impl(path, img, |v| v)
}

/// As [`write_exr`] but 16-bit half floats (half the size; ±65504 range).
pub fn write_exr_half(path: &Path, img: &Image) -> anyhow::Result<()> {
    write_exr_impl(path, img, exr::prelude::f16::from_f32)
}

/// Reads the first RGB(A) layer of an OpenEXR file (alpha dropped).
pub fn read_exr(path: &Path) -> anyhow::Result<Image> {
    let img = exr::prelude::read_first_rgba_layer_from_file(
        path,
        |res, _| Image::new(res.width() as u32, res.height() as u32),
        |img: &mut Image, pos, (r, g, b, _a): (f32, f32, f32, f32)| img.set(pos.x() as u32, pos.y() as u32, [r, g, b]),
    )
    .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
    Ok(img.layer_data.channel_data.pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tonemap::{AgxLook, ToneMap};

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("argent-post-tests/io");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn hdr_image() -> Image {
        let mut img = Image::new(37, 23);
        for y in 0..23 {
            for x in 0..37 {
                img.set(x, y, [x as f32 * 0.5, y as f32 * 0.01, 1e-4 * (x * y) as f32 + 100.0 * (x == 3) as u32 as f32]);
            }
        }
        img
    }

    #[test]
    fn exr_round_trip() {
        let img = hdr_image();
        let p = scratch("rt.exr");
        write_exr(&p, &img).unwrap();
        let back = read_exr(&p).unwrap();
        assert_eq!((back.width, back.height), (img.width, img.height));
        assert_eq!(back.pixels, img.pixels, "f32 EXR is lossless");
        let ph = scratch("rt-half.exr");
        write_exr_half(&ph, &img).unwrap();
        let half = read_exr(&ph).unwrap();
        for (a, b) in half.pixels.iter().zip(&img.pixels) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() <= b[c].abs() * 1e-3 + 1e-6, "{a:?} vs {b:?}");
            }
        }
        assert!(std::fs::metadata(&ph).unwrap().len() < std::fs::metadata(&p).unwrap().len());
    }

    #[test]
    fn png16_round_trip() {
        let img = hdr_image();
        let grade = Grade { tonemap: ToneMap::AgX(AgxLook::Base), ..Grade::default() };
        let p = scratch("t16.png");
        write_png16(&p, &img, &grade).unwrap();
        let back = image::open(&p).unwrap();
        assert_eq!(back.color(), image::ColorType::Rgb16);
        let back = back.to_rgb16();
        let want = develop_f32(&img, &grade);
        for (px, w) in back.pixels().zip(&want) {
            for (got, exp) in px.0.iter().zip(w) {
                assert!((*got as f32 / 65535.0 - exp).abs() < 1.0 / 65535.0 + 1e-6);
            }
        }
    }
}
