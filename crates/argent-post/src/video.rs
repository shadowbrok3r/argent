//! Video output through an `ffmpeg` child process.
//!
//! [`VideoWriter`] streams raw RGB24 frames to ffmpeg's stdin (a blocking
//! write, so a slow encoder applies backpressure to the renderer) and
//! produces a Forge-ready MP4: H.264 High, `yuv420p`, CRF from the spec with
//! a `-maxrate`/`-bufsize` VBV cap, `+faststart`, 2 s keyframe interval and
//! x264 `aq-mode=3` (keeps near-black studio gradients from banding).
//!
//! Colour: rgb24 → yuv is converted with the **BT.709** matrix into limited
//! (tv) range by the scale filter itself
//! (`scale=out_color_matrix=bt709:out_range=tv,format=yuv420p`; ffmpeg's
//! implicit conversion would use BT.601 and shift gold/silver hues), and the
//! stream is tagged `bt709` primaries/transfer/matrix + tv range.
//!
//! Audio: [`Audio::Silent`] muxes `anullsrc` stereo 48 kHz AAC 128k,
//! [`Audio::File`] re-encodes the file to AAC 48 kHz stereo 192k, padded
//! with silence if it is short and cut to the video, [`Audio::None`] writes
//! no audio stream. The cut is sample-accurate (`-t` = frames/fps in a
//! second, stream-copy mux pass) because `-shortest` races in ffmpeg 7 and
//! can leave audio running ~0.4 s past the last frame.
//!
//! ffmpeg's stderr is captured; the last lines are included in every error.
//! `ARGENT_FFMPEG` overrides the ffmpeg binary.

use crate::Rgb8;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

#[derive(Clone, Debug)]
pub enum Audio {
    /// No audio stream.
    None,
    /// Silent AAC 48 kHz stereo track (Forge's QC requires an audio stream).
    Silent,
    /// Mux this file, trimmed to the video.
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct VideoSpec {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// x264 CRF (lower = better), default 18.
    pub crf: u8,
    /// Cap in kbit/s (Forge requires < 25 Mbps), default 16000.
    pub max_kbps: u32,
    pub audio: Audio,
}

impl Default for VideoSpec {
    fn default() -> Self {
        VideoSpec { width: 1080, height: 1920, fps: 30, crf: 18, max_kbps: 16_000, audio: Audio::Silent }
    }
}

/// The ffmpeg binary (`$ARGENT_FFMPEG` or `ffmpeg` on `PATH`).
pub fn ffmpeg_bin() -> OsString {
    std::env::var_os("ARGENT_FFMPEG").unwrap_or_else(|| "ffmpeg".into())
}

/// The ffprobe binary (`$ARGENT_FFPROBE` or `ffprobe` on `PATH`).
pub fn ffprobe_bin() -> OsString {
    std::env::var_os("ARGENT_FFPROBE").unwrap_or_else(|| "ffprobe".into())
}

/// True when `ffmpeg -version` runs.
pub fn ffmpeg_available() -> bool {
    Command::new(ffmpeg_bin())
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

const TAIL_LINES: usize = 40;

/// Collects the last lines a child writes to stderr.
struct StderrTail {
    lines: Arc<Mutex<VecDeque<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl StderrTail {
    fn spawn(stderr: std::process::ChildStderr) -> Self {
        let lines = Arc::new(Mutex::new(VecDeque::new()));
        let sink = Arc::clone(&lines);
        let thread = std::thread::Builder::new()
            .name("ffmpeg-stderr".into())
            .spawn(move || {
                let mut reader = BufReader::new(stderr);
                let mut buf = Vec::new();
                while reader.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
                    let line = String::from_utf8_lossy(&buf).trim_end().chars().take(400).collect::<String>();
                    buf.clear();
                    if line.is_empty() {
                        continue;
                    }
                    let mut q = sink.lock().unwrap_or_else(|p| p.into_inner());
                    if q.len() == TAIL_LINES {
                        q.pop_front();
                    }
                    q.push_back(line);
                }
            })
            .ok();
        StderrTail { lines, thread }
    }

    fn join(&mut self) -> String {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let q = self.lines.lock().unwrap_or_else(|p| p.into_inner());
        q.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}

/// An ffmpeg process fed raw frames on stdin.
struct FfmpegPipe {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    tail: StderrTail,
    what: String,
}

impl FfmpegPipe {
    fn spawn(args: &[OsString], what: String) -> anyhow::Result<Self> {
        let mut child = Command::new(ffmpeg_bin())
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("cannot start {:?} for {what}: {e}", ffmpeg_bin()))?;
        let stdin = child.stdin.take();
        let tail = StderrTail::spawn(child.stderr.take().expect("piped stderr"));
        Ok(FfmpegPipe { child: Some(child), stdin, tail, what })
    }

    /// Waits for ffmpeg to exit and builds an error with its stderr tail.
    fn fail(&mut self, context: String) -> anyhow::Error {
        drop(self.stdin.take());
        let status = self.child.take().and_then(|mut c| c.wait().ok());
        let tail = self.tail.join();
        let status = status.map(|s| s.to_string()).unwrap_or_else(|| "unknown status".into());
        anyhow::anyhow!("ffmpeg {} ({context}, {status}):\n{tail}", self.what)
    }

    fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        let res = match self.stdin.as_mut() {
            Some(s) => s.write_all(data),
            None => return Err(anyhow::anyhow!("ffmpeg {}: already finished", self.what)),
        };
        res.map_err(|e| self.fail(format!("write failed: {e}")))
    }

    fn finish(mut self) -> anyhow::Result<()> {
        drop(self.stdin.take());
        let status = self.child.take().map(|mut c| c.wait());
        let tail = self.tail.join();
        match status {
            Some(Ok(s)) if s.success() => Ok(()),
            Some(Ok(s)) => Err(anyhow::anyhow!("ffmpeg {} failed ({s}):\n{tail}", self.what)),
            Some(Err(e)) => Err(anyhow::anyhow!("ffmpeg {}: wait failed: {e}", self.what)),
            None => Err(anyhow::anyhow!("ffmpeg {}: already finished", self.what)),
        }
    }
}

impl Drop for FfmpegPipe {
    fn drop(&mut self) {
        // Closing stdin lets ffmpeg finalise whatever it has; never leave a zombie.
        drop(self.stdin.take());
        if let Some(mut c) = self.child.take() {
            let _ = c.wait();
        }
        let _ = self.tail.join();
    }
}

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn ensure_parent(path: &Path) -> anyhow::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}

