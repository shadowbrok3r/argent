//! Contact sheets, text and burned-in reel hooks.
//!
//! All text is EB Garamond (embedded, SIL OFL 1.1), shaped with OpenType
//! kerning, rasterised with exact-area anti-aliasing and blended in linear
//! light. Lines wrap **only at spaces** (and explicit `\n`): never inside a
//! word and never at a hyphen, so "SOUL-FIRE" stays whole. A word wider than
//! the line shrinks the type instead of breaking. No-break spaces (U+00A0)
//! glue words together.

use crate::Rgb8;
use crate::color::{decode_lut, encode_u8};
use crate::text::{self, Face, Mask, Shaped, WEIGHT_REGULAR};
use rayon::prelude::*;

pub struct Tile {
    pub image: Rgb8,
    pub caption: String,
}

pub struct SheetStyle {
    pub columns: u32,
    pub tile_width: u32,
    pub background: [u8; 3],
    pub text: [u8; 3],
    pub title: Option<String>,
    pub subtitle: Option<String>,
}

/// Horizontal anchor for [`draw_text`]: `x` is the left edge, centre or right edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

// ---------------------------------------------------------------------------
// Plain text.

/// Advance width of one line of regular-weight text at em size `px`.
pub fn text_width(text: &str, px: f32) -> f32 {
    text::face(WEIGHT_REGULAR).width(text, px, 0.0)
}

/// Draws regular-weight text with its line box top at `y` (baseline at
/// `y + ascender`), `px` = em size. `\n` starts a new line (1.25 em apart).
/// Returns the widest line's advance width in pixels.
pub fn draw_text(img: &mut Rgb8, text: &str, x: f32, y: f32, px: f32, color: [u8; 3], align: Align) -> f32 {
    draw_text_weighted(img, text, x, y, px, color, align, WEIGHT_REGULAR)
}

/// [`draw_text`] at a variable-font weight (400 regular … 800 extra bold).
#[allow(clippy::too_many_arguments)]
pub fn draw_text_weighted(
    img: &mut Rgb8,
    text: &str,
    x: f32,
    y: f32,
    px: f32,
    color: [u8; 3],
    align: Align,
    weight: f32,
) -> f32 {
    if text.is_empty() || px.is_nan() || px <= 0.0 || img.width == 0 || img.height == 0 {
        return 0.0;
    }
    let face = text::face(weight);
    let k = px / face.upem;
    let mut widest = 0.0f32;
    for (li, line) in text.split('\n').enumerate() {
        let run = face.shape(line, 0.0);
        let w = run.advance * k;
        widest = widest.max(w);
        let left = match align {
            Align::Left => x,
            Align::Center => x - w * 0.5,
            Align::Right => x - w,
        };
        let baseline = y + li as f32 * px * 1.25 + face.ascender * k;
        paint_runs(img, &face, &[(run, left, baseline)], px, color);
    }
    widest
}

fn paint_runs(img: &mut Rgb8, face: &Face, runs: &[(Shaped, f32, f32)], px: f32, color: [u8; 3]) {
    let Some((x0, y0, x1, y1)) = union_bounds(face, runs, px) else { return };
    let (x0, y0) = (x0.max(0), y0.max(0));
    let (x1, y1) = (x1.min(img.width as i32), y1.min(img.height as i32));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let mut mask = Mask::new(x0, y0, (x1 - x0) as usize, (y1 - y0) as usize);
    for (run, x, b) in runs {
        face.rasterize(run, px, *x, *b, &mut mask);
    }
    text::blend(img, &mask, color, 1.0);
}

