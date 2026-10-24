//! Text engine: the embedded EB Garamond variable font (SIL OFL 1.1, see
//! `assets/fonts/FONT-LICENSE.txt`), shaped with rustybuzz (GPOS kerning,
//! ligatures, `wght` axis 400–800) and rasterised with ab_glyph (exact-area
//! anti-aliasing at sub-pixel positions). Coverage is accumulated into a
//! [`Mask`] and composited in linear light.

use crate::Rgb8;
use crate::color::{decode_lut, encode_u8};
use ab_glyph::{Font, FontRef, GlyphId, PxScale, VariableFont, point};
use std::cell::RefCell;
use std::rc::Rc;

static FONT_DATA: &[u8] = include_bytes!("../assets/fonts/EBGaramond.ttf");

pub(crate) const WEIGHT_REGULAR: f32 = 400.0;

pub(crate) struct Face {
    shaper: rustybuzz::Face<'static>,
    raster: FontRef<'static>,
    /// Font units per em.
    pub upem: f32,
    /// Ascender / descender (positive, below baseline) / cap height, font units.
    pub ascender: f32,
    pub descender: f32,
    pub cap_height: f32,
}

/// Font instance at `weight` (clamped to the font's 400–800), cached per thread.
pub(crate) fn face(weight: f32) -> Rc<Face> {
    thread_local! {
        static CACHE: RefCell<Vec<(u32, Rc<Face>)>> = const { RefCell::new(Vec::new()) };
    }
    let w = weight.clamp(400.0, 800.0).round();
    let key = w as u32;
    CACHE.with(|c| {
        if let Some((_, f)) = c.borrow().iter().find(|(k, _)| *k == key) {
            return Rc::clone(f);
        }
        let mut shaper = rustybuzz::Face::from_slice(FONT_DATA, 0).expect("embedded font parses");
        let tag = rustybuzz::ttf_parser::Tag::from_bytes(b"wght");
        shaper.set_variations(&[rustybuzz::Variation { tag, value: w }]);
        let mut raster = FontRef::try_from_slice(FONT_DATA).expect("embedded font parses");
        raster.set_variation(b"wght", w);
        let upem = shaper.units_per_em() as f32;
        let cap = shaper.capital_height().map(|v| v as f32).unwrap_or(0.65 * upem);
        let f = Rc::new(Face {
            ascender: shaper.ascender() as f32,
            descender: -(shaper.descender() as f32),
            cap_height: cap,
            upem,
            shaper,
            raster,
        });
        let mut c = c.borrow_mut();
        if c.len() > 8 {
            c.remove(0);
        }
        c.push((key, Rc::clone(&f)));
        f
    })
}

/// Shaped run: glyph ids with pen positions, all in font units.
pub(crate) struct Shaped {
    glyphs: Vec<(GlyphId, f32, f32)>,
    /// Advance width in font units (tracking between clusters included).
    pub advance: f32,
}

impl Face {
    /// Shapes one line. `tracking_em` adds letter-spacing between clusters
    /// (and turns off ligatures, as CSS letter-spacing does).
    pub(crate) fn shape(&self, text: &str, tracking_em: f32) -> Shaped {
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(text);
        buf.guess_segment_properties();
        let features: Vec<rustybuzz::Feature> = if tracking_em != 0.0 {
            ["-liga", "-clig", "-dlig"].iter().filter_map(|s| s.parse().ok()).collect()
        } else {
            Vec::new()
        };
        let out = rustybuzz::shape(&self.shaper, &features, buf);
        let track = tracking_em * self.upem;
        let infos = out.glyph_infos();
        let pos = out.glyph_positions();
        let mut glyphs = Vec::with_capacity(infos.len());
        let mut x = 0.0f32;
        let mut y = 0.0f32;
        for (i, (info, p)) in infos.iter().zip(pos).enumerate() {
            glyphs.push((GlyphId(info.glyph_id as u16), x + p.x_offset as f32, y + p.y_offset as f32));
            x += p.x_advance as f32;
            y += p.y_advance as f32;
            if track != 0.0 && infos.get(i + 1).is_some_and(|n| n.cluster != info.cluster) {
                x += track;
            }
        }
        Shaped { glyphs, advance: x }
    }