fn rawvideo_input(width: u32, height: u32, fps: u32) -> Vec<OsString> {
    let size = format!("{width}x{height}");
    let fps = fps.to_string();
    os(&["-f", "rawvideo", "-pix_fmt", "rgb24", "-video_size", &size, "-framerate", &fps, "-i", "pipe:0"])
}

/// x264 pass: raw RGB24 on stdin → H.264 High yuv420p BT.709, no audio.
fn video_args(path: &Path, spec: &VideoSpec, faststart: bool) -> Vec<OsString> {
    let mut a = os(&["-hide_banner", "-nostats", "-loglevel", "warning", "-y"]);
    a.extend(rawvideo_input(spec.width, spec.height, spec.fps));
    a.extend(os(&[
        "-map",
        "0:v:0",
        "-vf",
        // ffmpeg 7 takes the encoder's colour tags from the frames, so set them in the graph too.
        "scale=out_color_matrix=bt709:out_range=tv:flags=bicubic+accurate_rnd+full_chroma_inp,format=yuv420p,\
         setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv",
        "-c:v",
        "libx264",
        "-preset",
        "slow",
        "-profile:v",
        "high",
        "-pix_fmt",
        "yuv420p",
        "-crf",
        &spec.crf.min(51).to_string(),
    ]));
    if spec.max_kbps > 0 {
        a.extend(os(&["-maxrate", &format!("{}k", spec.max_kbps), "-bufsize", &format!("{}k", spec.max_kbps * 2)]));
    }
    a.extend(os(&[
        "-g",
        &(spec.fps * 2).max(1).to_string(),
        "-x264-params",
        "aq-mode=3",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-color_range",
        "tv",
    ]));
    if faststart {
        a.extend(os(&["-movflags", "+faststart"]));
    }
    a.extend(os(&["-f", "mp4"]));
    a.push(path.into());
    a
}