fn union_bounds(face: &Face, runs: &[(Shaped, f32, f32)], px: f32) -> Option<(i32, i32, i32, i32)> {
    runs.iter().filter_map(|(r, x, b)| face.ink_bounds(r, px, *x, *b)).reduce(|a, b| {
        (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
    })
}

// ---------------------------------------------------------------------------
// Wrapping.

/// Words of one paragraph: split on ASCII spaces/tabs only.
fn words(par: &str) -> Vec<&str> {
    par.split([' ', '\t', '\r']).filter(|w| !w.is_empty()).collect()
}

/// Widths (font units) of every candidate line `words[i..j]`, `w[i][j - i - 1]`.
struct LineWidths {
    w: Vec<Vec<f32>>,
}

impl LineWidths {
    fn new(face: &Face, words: &[&str], tracking: f32) -> Self {
        let n = words.len();
        // Long paragraphs (captions) only need lines up to ~24 words.
        let span = n.min(24);
        let w = (0..n)
            .map(|i| (i + 1..=(i + span).min(n)).map(|j| face.shape(&words[i..j].join(" "), tracking).advance).collect())
            .collect();
        LineWidths { w }
    }

    /// Width of words[i..j] in font units (∞ when longer than the cached span).
    fn get(&self, i: usize, j: usize) -> f32 {
        self.w[i].get(j - i - 1).copied().unwrap_or(f32::INFINITY)
    }
}

/// Breaks `n` words into lines no wider than `max_u` (font units); a single
/// over-long word gets a line of its own. Returns line start indices.
fn break_lines(lw: &LineWidths, n: usize, max_u: f32, balance: bool) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let fits = |i: usize, j: usize| j == i + 1 || lw.get(i, j) <= max_u;
    // Greedy fill gives the minimum line count.
    let mut greedy = vec![0usize];
    let mut start = 0;
    for j in 2..=n {
        if !fits(start, j) {
            start = j - 1;
            greedy.push(start);
        }
    }
    let lines = greedy.len();
    if !balance || lines == 1 || n > 40 {
        return greedy;
    }
    // Balanced: among partitions into `lines` lines, minimise the widest line,
    // then the sum of squared slack against it.
    let inf = f32::INFINITY;
    let width = |i: usize, j: usize| lw.get(i, j);
    let mut best = vec![vec![inf; n + 1]; lines + 1];
    best[0][0] = 0.0;
    for k in 1..=lines {
        for j in 1..=n {
            for i in (k - 1)..j {
                if best[k - 1][i] < inf && fits(i, j) {
                    best[k][j] = best[k][j].min(best[k - 1][i].max(width(i, j)));
                }
            }
        }
    }
    let target = best[lines][n];
    if !target.is_finite() {
        return greedy;
    }
    let cap = target * (1.0 + 1e-6);
    // Second pass: minimise Σ (target − w)² subject to every line ≤ target.
    let mut cost = vec![vec![inf; n + 1]; lines + 1];
    let mut from = vec![vec![0usize; n + 1]; lines + 1];
    cost[0][0] = 0.0;
    for k in 1..=lines {
        for j in 1..=n {
            for i in (k - 1)..j {
                let w = width(i, j);
                if cost[k - 1][i] < inf && fits(i, j) && w <= cap {
                    let c = cost[k - 1][i] + (target - w) * (target - w);
                    if c < cost[k][j] {
                        cost[k][j] = c;
                        from[k][j] = i;
                    }
                }
            }
        }
    }
    if !cost[lines][n].is_finite() {
        return greedy;
    }
    let mut starts = vec![0usize; lines];
    let mut j = n;
    for k in (1..=lines).rev() {
        let i = from[k][j];
        starts[k - 1] = i;
        j = i;
    }
    starts
}

fn assemble(words: &[&str], starts: &[usize]) -> Vec<String> {
    starts
        .iter()
        .enumerate()
        .map(|(l, &s)| {
            let e = starts.get(l + 1).copied().unwrap_or(words.len());
            words[s..e].join(" ")
        })
        .collect()
}

/// Greedy word wrap of regular-weight text at em size `px` into lines no
/// wider than `max_width` px, breaking only at spaces and `\n`.
pub fn wrap_text(text: &str, px: f32, max_width: f32) -> Vec<String> {
    let face = text::face(WEIGHT_REGULAR);
    let max_u = max_width / px.max(1e-3) * face.upem;
    let mut out = Vec::new();
    for par in text.split('\n') {
        let w = words(par);
        let lw = LineWidths::new(&face, &w, 0.0);
        out.extend(assemble(&w, &break_lines(&lw, w.len(), max_u, false)));
    }
    out
}

// ---------------------------------------------------------------------------
// Reel hooks.

/// Look of a burned-in hook. Sizes in em are relative to the font size,
/// which is itself relative to the frame width, so a style works at any
/// resolution. Defaults: cream EB Garamond semibold caps, ~78 px on a
/// 1080-wide reel, centred at 20 % of the height (upper third, below the
/// Instagram/TikTok top bar), soft shadow plus a thin dark outline.
#[derive(Clone, Debug, PartialEq)]
pub struct HookStyle {
    /// Em size as a fraction of the frame width.
    pub size: f32,
    /// Variable-font weight, 400 (regular) … 800 (extra bold).
    pub weight: f32,
    pub uppercase: bool,
    /// Extra letter-spacing, em.
    pub tracking: f32,
    /// Baseline-to-baseline distance, em.
    pub line_height: f32,
    /// Widest line as a fraction of the frame width.
    pub max_width: f32,
    /// Vertical centre of the block as a fraction of the frame height; the
    /// block is kept inside the upper third when it fits.
    pub center_y: f32,
    /// More lines than this shrinks the type (down to half size).
    pub max_lines: usize,
    /// Balance line lengths (better for centred titles) instead of greedy fill.
    pub balance: bool,
    pub color: [u8; 3],
    pub outline_color: [u8; 3],
    /// Outline width, em (0 = none).
    pub outline: f32,
    pub outline_opacity: f32,
    pub shadow_color: [u8; 3],
    /// Shadow blur σ, em.
    pub shadow_blur: f32,
    /// Shadow offset (x, y), em.
    pub shadow_offset: (f32, f32),
    pub shadow_opacity: f32,
    /// Overall opacity 0..1, for fades.
    pub opacity: f32,
}