    /// Width of `text` at `px` (em size) in pixels.
    pub(crate) fn width(&self, text: &str, px: f32, tracking_em: f32) -> f32 {
        self.shape(text, tracking_em).advance * px / self.upem
    }

    /// Pixel bounds (x0, y0, x1, y1) of the run's ink at the given placement.
    pub(crate) fn ink_bounds(&self, run: &Shaped, px: f32, x: f32, baseline: f32) -> Option<(i32, i32, i32, i32)> {
        let mut b: Option<(i32, i32, i32, i32)> = None;
        for g in self.outlines(run, px, x, baseline) {
            let r = g.px_bounds();
            let (x0, y0, x1, y1) = (r.min.x.floor() as i32, r.min.y.floor() as i32, r.max.x.ceil() as i32, r.max.y.ceil() as i32);
            b = Some(match b {
                None => (x0, y0, x1, y1),
                Some((a, c, d, e)) => (a.min(x0), c.min(y0), d.max(x1), e.max(y1)),
            });
        }
        b
    }

    fn outlines<'a>(
        &'a self,
        run: &'a Shaped,
        px: f32,
        x: f32,
        baseline: f32,
    ) -> impl Iterator<Item = ab_glyph::OutlinedGlyph> + 'a {
        let k = px / self.upem;
        // ab_glyph scales by height (ascent − descent), not by the em.
        let scale = PxScale::from(px * self.raster.height_unscaled() / self.upem);
        run.glyphs.iter().filter_map(move |&(id, gx, gy)| {
            self.raster.outline_glyph(id.with_scale_and_position(scale, point(x + gx * k, baseline - gy * k)))
        })
    }

    /// Adds the run's coverage to `mask` (sum, clamped to 1).
    pub(crate) fn rasterize(&self, run: &Shaped, px: f32, x: f32, baseline: f32, mask: &mut Mask) {
        for g in self.outlines(run, px, x, baseline) {
            let r = g.px_bounds();
            let (gx0, gy0) = (r.min.x as i32, r.min.y as i32);
            g.draw(|dx, dy, c| {
                let mx = gx0 + dx as i32 - mask.x0;
                let my = gy0 + dy as i32 - mask.y0;
                if mx >= 0 && my >= 0 && (mx as usize) < mask.w && (my as usize) < mask.h {
                    let a = &mut mask.a[my as usize * mask.w + mx as usize];
                    *a = (*a + c).min(1.0);
                }
            });
        }
    }
}

/// Coverage (0..1) over an axis-aligned pixel rectangle.
#[derive(Clone)]
pub(crate) struct Mask {
    pub x0: i32,
    pub y0: i32,
    pub w: usize,
    pub h: usize,
    pub a: Vec<f32>,
}

impl Mask {
    pub(crate) fn new(x0: i32, y0: i32, w: usize, h: usize) -> Self {
        Mask { x0, y0, w, h, a: vec![0.0; w * h] }
    }