/// Mux pass: copy the video, add AAC 48 kHz stereo audio cut to exactly
/// `duration` seconds (sample-accurate; `-shortest` races in ffmpeg 7).
fn mux_audio(video: &Path, out: &Path, audio: &Audio, duration: f64) -> anyhow::Result<()> {
    let mut cmd = Command::new(ffmpeg_bin());
    cmd.args(["-hide_banner", "-nostats", "-nostdin", "-loglevel", "warning", "-y", "-i"]).arg(video);
    match audio {
        Audio::Silent => {
            cmd.args(["-f", "lavfi", "-i", "anullsrc=channel_layout=stereo:sample_rate=48000"]);
            cmd.args(["-map", "0:v:0", "-map", "1:a:0", "-c:v", "copy", "-c:a", "aac", "-b:a", "128k"]);
        }
        Audio::File(p) => {
            cmd.arg("-i").arg(p);
            // apad: a short track is padded with silence, never cutting the video.
            cmd.args(["-map", "0:v:0", "-map", "1:a:0", "-c:v", "copy", "-af", "apad", "-c:a", "aac", "-b:a", "192k"]);
        }
        Audio::None => unreachable!("mux_audio without audio"),
    }
    cmd.args(["-ar", "48000", "-ac", "2", "-t", &format!("{duration:.6}"), "-movflags", "+faststart", "-f", "mp4"]);
    cmd.arg(out);
    let res = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output()
        .map_err(|e| anyhow::anyhow!("cannot start {:?}: {e}", ffmpeg_bin()))?;
    if !res.status.success() {
        let err = String::from_utf8_lossy(&res.stderr);
        let tail: Vec<&str> = err.lines().rev().take(TAIL_LINES).collect::<Vec<_>>().into_iter().rev().collect();
        anyhow::bail!("ffmpeg muxing audio into {} failed ({}):\n{}", out.display(), res.status, tail.join("\n"));
    }
    Ok(())
}

/// Streams frames to a Forge-ready MP4 (see module docs). With audio, the
/// video is encoded to a hidden temporary file beside `path` and the audio
/// is muxed in by [`finish`](Self::finish) at the exact video length.
pub struct VideoWriter {
    pipe: Option<FfmpegPipe>,
    width: u32,
    height: u32,
    fps: u32,
    frames: u64,
    out: PathBuf,
    /// Video-only intermediate when an audio track is muxed in afterwards.
    tmp: Option<PathBuf>,
    audio: Audio,
}

impl VideoWriter {
    pub fn create(path: &Path, spec: &VideoSpec) -> anyhow::Result<Self> {
        anyhow::ensure!(spec.width > 0 && spec.height > 0, "video size {}x{} is empty", spec.width, spec.height);
        anyhow::ensure!(
            spec.width.is_multiple_of(2) && spec.height.is_multiple_of(2),
            "yuv420p needs even dimensions, got {}x{}",
            spec.width,
            spec.height
        );
        anyhow::ensure!(spec.fps > 0, "fps must be > 0");
        if let Audio::File(p) = &spec.audio {
            anyhow::ensure!(p.is_file(), "audio file {} not found", p.display());
        }
        ensure_parent(path)?;
        let tmp = match spec.audio {
            Audio::None => None,
            _ => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "video".into());
                Some(path.with_file_name(format!(".{name}.{}.video.mp4", std::process::id())))
            }
        };
        let target = tmp.as_deref().unwrap_or(path);
        let pipe = FfmpegPipe::spawn(&video_args(target, spec, tmp.is_none()), format!("writing {}", path.display()))?;
        Ok(VideoWriter {
            pipe: Some(pipe),
            width: spec.width,
            height: spec.height,
            fps: spec.fps,
            frames: 0,
            out: path.to_path_buf(),
            tmp,
            audio: spec.audio.clone(),
        })
    }

    /// Writes one frame; blocks while ffmpeg is behind.
    pub fn push(&mut self, frame: &Rgb8) -> anyhow::Result<()> {
        anyhow::ensure!(
            frame.width == self.width && frame.height == self.height,
            "frame is {}x{} but the video is {}x{}",
            frame.width,
            frame.height,
            self.width,
            self.height
        );
        anyhow::ensure!(frame.data.len() == (frame.width * frame.height * 3) as usize, "frame buffer size mismatch");
        let pipe = self.pipe.as_mut().ok_or_else(|| anyhow::anyhow!("video writer already closed"))?;
        pipe.write(&frame.data)?;
        self.frames += 1;
        Ok(())
    }

    /// Frames pushed so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Closes the stream and waits for ffmpeg to finish the file (and, with
    /// audio, muxes the track in at exactly `frames / fps` seconds).
    pub fn finish(mut self) -> anyhow::Result<()> {
        anyhow::ensure!(self.frames > 0, "no frames were written");
        let pipe = self.pipe.take().ok_or_else(|| anyhow::anyhow!("video writer already closed"))?;
        pipe.finish()?;
        if let Some(tmp) = &self.tmp {
            mux_audio(tmp, &self.out, &self.audio, self.frames as f64 / self.fps as f64)?;
        }
        Ok(())
    }
}