impl Default for HookStyle {
    fn default() -> Self {
        HookStyle {
            size: 0.072,
            weight: 600.0,
            uppercase: true,
            tracking: 0.04,
            line_height: 1.12,
            max_width: 0.84,
            center_y: 0.20,
            max_lines: 3,
            balance: true,
            color: [244, 236, 220],
            outline_color: [12, 9, 6],
            outline: 0.035,
            outline_opacity: 0.7,
            shadow_color: [0, 0, 0],
            shadow_blur: 0.08,
            shadow_offset: (0.0, 0.03),
            shadow_opacity: 0.6,
            opacity: 1.0,
        }
    }
}

/// Where a hook lands on a frame (all values in pixels).
#[derive(Clone, Debug, PartialEq)]
pub struct HookLayout {
    pub lines: Vec<String>,
    /// Em size actually used (smaller than requested if it had to shrink).
    pub px: f32,
    pub line_widths: Vec<f32>,
    pub baselines: Vec<f32>,
    /// Allowed line width.
    pub max_width: f32,
    /// Optical block extent: cap line of the first line to the last baseline
    /// (plus descenders for mixed case).
    pub top: f32,
    pub bottom: f32,
}

/// Lays a hook out for a `frame_width`×`frame_height` frame without drawing.
pub fn hook_layout(frame_width: u32, frame_height: u32, text: &str, style: &HookStyle) -> HookLayout {
    let face = text::face(style.weight);
    hook_layout_with(&face, frame_width as f32, frame_height as f32, text, style)
}

fn hook_layout_with(face: &Face, fw: f32, fh: f32, text: &str, style: &HookStyle) -> HookLayout {
    let txt = if style.uppercase { text.to_uppercase() } else { text.to_owned() };
    let paras: Vec<Vec<&str>> = txt.split('\n').map(words).filter(|w| !w.is_empty()).collect();
    let widths: Vec<LineWidths> = paras.iter().map(|w| LineWidths::new(face, w, style.tracking)).collect();
    let max_width = style.max_width.clamp(0.05, 1.0) * fw;
    let base_px = (style.size * fw).max(1.0);
    let min_px = base_px * 0.5;
    let (top_limit, bottom_limit) = (fh * 0.05, fh / 3.0);
    let cap = face.cap_height / face.upem;
    let desc = if style.uppercase { 0.0 } else { 0.8 * face.descender / face.upem };
    let mut px = base_px;
    let (starts, block_h) = loop {
        let max_u = max_width / px * face.upem;
        let starts: Vec<Vec<usize>> =
            paras.iter().zip(&widths).map(|(w, lw)| break_lines(lw, w.len(), max_u, style.balance)).collect();
        let n_lines: usize = starts.iter().map(Vec::len).sum();
        let words_fit = widths.iter().zip(&paras).all(|(lw, w)| (0..w.len()).all(|i| lw.get(i, i + 1) <= max_u));
        let block_h = px * (cap + desc + style.line_height * n_lines.saturating_sub(1) as f32);
        let fits = words_fit && n_lines <= style.max_lines.max(1) && block_h <= bottom_limit - top_limit;
        if fits || px <= min_px {
            break (starts, block_h);
        }
        px = (px * 0.94).max(min_px);
    };
    let k = px / face.upem;
    let mut lines = Vec::new();
    let mut line_widths = Vec::new();
    for ((w, lw), st) in paras.iter().zip(&widths).zip(&starts) {
        for (l, &s) in st.iter().enumerate() {
            let e = st.get(l + 1).copied().unwrap_or(w.len());
            lines.push(w[s..e].join(" "));
            let u = lw.get(s, e);
            line_widths.push(if u.is_finite() { u * k } else { face.width(&w[s..e].join(" "), px, style.tracking) });
        }
    }
    let centre = style.center_y * fh;
    let mut top = centre - block_h * 0.5;
    top = top.min(bottom_limit - block_h).max(top_limit);
    let first_baseline = top + cap * px;
    let baselines = (0..lines.len()).map(|i| first_baseline + i as f32 * style.line_height * px).collect();
    HookLayout { lines, px, line_widths, baselines, max_width, top, bottom: top + block_h }
}