    #[inline]
    fn at(&self, x: isize, y: isize) -> f32 {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            0.0
        } else {
            self.a[y as usize * self.w + x as usize]
        }
    }

    /// Morphological dilation by a disc of radius `r` px, anti-aliased.
    pub(crate) fn dilate(&self, r: f32) -> Mask {
        if r <= 0.0 {
            return self.clone();
        }
        let ri = (r + 1.0).ceil() as isize;
        let taps: Vec<(isize, isize, f32)> = (-ri..=ri)
            .flat_map(|dy| (-ri..=ri).map(move |dx| (dx, dy)))
            .filter_map(|(dx, dy)| {
                let d = ((dx * dx + dy * dy) as f32).sqrt();
                let wgt = (r + 0.5 - d).clamp(0.0, 1.0);
                (wgt > 0.0).then_some((dx, dy, wgt))
            })
            .collect();
        let mut out = Mask::new(self.x0, self.y0, self.w, self.h);
        use rayon::prelude::*;
        out.a.par_chunks_mut(self.w).enumerate().for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let mut m = 0.0f32;
                for &(dx, dy, wgt) in &taps {
                    let v = self.at(x as isize + dx, y as isize + dy) * wgt;
                    if v > m {
                        m = v;
                    }
                }
                *o = m;
            }
        });
        out
    }

    /// Gaussian blur (σ px) shifted by (dx, dy) px.
    pub(crate) fn shadow(&self, sigma: f32, dx: f32, dy: f32) -> Mask {
        let r = (sigma * 3.0).ceil().max(1.0) as isize;
        let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma).max(1e-6)).exp()).collect();
        let ks: f32 = k.iter().sum();
        let k: Vec<f32> = k.iter().map(|v| v / ks).collect();
        let (w, h) = (self.w, self.h);
        // Integer shift + horizontal pass, then vertical pass.
        let (sx, sy) = (dx.round() as isize, dy.round() as isize);
        let mut tmp = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0;
                for (i, kv) in k.iter().enumerate() {
                    acc += kv * self.at(x as isize - sx + i as isize - r, y as isize - sy);
                }
                tmp[y * w + x] = acc;
            }
        }
        let mut out = Mask::new(self.x0, self.y0, w, h);
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0;
                for (i, kv) in k.iter().enumerate() {
                    let yy = y as isize + i as isize - r;
                    if yy >= 0 && (yy as usize) < h {
                        acc += kv * tmp[yy as usize * w + x];
                    }
                }
                out.a[y * w + x] = acc.min(1.0);
            }
        }
        out
    }
}

/// Blends `color` over `img` with per-pixel alpha `mask × opacity`, in linear light.
pub(crate) fn blend(img: &mut Rgb8, mask: &Mask, color: [u8; 3], opacity: f32) {
    let lut = decode_lut();
    let c = color.map(|v| lut[v as usize]);
    for_each_covered(img, mask, |px, a| {
        let a = a * opacity;
        for ch in 0..3 {
            let d = lut[px[ch] as usize];
            px[ch] = encode_u8(d + (c[ch] - d) * a);
        }
    });
}

/// Composites text over an outline as one group (so fades don't reveal the
/// outline through the letters): group alpha = t + o(1 − t).
pub(crate) fn blend_group(
    img: &mut Rgb8,
    text: &Mask,
    outline: &Mask,
    text_color: [u8; 3],
    outline_color: [u8; 3],
    outline_opacity: f32,
    opacity: f32,
) {
    debug_assert!(text.w == outline.w && text.h == outline.h && text.x0 == outline.x0 && text.y0 == outline.y0);
    let lut = decode_lut();
    let tc = text_color.map(|v| lut[v as usize]);
    let oc = outline_color.map(|v| lut[v as usize]);
    let mut i = 0usize;
    let (w, h) = (text.w, text.h);
    for my in 0..h {
        let y = text.y0 + my as i32;
        if y < 0 || y >= img.height as i32 {
            i += w;
            continue;
        }
        for mx in 0..w {
            let x = text.x0 + mx as i32;
            let (t, o) = (text.a[i], outline.a[i] * outline_opacity);
            i += 1;
            if x < 0 || x >= img.width as i32 || (t <= 0.0 && o <= 0.0) {
                continue;
            }
            let ga = t + o * (1.0 - t);
            let a = ga * opacity;
            let p = (y as usize * img.width as usize + x as usize) * 3;
            for ch in 0..3 {
                let gc = (tc[ch] * t + oc[ch] * o * (1.0 - t)) / ga;
                let d = lut[img.data[p + ch] as usize];
                img.data[p + ch] = encode_u8(d + (gc - d) * a);
            }
        }
    }
}

fn for_each_covered(img: &mut Rgb8, mask: &Mask, mut f: impl FnMut(&mut [u8], f32)) {
    let (iw, ih) = (img.width as i32, img.height as i32);
    for my in 0..mask.h {
        let y = mask.y0 + my as i32;
        if y < 0 || y >= ih {
            continue;
        }
        for mx in 0..mask.w {
            let x = mask.x0 + mx as i32;
            let a = mask.a[my * mask.w + mx];
            if x < 0 || x >= iw || a <= 0.0 {
                continue;
            }
            let p = (y as usize * iw as usize + x as usize) * 3;
            f(&mut img.data[p..p + 3], a);
        }
    }
}