impl Drop for VideoWriter {
    fn drop(&mut self) {
        drop(self.pipe.take());
        if let Some(t) = self.tmp.take() {
            let _ = std::fs::remove_file(t);
        }
    }
}

/// Encodes every frame of `frames` to `path`; returns the frame count.
pub fn encode_frames(path: &Path, spec: &VideoSpec, frames: impl IntoIterator<Item = Rgb8>) -> anyhow::Result<u64> {
    let mut w = VideoWriter::create(path, spec)?;
    for f in frames {
        w.push(&f)?;
    }
    let n = w.frames();
    w.finish()?;
    Ok(n)
}

/// Pipes frames (all the size of the first) into ffmpeg with `output_args`.
fn pipe_frames(
    path: &Path,
    fps: u32,
    frames: impl IntoIterator<Item = Rgb8>,
    output_args: impl FnOnce(u32, u32) -> Vec<OsString>,
) -> anyhow::Result<u64> {
    anyhow::ensure!(fps > 0, "fps must be > 0");
    let mut it = frames.into_iter();
    let first = it.next().ok_or_else(|| anyhow::anyhow!("no frames for {}", path.display()))?;
    let (w, h) = (first.width, first.height);
    ensure_parent(path)?;
    let mut args = os(&["-hide_banner", "-nostats", "-loglevel", "warning", "-y"]);
    args.extend(rawvideo_input(w, h, fps));
    args.extend(output_args(w, h));
    args.push(path.into());
    let mut pipe = FfmpegPipe::spawn(&args, format!("writing {}", path.display()))?;
    let mut n = 0u64;
    for f in std::iter::once(first).chain(it) {
        anyhow::ensure!(f.width == w && f.height == h, "frame {n} is {}x{}, expected {w}x{h}", f.width, f.height);
        pipe.write(&f.data)?;
        n += 1;
    }
    pipe.finish()?;
    Ok(n)
}

/// Looping GIF with a per-clip 256-colour palette (palettegen/paletteuse,
/// Sierra dithering), optionally downscaled to `max_width`.
pub fn write_gif(
    path: &Path,
    fps: u32,
    max_width: Option<u32>,
    frames: impl IntoIterator<Item = Rgb8>,
) -> anyhow::Result<u64> {
    pipe_frames(path, fps, frames, |w, _| {
        let scale = match max_width {
            Some(mw) if mw > 0 && mw < w => format!("scale={mw}:-1:flags=lanczos,"),
            _ => String::new(),
        };
        let vf = format!(
            "{scale}split[a][b];[a]palettegen=stats_mode=full:max_colors=256[p];[b][p]paletteuse=dither=sierra2_4a"
        );
        os(&["-vf", &vf, "-loop", "0", "-f", "gif"])
    })
}