/// Burns `text` into `frame` as a reel hook (see [`HookStyle`]).
pub fn overlay_hook(frame: &mut Rgb8, text: &str, style: &HookStyle) {
    let opacity = style.opacity.clamp(0.0, 1.0);
    if frame.width == 0 || frame.height == 0 || text.trim().is_empty() || opacity <= 0.0 {
        return;
    }
    let face = text::face(style.weight);
    let (fw, fh) = (frame.width as f32, frame.height as f32);
    let layout = hook_layout_with(&face, fw, fh, text, style);
    let px = layout.px;
    let runs: Vec<(Shaped, f32, f32)> = layout
        .lines
        .iter()
        .zip(&layout.line_widths)
        .zip(&layout.baselines)
        .map(|((line, w), b)| (face.shape(line, style.tracking), (fw - w) * 0.5, *b))
        .collect();
    let Some((x0, y0, x1, y1)) = union_bounds(&face, &runs, px) else { return };
    let outline_px = (style.outline * px).max(0.0);
    let sigma = (style.shadow_blur * px).max(0.0);
    let (sdx, sdy) = (style.shadow_offset.0 * px, style.shadow_offset.1 * px);
    let pad = (outline_px + 3.0 * sigma + sdx.abs().max(sdy.abs()) + 3.0).ceil() as i32;
    let (mx0, my0) = ((x0 - pad).max(0), (y0 - pad).max(0));
    let (mx1, my1) = ((x1 + pad).min(frame.width as i32), (y1 + pad).min(frame.height as i32));
    if mx1 <= mx0 || my1 <= my0 {
        return;
    }
    let mut mask = Mask::new(mx0, my0, (mx1 - mx0) as usize, (my1 - my0) as usize);
    for (run, x, b) in &runs {
        face.rasterize(run, px, *x, *b, &mut mask);
    }
    let outline = if outline_px > 0.0 && style.outline_opacity > 0.0 {
        mask.dilate(outline_px)
    } else {
        Mask::new(mask.x0, mask.y0, mask.w, mask.h)
    };
    if style.shadow_opacity > 0.0 && sigma > 0.0 {
        let src = if outline_px > 0.0 { &outline } else { &mask };
        let shadow = src.shadow(sigma, sdx, sdy);
        text::blend(frame, &shadow, style.shadow_color, style.shadow_opacity.clamp(0.0, 1.0) * opacity);
    }
    text::blend_group(
        frame,
        &mask,
        &outline,
        style.color,
        style.outline_color,
        style.outline_opacity.clamp(0.0, 1.0),
        opacity,
    );
}

// ---------------------------------------------------------------------------
// Contact sheets.

/// Outer margin of a sheet: 8 % of the tile width, at least 16 px.
pub fn sheet_margin(tile_width: u32) -> u32 {
    ((tile_width as f32 * 0.08).round() as u32).max(16)
}

/// Gap between tiles: 5 % of the tile width, at least 10 px.
pub fn sheet_gutter(tile_width: u32) -> u32 {
    ((tile_width as f32 * 0.05).round() as u32).max(10)
}

/// Resizes to `width` keeping the aspect ratio, filtering in linear light
/// (so specular glints keep their energy when downscaled).
fn fit_width(img: &Rgb8, width: u32) -> Rgb8 {
    if img.width == 0 || img.height == 0 {
        return Rgb8::new(0, 0);
    }
    if img.width == width {
        return img.clone();
    }
    let height = ((img.height as f64 * width as f64 / img.width as f64).round() as u32).max(1);
    let lut = decode_lut();
    let lin: Vec<f32> = img.data.par_iter().map(|&v| lut[v as usize]).collect();
    let src = image::ImageBuffer::<image::Rgb<f32>, Vec<f32>>::from_raw(img.width, img.height, lin)
        .expect("rgb8 buffer matches its size");
    let filter =
        if width < img.width { image::imageops::FilterType::Lanczos3 } else { image::imageops::FilterType::CatmullRom };
    let dst = image::imageops::resize(&src, width, height, filter);
    Rgb8 { width, height, data: dst.into_raw().par_iter().map(|&v| encode_u8(v)).collect() }
}

fn blit(dst: &mut Rgb8, src: &Rgb8, x: i32, y: i32) {
    for sy in 0..src.height as i32 {
        let dy = y + sy;
        if dy < 0 || dy >= dst.height as i32 {
            continue;
        }
        let sx0 = (-x).max(0);
        let sx1 = (src.width as i32).min(dst.width as i32 - x);
        if sx1 <= sx0 {
            continue;
        }
        let s = ((sy as u32 * src.width + sx0 as u32) * 3) as usize;
        let d = ((dy as u32 * dst.width + (x + sx0) as u32) * 3) as usize;
        let n = ((sx1 - sx0) * 3) as usize;
        dst.data[d..d + n].copy_from_slice(&src.data[s..s + n]);
    }
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let lut = decode_lut();
    std::array::from_fn(|i| encode_u8(lut[a[i] as usize] + (lut[b[i] as usize] - lut[a[i] as usize]) * t))
}