/// Looping animated WebP (lossy, `quality` 0–100).
pub fn write_webp(path: &Path, fps: u32, quality: u8, frames: impl IntoIterator<Item = Rgb8>) -> anyhow::Result<u64> {
    pipe_frames(path, fps, frames, |_, _| {
        os(&[
            "-c:v",
            "libwebp_anim",
            "-lossless",
            "0",
            "-quality",
            &quality.min(100).to_string(),
            "-loop",
            "0",
            "-f",
            "webp",
        ])
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn out_dir() -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../out/post-selftest");
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("argent-post-tests/video");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// Moving colour field: never black, never frozen.
    pub(crate) fn test_frame(w: u32, h: u32, i: u32) -> Rgb8 {
        let mut f = Rgb8::new(w, h);
        let bar = (i * 23) % w;
        for y in 0..h {
            for x in 0..w {
                let o = ((y * w + x) * 3) as usize;
                let near = x.abs_diff(bar) < 40;
                f.data[o] = if near { 240 } else { (40 + (x * 150 / w) + i) as u8 };
                f.data[o + 1] = if near { 200 } else { (30 + (y * 120 / h)) as u8 };
                f.data[o + 2] = (60 + ((x + y + i * 7) % 90)) as u8;
            }
        }
        f
    }

    fn decode_first_frame(path: &Path, matrix: &str) -> Vec<u8> {
        let out = Command::new(ffmpeg_bin())
            .args(["-v", "error", "-i"])
            .arg(path)
            .args([
                "-frames:v",
                "1",
                "-vf",
                &format!("scale=in_color_matrix={matrix}:in_range=tv:flags=accurate_rnd+full_chroma_int,format=rgb24"),
                "-f",
                "rawvideo",
                "-",
            ])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        out.stdout
    }

    #[test]
    fn bt709_round_trip_keeps_metal_hues() {
        if !ffmpeg_available() {
            eprintln!("ffmpeg missing, skipping");
            return;
        }
        // Flat patches: gold, rose gold, silver, deep blue, near-black, white.
        let patches: [[u8; 3]; 6] =
            [[212, 175, 55], [183, 110, 121], [192, 192, 196], [30, 60, 160], [12, 10, 9], [250, 248, 240]];
        let (w, h) = (192u32, 64u32);
        let mut f = Rgb8::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let p = patches[(x / 32) as usize];
                let o = ((y * w + x) * 3) as usize;
                f.data[o..o + 3].copy_from_slice(&p);
            }
        }
        let path = scratch("bt709.mp4");
        let spec = VideoSpec { width: w, height: h, crf: 8, audio: Audio::None, ..VideoSpec::default() };
        encode_frames(&path, &spec, (0..10).map(|_| f.clone())).unwrap();
        let err = |rgb: &[u8]| {
            let mut worst = 0i32;
            for (i, p) in patches.iter().enumerate() {
                // Patch centre, away from chroma edges.
                let o = ((32 * w + i as u32 * 32 + 16) * 3) as usize;
                for c in 0..3 {
                    worst = worst.max((rgb[o + c] as i32 - p[c] as i32).abs());
                }
            }
            worst
        };
        let e709 = err(&decode_first_frame(&path, "bt709"));
        let e601 = err(&decode_first_frame(&path, "bt601"));
        eprintln!("patch error decoded as BT.709: {e709}, as BT.601: {e601}");
        assert!(e709 <= 3, "BT.709 round trip error {e709}");
        assert!(e601 > e709 + 4, "stream does not look BT.709-encoded ({e601} vs {e709})");
        let probe = Command::new(ffprobe_bin())
            .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
            .arg("stream=color_space,color_transfer,color_primaries,color_range,profile")
            .args(["-of", "default=nw=1"])
            .arg(&path)
            .output()
            .unwrap();
        let tags = String::from_utf8_lossy(&probe.stdout);
        for want in ["color_space=bt709", "color_transfer=bt709", "color_primaries=bt709", "color_range=tv", "High"] {
            assert!(tags.contains(want), "missing {want} in:\n{tags}");
        }
    }

    #[test]
    fn bad_input_reports_ffmpeg_stderr() {
        if !ffmpeg_available() {
            return;
        }
        let bogus = scratch("not-audio.wav");
        std::fs::write(&bogus, b"definitely not a wav file").unwrap();
        let spec = VideoSpec { width: 64, height: 64, audio: Audio::File(bogus), ..VideoSpec::default() };
        let res = encode_frames(&scratch("bad.mp4"), &spec, (0..30).map(|i| test_frame(64, 64, i)));
        let msg = format!("{:#}", res.unwrap_err());
        assert!(msg.contains("not-audio.wav") || msg.contains("Invalid data"), "{msg}");
        assert!(VideoWriter::create(&scratch("odd.mp4"), &VideoSpec { width: 63, ..VideoSpec::default() }).is_err());
    }

    #[test]
    fn gif_and_webp() {
        if !ffmpeg_available() {
            return;
        }
        let frames = || (0..12).map(|i| test_frame(160, 120, i));
        let gif = scratch("t.gif");
        assert_eq!(write_gif(&gif, 12, Some(80), frames()).unwrap(), 12);
        assert_eq!(&std::fs::read(&gif).unwrap()[..6], b"GIF89a");
        let webp = scratch("t.webp");
        assert_eq!(write_webp(&webp, 12, 80, frames()).unwrap(), 12);
        let bytes = std::fs::read(&webp).unwrap();
        assert!(&bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP");
    }
}