/// Lays tiles out in a grid, each scaled to `tile_width`, with a centred
/// caption under each (wrapped at spaces) and an optional title/subtitle.
///
/// Width = 2·[`sheet_margin`] + n·`tile_width` + (n−1)·[`sheet_gutter`], with
/// n = `min(columns, tiles)`. Rows are as tall as their tallest tile plus
/// the caption lines; shorter tiles are centred vertically.
pub fn contact_sheet(tiles: &[Tile], style: &SheetStyle) -> Rgb8 {
    let tw = style.tile_width.max(8);
    let cols = (style.columns.max(1) as usize).min(tiles.len().max(1));
    let margin = sheet_margin(tw);
    let gutter = sheet_gutter(tw);
    let width = 2 * margin + cols as u32 * tw + (cols as u32 - 1) * gutter;
    let regular = text::face(WEIGHT_REGULAR);
    let up = 1.0 / regular.upem;

    // Header.
    let inner = (width - 2 * margin) as f32;
    let title_px = (inner * 0.045).clamp(20.0, 110.0);
    let sub_px = (title_px * 0.5).max(13.0);
    let mut y = margin as f32;
    let title_y = style.title.as_ref().map(|_| {
        let t = y;
        y += title_px * 1.3;
        t
    });
    let sub_y = style.subtitle.as_ref().map(|_| {
        let t = y;
        y += sub_px * 1.6;
        t
    });
    if title_y.is_some() || sub_y.is_some() {
        y += margin as f32 * 0.5;
    }
    let header_h = y.round() as u32;

    // Tiles and captions.
    let cap_px = (tw as f32 * 0.055).max(13.0);
    let cap_lh = (cap_px * 1.3).round() as u32;
    let cap_gap = (cap_px * 0.55).round() as u32;
    let scaled: Vec<Rgb8> = tiles.par_iter().map(|t| fit_width(&t.image, tw)).collect();
    let captions: Vec<Vec<String>> =
        tiles.iter().map(|t| wrap_text(&t.caption, cap_px, tw as f32 - cap_px * 0.5)).collect();
    let rows: Vec<(u32, u32)> = scaled
        .chunks(cols)
        .zip(captions.chunks(cols))
        .map(|(imgs, caps)| {
            let ih = imgs.iter().map(|i| i.height).max().unwrap_or(0);
            let lines = caps.iter().map(Vec::len).max().unwrap_or(0) as u32;
            (ih, if lines > 0 { cap_gap + lines * cap_lh } else { 0 })
        })
        .collect();
    let body: u32 = rows.iter().map(|(a, b)| a + b).sum::<u32>() + rows.len().saturating_sub(1) as u32 * gutter;
    let height = header_h + body + margin;

    let mut sheet = Rgb8::new(width, height);
    for px in sheet.data.chunks_exact_mut(3) {
        px.copy_from_slice(&style.background);
    }
    let centre = width as f32 * 0.5;
    if let (Some(t), Some(ty)) = (&style.title, title_y) {
        draw_text_weighted(&mut sheet, t, centre, ty, title_px, style.text, Align::Center, 500.0);
    }
    if let (Some(s), Some(sy)) = (&style.subtitle, sub_y) {
        let dim = mix(style.text, style.background, 0.35);
        draw_text(&mut sheet, s, centre, sy, sub_px, dim, Align::Center);
    }
    let mut row_y = header_h;
    for (r, (img_h, cap_h)) in rows.iter().enumerate() {
        for c in 0..cols {
            let i = r * cols + c;
            if i >= tiles.len() {
                break;
            }
            let x = margin + c as u32 * (tw + gutter);
            let img = &scaled[i];
            blit(&mut sheet, img, x as i32, (row_y + (img_h - img.height) / 2) as i32);
            let cap_top = (row_y + img_h + cap_gap) as f32;
            for (l, line) in captions[i].iter().enumerate() {
                // Cap line of the first caption line sits at `cap_top`.
                let ly = cap_top + l as f32 * cap_lh as f32 - (regular.ascender - regular.cap_height) * up * cap_px;
                draw_text(&mut sheet, line, x as f32 + tw as f32 * 0.5, ly, cap_px, style.text, Align::Center);
            }
        }
        row_y += img_h + cap_h + gutter;
    }
    sheet
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tonemap::{AgxLook, Grade, ToneMap, develop};
    use crate::{Image, io};

    fn out_dir() -> std::path::PathBuf {
        crate::video::tests::out_dir()
    }

    fn assert_whole_words(lines: &[String], text: &str) {
        let want: Vec<String> = text.to_uppercase().split_whitespace().map(str::to_owned).collect();
        let got: Vec<String> = lines.iter().flat_map(|l| l.split(' ').map(str::to_owned)).collect();
        assert_eq!(got, want, "lines {lines:?} must be the original words in order");
        for l in lines {
            assert!(!l.ends_with('-') && !l.starts_with('-'), "line {l:?} breaks at a hyphen");
            assert_eq!(l.trim(), l, "stray spaces in {l:?}");
        }
    }

    #[test]
    fn hook_wraps_only_at_spaces() {
        let style = HookStyle::default();
        let text = "Wait for the soul-fire";
        let mut saw_multiline = false;
        for fw in (180..=1080).step_by(20) {
            let l = hook_layout(fw, fw * 16 / 9, text, &style);
            assert_whole_words(&l.lines, text);
            assert!(l.lines.iter().any(|x| x == "SOUL-FIRE" || x.ends_with(" SOUL-FIRE")), "{fw}: {:?}", l.lines);
            for w in &l.line_widths {
                assert!(*w <= l.max_width + 0.5, "{fw}: line {w} wider than {}", l.max_width);
            }
            saw_multiline |= l.lines.len() > 1;
            assert!(l.top >= 0.0 && l.bottom <= (fw * 16 / 9) as f32 / 3.0 + 0.5, "{fw}: {l:?}");
        }
        assert!(saw_multiline);
        // So narrow that "SOUL-FIRE" alone overflows: the type shrinks instead of splitting.
        let narrow = HookStyle { max_width: 0.2, ..HookStyle::default() };
        let l = hook_layout(1080, 1920, text, &narrow);
        assert_whole_words(&l.lines, text);
        assert!(l.px < 0.072 * 1080.0, "font should shrink, px = {}", l.px);
        assert!(l.lines.contains(&"SOUL-FIRE".to_owned()), "{:?}", l.lines);
    }

    #[test]
    fn hook_balances_lines() {
        let l = hook_layout(1080, 1920, "Watch the souls ignite", &HookStyle::default());
        assert_eq!(l.lines, ["WATCH THE", "SOULS IGNITE"], "{l:?}");
        let g = hook_layout(1080, 1920, "Watch the souls ignite", &HookStyle { balance: false, ..HookStyle::default() });
        assert_eq!(g.lines, ["WATCH THE SOULS", "IGNITE"], "{g:?}");
        let one = hook_layout(1080, 1920, "Look into the eyes", &HookStyle::default());
        assert_eq!(one.lines.len(), 1, "{one:?}");
        // NBSP glues, explicit newlines break.
        let nb = hook_layout(1080, 1920, "THE\u{a0}SOULS\nIGNITE", &HookStyle::default());
        assert_eq!(nb.lines, ["THE\u{a0}SOULS", "IGNITE"]);
    }

    #[test]
    fn wrap_text_never_splits_words() {
        let t = "A dark-mythology skull ring, cast in sterling silver";
        let lines = wrap_text(t, 20.0, 120.0);
        assert!(lines.len() > 2);
        assert_eq!(lines.join(" "), t);
        for l in &lines {
            assert!(text_width(l, 20.0) <= 120.0 || !l.contains(' '), "{l}");
        }
    }

    #[test]
    fn draw_text_antialiases_and_kerns() {
        let mut img = Rgb8::new(300, 80);
        let w = draw_text(&mut img, "AVATAR Wavy", 10.0, 10.0, 40.0, [255, 255, 255], Align::Left);
        assert!(w > 100.0 && w < 290.0, "{w}");
        let values: std::collections::BTreeSet<u8> = img.data.iter().copied().collect();
        assert!(values.len() > 50, "anti-aliased edges expected, got {} levels", values.len());
        // GPOS kerning: "AV" sits tighter than A + V.
        assert!(text_width("AV", 100.0) < text_width("A", 100.0) + text_width("V", 100.0) - 1.0);
        // Centre/right alignment anchor at x.
        let mut a = Rgb8::new(200, 60);
        let wc = draw_text(&mut a, "Hi", 100.0, 5.0, 30.0, [255; 3], Align::Center);
        let lit: Vec<u32> =
            (0..200u32).filter(|x| (0..60u32).any(|y| a.data[((y * 200 + x) * 3) as usize] > 128)).collect();
        let mid = (lit[0] + lit[lit.len() - 1]) as f32 * 0.5;
        assert!((mid - 100.0).abs() < 4.0 && wc > 10.0, "centred at {mid}");
        // Off-image text is clipped, not a panic.
        draw_text(&mut a, "clip me", -50.0, -20.0, 30.0, [255; 3], Align::Right);
        draw_text(&mut a, "clip me", 190.0, 55.0, 300.0, [255; 3], Align::Left);
    }

    #[test]
    fn hook_overlay_is_legible_and_fades() {
        // Half black, half bright: the hook must read on both.
        let (w, h) = (1080u32, 1920u32);
        let mut frame = Rgb8::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let o = ((y * w + x) * 3) as usize;
                let v = if x < w / 2 { 6 } else { 236 };
                frame.data[o..o + 3].copy_from_slice(&[v, v, v - 4]);
            }
        }
        let style = HookStyle::default();
        let layout = hook_layout(w, h, "Wait for the soul-fire", &style);
        let mut full = frame.clone();
        overlay_hook(&mut full, "Wait for the soul-fire", &style);
        assert!(layout.top > h as f32 * 0.05 && layout.bottom < h as f32 / 3.0);
        // Changed pixels stay in the hook band.
        let changed = |img: &Rgb8| {
            let mut ys = Vec::new();
            for y in 0..h {
                let row = (y * w * 3) as usize..((y + 1) * w * 3) as usize;
                if img.data[row.clone()] != frame.data[row] {
                    ys.push(y);
                }
            }
            ys
        };
        let ys = changed(&full);
        assert!(!ys.is_empty());
        assert!(*ys.first().unwrap() as f32 >= layout.top - 0.25 * layout.px);
        assert!((*ys.last().unwrap() as f32) <= layout.bottom + 0.5 * layout.px);
        // Dark outline shows on the bright half, cream text on the dark half.
        let band = |img: &Rgb8, x0: u32, x1: u32| {
            let (mut lo, mut hi) = (255u8, 0u8);
            for y in layout.top as u32..layout.bottom as u32 {
                for x in x0..x1 {
                    let v = img.data[((y * w + x) * 3) as usize];
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            (lo, hi)
        };
        let (_, dark_hi) = band(&full, 0, w / 2);
        let (bright_lo, _) = band(&full, w / 2, w);
        assert!(dark_hi > 200, "text visible on black: max {dark_hi}");
        assert!(bright_lo < 120, "outline/shadow visible on white: min {bright_lo}");
        // Opacity scales the effect; 0 is a no-op.
        let mut half = frame.clone();
        overlay_hook(&mut half, "Wait for the soul-fire", &HookStyle { opacity: 0.5, ..style.clone() });
        let (_, half_hi) = band(&half, 0, w / 2);
        assert!(half_hi < dark_hi && half_hi > 60, "half {half_hi} vs full {dark_hi}");
        let mut none = frame.clone();
        overlay_hook(&mut none, "Wait for the soul-fire", &HookStyle { opacity: 0.0, ..style });
        assert_eq!(none.data, frame.data);
        io::write_png(&out_dir().join("hook-split.png"), &full).unwrap();
    }

    #[test]
    fn hook_overlay_speed() {
        let mut frame = Rgb8::new(1080, 1920);
        let style = HookStyle::default();
        overlay_hook(&mut frame, "Wait for the soul-fire", &style);
        let t = std::time::Instant::now();
        for i in 0..10 {
            overlay_hook(&mut frame, "Wait for the soul-fire", &HookStyle { opacity: i as f32 / 10.0, ..style.clone() });
        }
        let ms = t.elapsed().as_secs_f64() * 1e3 / 10.0;
        eprintln!("overlay_hook 1080x1920: {ms:.1} ms/frame");
        assert!(ms < 500.0);
    }

    fn tile(w: u32, h: u32, rgb: [u8; 3]) -> Rgb8 {
        let mut t = Rgb8::new(w, h);
        for p in t.data.chunks_exact_mut(3) {
            p.copy_from_slice(&rgb);
        }
        t
    }

    #[test]
    fn contact_sheet_dimensions() {
        let tiles: Vec<Tile> = (0..5u8)
            .map(|i| Tile { image: tile(100, 150, [40 * i + 20, 100, 200 - 30 * i]), caption: format!("Tile {i}") })
            .collect();
        let style = SheetStyle {
            columns: 3,
            tile_width: 240,
            background: [10, 10, 12],
            text: [230, 225, 215],
            title: Some("Sheet".into()),
            subtitle: None,
        };
        let s = contact_sheet(&tiles, &style);
        let (m, g) = (sheet_margin(240), sheet_gutter(240));
        assert_eq!((m, g), (19, 12));
        assert_eq!(s.width, 2 * m + 3 * 240 + 2 * g);
        // Two rows of 240×360 tiles plus captions, header and margins.
        assert!(s.height > 2 * 360 + g + 2 * m && s.height < 2 * 360 + 260, "height {}", s.height);
        assert_eq!(s.data.len(), (s.width * s.height * 3) as usize);
        // Centre of the first tile carries its colour; the margin is background.
        let px = |x: u32, y: u32| &s.data[((y * s.width + x) * 3) as usize..][..3];
        assert_eq!(px(2, 2), &[10, 10, 12]);
        let first = (0..s.height).find(|&y| px(m + 120, y) == [20, 100, 200]).expect("first tile drawn");
        assert_eq!(px(m + 120, first + 180), &[20, 100, 200]);
        // Fewer tiles than columns narrows the sheet; no tiles still works.
        assert_eq!(contact_sheet(&tiles[..2], &style).width, 2 * m + 2 * 240 + g);
        let empty = contact_sheet(&[], &style);
        assert!(empty.width > 0 && empty.height > 0);
    }

    /// Analytic HDR "studio" scene: gold and silver spheres, an EV ramp.
    fn synthetic_scene(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        let spheres = [
            (0.32f32, 0.40f32, 0.24f32, [1.0f32, 0.71, 0.29]),
            (0.72, 0.42, 0.20, [0.97, 0.96, 0.91]),
        ];
        let l = {
            let v = [-0.5f32, 0.6, 0.62];
            let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            v.map(|c| c / n)
        };
        for y in 0..h {
            for x in 0..w {
                let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
                let mut c = [0.004 + 0.01 * (1.0 - v); 3];
                for (cx, cy, r, f0) in spheres {
                    let (dx, dy) = ((u - cx) / r, (v - cy) / r * h as f32 / w as f32);
                    let d2 = dx * dx + dy * dy;
                    if d2 < 1.0 {
                        let n = [dx, -dy, (1.0 - d2).sqrt()];
                        let ndl = (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.0);
                        // Mirror-like: reflect a softbox (HDR) plus a dim environment.
                        let rz = 2.0 * n[2] * n[2] - 1.0;
                        let refl = [2.0 * n[2] * n[0], 2.0 * n[2] * n[1], rz];
                        let rl = (refl[0] * l[0] + refl[1] * l[1] + refl[2] * l[2]).max(0.0);
                        let spec = 60.0 * rl.powi(80) + 0.6 * rl.powi(4);
                        let fres = 1.0 - n[2];
                        c = std::array::from_fn(|i| f0[i] * (0.05 + 0.25 * ndl + spec) + 0.3 * fres.powi(5));
                    }
                }
                if v > 0.86 {
                    let ev = -8.0 + 16.0 * u;
                    let g = 0.18 * 2f32.powf(ev);
                    c = if v > 0.93 { [g, g * 0.55, g * 0.12] } else { [g; 3] };
                }
                img.set(x, y, c);
            }
        }
        img
    }

    /// Writes the eyeballing sheets to out/post-selftest/.
    #[test]
    fn selftest_sheets() {
        let scene = synthetic_scene(540, 720);
        let ops: [(ToneMap, &str); 6] = [
            (ToneMap::AgX(AgxLook::Base), "AgX Base"),
            (ToneMap::AgX(AgxLook::Punchy), "AgX Punchy"),
            (ToneMap::AgX(AgxLook::Golden), "AgX Golden"),
            (ToneMap::AcesFitted, "ACES fitted (Hill)"),
            (ToneMap::KhronosNeutral, "Khronos PBR Neutral"),
            (ToneMap::Reinhard, "Reinhard"),
        ];
        let tiles: Vec<Tile> = ops
            .iter()
            .map(|(tm, name)| Tile {
                image: develop(&scene, &Grade { exposure_ev: 0.0, tonemap: *tm, saturation: 1.0 }),
                caption: (*name).to_owned(),
            })
            .collect();
        let style = SheetStyle {
            columns: 3,
            tile_width: 360,
            background: [12, 11, 10],
            text: [236, 228, 212],
            title: Some("argent-post self-test".into()),
            subtitle: Some("Tone maps on a synthetic HDR gold and silver studio scene, EV ramp −8…+8".into()),
        };
        let sheet = contact_sheet(&tiles, &style);
        let dir = out_dir();
        io::write_png(&dir.join("tonemaps-sheet.png"), &sheet).unwrap();
        assert_eq!(sheet.width, 2 * sheet_margin(360) + 3 * 360 + 2 * sheet_gutter(360));

        // A reel frame with the hook burned in.
        let reel = synthetic_scene(1080, 1920);
        let mut frame = develop(&reel, &Grade::default());
        overlay_hook(&mut frame, "Wait for the soul-fire", &HookStyle::default());
        io::write_png(&dir.join("hook-example.png"), &frame).unwrap();
    }
}
