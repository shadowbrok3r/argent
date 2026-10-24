//! Forge reel QC, reproduced locally with ffprobe/ffmpeg.
//!
//! [`forge_reel`] runs the nine format checks, with the same names, detail
//! formats and thresholds as the QC JSON Forge stores on each item:
//!
//! | check | ok when | detail |
//! |---|---|---|
//! | `video stream` | first video stream is h264 | codec, e.g. `h264` |
//! | `resolution 1080x1920` | exactly 1080×1920 | `1080x1920` |
//! | `30 fps` | avg frame rate within 0.01 of 30 | `30.00` |
//! | `h264 yuv420p` | h264 and yuv420p | `h264 yuv420p` |
//! | `aac 48 kHz audio` | first audio stream AAC at 48000 Hz | `aac 48000` |
//! | `duration 3-90 s` | 3 ≤ container duration ≤ 90 s | `8.0 s` |
//! | `bitrate under 25 Mbps` | container bit rate < 25·10⁶ bit/s | `14.1 Mbps` |
//! | `file under 300 MB` | size < 300 MiB (Forge's "MB" is bytes/2²⁰) | `13.4 MB` |
//! | `no black stretches over 0.5 s` | `blackdetect=d=0.5:pix_th=0.10` finds none | `none` or the stretches |
//!
//! [`forge_reel_full`] adds the three checks Forge's QC JSON also carries
//! (needs real audio, so a silent track fails the loudness check):
//! `no frozen stretches over 2 s` (`freezedetect=n=-60dB:d=2`),
//! `loudness -14 LUFS` (integrated, EBU R128, within ±1 LU — tolerance
//! assumed) and `true peak under -1.0 dBTP`.
//!
//! **Near-black jewelry backgrounds:** blackdetect counts a frame as black
//! when ≥ 98 % of its pixels have luma under 10 % of the video range (about
//! sRGB 24/255). A small piece on a near-black studio background — or a
//! cold open where only a glint is lit — can therefore trip it; the detail
//! lists the detected stretches (`0.00-1.20 s, …`) so the cause is visible.
//!
//! `ARGENT_FFPROBE` / `ARGENT_FFMPEG` override the binaries.

use crate::video::{ffmpeg_bin, ffprobe_bin};
use std::fmt;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Check {
    pub check: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct QcReport {
    pub passed: bool,
    pub checks: Vec<Check>,
}

impl QcReport {
    fn from_checks(checks: Vec<Check>) -> Self {
        QcReport { passed: checks.iter().all(|c| c.ok), checks }
    }

    /// The check with this exact name.
    pub fn get(&self, name: &str) -> Option<&Check> {
        self.checks.iter().find(|c| c.check == name)
    }

    /// Checks that did not pass.
    pub fn failures(&self) -> impl Iterator<Item = &Check> {
        self.checks.iter().filter(|c| !c.ok)
    }
}

impl fmt::Display for QcReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "QC {}", if self.passed { "passed" } else { "FAILED" })?;
        for c in &self.checks {
            writeln!(f, "  [{}] {}: {}", if c.ok { "ok" } else { "FAIL" }, c.check, c.detail)?;
        }
        Ok(())
    }
}

/// Forge's black-frame filter.
pub const BLACKDETECT: &str = "blackdetect=d=0.5:pix_th=0.10";
/// Forge's frozen-frame filter (`no frozen stretches over 2 s`).
pub const FREEZEDETECT: &str = "freezedetect=n=-60dB:d=2";

/// Forge's nine format checks (see module docs).
pub fn forge_reel(path: &Path) -> anyhow::Result<QcReport> {
    let probe = probe(path)?;
    let scan = scan(path, &probe, false)?;
    Ok(QcReport::from_checks(basic_checks(&probe, &scan)))
}

/// All twelve checks of Forge's QC JSON: [`forge_reel`] plus frozen frames,
/// loudness and true peak.
pub fn forge_reel_full(path: &Path) -> anyhow::Result<QcReport> {
    let probe = probe(path)?;
    let scan = scan(path, &probe, true)?;
    let mut checks = basic_checks(&probe, &scan);
    checks.extend(extra_checks(&probe, &scan));
    Ok(QcReport::from_checks(checks))
}

// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Stream {
    codec: String,
    width: u32,
    height: u32,
    pix_fmt: String,
    fps: Option<f64>,
    sample_rate: Option<u32>,
}

#[derive(Debug, Default)]
struct Probe {
    video: Option<Stream>,
    audio: Option<Stream>,
    duration: Option<f64>,
    bit_rate: Option<f64>,
    size: Option<u64>,
}

fn num(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::String(s) => s.trim().parse().ok(),
        serde_json::Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

fn rate(v: &serde_json::Value) -> Option<f64> {
    let s = v.as_str()?;
    let (n, d) = s.split_once('/').unwrap_or((s, "1"));
    let (n, d): (f64, f64) = (n.trim().parse().ok()?, d.trim().parse().ok()?);
    (d != 0.0 && n > 0.0).then(|| n / d)
}

fn probe(path: &Path) -> anyhow::Result<Probe> {
    anyhow::ensure!(path.is_file(), "{} not found", path.display());
    let out = Command::new(ffprobe_bin())
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| anyhow::anyhow!("cannot run {:?}: {e}", ffprobe_bin()))?;
    anyhow::ensure!(
        out.status.success(),
        "ffprobe failed on {}: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let mut p = Probe::default();
    for s in v["streams"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let kind = s["codec_type"].as_str().unwrap_or("");
        let attached = s["disposition"]["attached_pic"].as_i64() == Some(1);
        let stream = || Stream {
            codec: s["codec_name"].as_str().unwrap_or("unknown").to_owned(),
            width: s["width"].as_u64().unwrap_or(0) as u32,
            height: s["height"].as_u64().unwrap_or(0) as u32,
            pix_fmt: s["pix_fmt"].as_str().unwrap_or("unknown").to_owned(),
            fps: rate(&s["avg_frame_rate"]).or_else(|| rate(&s["r_frame_rate"])),
            sample_rate: num(&s["sample_rate"]).map(|r| r as u32),
        };
        if kind == "video" && !attached && p.video.is_none() {
            p.video = Some(stream());
        } else if kind == "audio" && p.audio.is_none() {
            p.audio = Some(stream());
        }
    }
    let fmt = &v["format"];
    p.duration = num(&fmt["duration"]);
    p.size = num(&fmt["size"]).map(|s| s as u64).or_else(|| std::fs::metadata(path).ok().map(|m| m.len()));
    p.bit_rate = num(&fmt["bit_rate"]).or_else(|| match (p.size, p.duration) {
        (Some(s), Some(d)) if d > 0.0 => Some(s as f64 * 8.0 / d),
        _ => None,
    });
    Ok(p)
}

#[derive(Debug, Default)]
struct Scan {
    black: Vec<(f64, f64)>,
    frozen: Vec<(f64, f64)>,
    lufs: Option<f64>,
    true_peak: Option<f64>,
}

/// The number after `key` on `line` (`-inf` allowed).
fn value_after(line: &str, key: &str) -> Option<f64> {
    let rest = line[line.find(key)? + key.len()..].trim_start();
    let tok = rest.split_whitespace().next()?;
    match tok {
        "-inf" => Some(f64::NEG_INFINITY),
        "inf" => Some(f64::INFINITY),
        t => t.parse().ok(),
    }
}

fn parse_scan(stderr: &str, duration: Option<f64>) -> Scan {
    let mut s = Scan::default();
    let mut freeze_start: Option<f64> = None;
    for line in stderr.lines() {
        if line.contains("black_start:") {
            if let (Some(a), Some(b)) = (value_after(line, "black_start:"), value_after(line, "black_end:")) {
                s.black.push((a, b));
            }
        } else if line.contains("freezedetect.freeze_start:") {
            freeze_start = value_after(line, "freeze_start:");
        } else if line.contains("freezedetect.freeze_end:")
            && let (Some(a), Some(b)) = (freeze_start.take(), value_after(line, "freeze_end:"))
        {
            s.frozen.push((a, b));
        }
    }
    // A freeze that runs to the end of the file has no end line.
    if let Some(a) = freeze_start {
        s.frozen.push((a, duration.unwrap_or(a)));
    }
    // ebur128 prints per-frame lines with "I:" too; only the final summary counts.
    if let Some(i) = stderr.rfind("Summary:") {
        let summary = &stderr[i..];
        s.lufs = summary.lines().find(|l| l.trim_start().starts_with("I:")).and_then(|l| value_after(l, "I:"));
        if let Some(j) = summary.find("True peak:") {
            s.true_peak =
                summary[j..].lines().find(|l| l.trim_start().starts_with("Peak:")).and_then(|l| value_after(l, "Peak:"));
        }
    }
    s
}

fn scan(path: &Path, probe: &Probe, full: bool) -> anyhow::Result<Scan> {
    let video = probe.video.is_some();
    let audio = full && probe.audio.is_some();
    if !video && !audio {
        return Ok(Scan::default());
    }
    let mut cmd = Command::new(ffmpeg_bin());
    cmd.args(["-hide_banner", "-nostats", "-nostdin", "-i"]).arg(path);
    if video {
        let vf = if full { format!("{BLACKDETECT},{FREEZEDETECT}") } else { BLACKDETECT.to_owned() };
        cmd.args(["-map", "0:v:0", "-vf", &vf]);
    }
    if audio {
        cmd.args(["-map", "0:a:0", "-af", "ebur128=peak=true:framelog=quiet"]);
    }
    cmd.args(["-f", "null", "-"]);
    let out = cmd.stdin(Stdio::null()).output().map_err(|e| anyhow::anyhow!("cannot run {:?}: {e}", ffmpeg_bin()))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        let tail: Vec<&str> = stderr.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect();
        anyhow::bail!("ffmpeg QC scan of {} failed ({}):\n{}", path.display(), out.status, tail.join("\n"));
    }
    Ok(parse_scan(&stderr, probe.duration))
}

fn check(name: &str, ok: bool, detail: impl Into<String>) -> Check {
    Check { check: name.to_owned(), ok, detail: detail.into() }
}

fn stretches(v: &[(f64, f64)]) -> String {
    if v.is_empty() {
        "none".into()
    } else {
        v.iter().map(|(a, b)| format!("{a:.2}-{b:.2} s")).collect::<Vec<_>>().join(", ")
    }
}

fn basic_checks(p: &Probe, s: &Scan) -> Vec<Check> {
    let mut c = Vec::with_capacity(9);
    match &p.video {
        Some(v) => {
            c.push(check("video stream", v.codec == "h264", v.codec.clone()));
            c.push(check(
                "resolution 1080x1920",
                v.width == 1080 && v.height == 1920,
                format!("{}x{}", v.width, v.height),
            ));
            match v.fps {
                Some(f) => c.push(check("30 fps", (f - 30.0).abs() < 0.01, format!("{f:.2}"))),
                None => c.push(check("30 fps", false, "unknown")),
            }
            c.push(check(
                "h264 yuv420p",
                v.codec == "h264" && v.pix_fmt == "yuv420p",
                format!("{} {}", v.codec, v.pix_fmt),
            ));
        }
        None => {
            for name in ["video stream", "resolution 1080x1920", "30 fps", "h264 yuv420p"] {
                c.push(check(name, false, "none"));
            }
        }
    }
    match &p.audio {
        Some(a) => {
            let sr = a.sample_rate.unwrap_or(0);
            c.push(check("aac 48 kHz audio", a.codec == "aac" && sr == 48_000, format!("{} {sr}", a.codec)));
        }
        None => c.push(check("aac 48 kHz audio", false, "none")),
    }
    match p.duration {
        Some(d) => c.push(check("duration 3-90 s", (3.0..=90.0).contains(&d), format!("{d:.1} s"))),
        None => c.push(check("duration 3-90 s", false, "unknown")),
    }
    match p.bit_rate {
        Some(b) => c.push(check("bitrate under 25 Mbps", b < 25e6, format!("{:.1} Mbps", b / 1e6))),
        None => c.push(check("bitrate under 25 Mbps", false, "unknown")),
    }
    match p.size {
        Some(b) => {
            let mib = b as f64 / 1_048_576.0;
            c.push(check("file under 300 MB", mib < 300.0, format!("{mib:.1} MB")));
        }
        None => c.push(check("file under 300 MB", false, "unknown")),
    }
    if p.video.is_some() {
        c.push(check("no black stretches over 0.5 s", s.black.is_empty(), stretches(&s.black)));
    } else {
        c.push(check("no black stretches over 0.5 s", false, "no video"));
    }
    c
}

fn extra_checks(p: &Probe, s: &Scan) -> Vec<Check> {
    let mut c = Vec::with_capacity(3);
    if p.video.is_some() {
        c.push(check("no frozen stretches over 2 s", s.frozen.is_empty(), stretches(&s.frozen)));
    } else {
        c.push(check("no frozen stretches over 2 s", false, "no video"));
    }
    let db = |v: f64, unit: &str| if v.is_finite() { format!("{v:.1} {unit}") } else { format!("{v} {unit}") };
    match (p.audio.is_some(), s.lufs) {
        (true, Some(l)) => c.push(check("loudness -14 LUFS", (l + 14.0).abs() <= 1.0, db(l, "LUFS"))),
        (true, None) => c.push(check("loudness -14 LUFS", false, "unknown")),
        (false, _) => c.push(check("loudness -14 LUFS", false, "no audio")),
    }
    match (p.audio.is_some(), s.true_peak) {
        (true, Some(tp)) => c.push(check("true peak under -1.0 dBTP", tp < -1.0, db(tp, "dBTP"))),
        (true, None) => c.push(check("true peak under -1.0 dBTP", false, "unknown")),
        (false, _) => c.push(check("true peak under -1.0 dBTP", false, "no audio")),
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::tests::{out_dir, scratch, test_frame};
    use crate::video::{Audio, VideoSpec, encode_frames, ffmpeg_available};

    const NAMES: [&str; 9] = [
        "video stream",
        "resolution 1080x1920",
        "30 fps",
        "h264 yuv420p",
        "aac 48 kHz audio",
        "duration 3-90 s",
        "bitrate under 25 Mbps",
        "file under 300 MB",
        "no black stretches over 0.5 s",
    ];

    #[test]
    fn parses_ffmpeg_logs() {
        let log = "\
[blackdetect @ 0x55d1] black_start:0 black_end:1.2 black_duration:1.2
[freezedetect @ 0x55d2] lavfi.freezedetect.freeze_start: 2.5
[freezedetect @ 0x55d2] lavfi.freezedetect.freeze_duration: 2.1
[freezedetect @ 0x55d2] lavfi.freezedetect.freeze_end: 4.6
[freezedetect @ 0x55d2] lavfi.freezedetect.freeze_start: 7
[Parsed_ebur128_0 @ 0x55d3] t: 0.1 TARGET:-23 LUFS M: -20.0 S: -20.0 I: -19.0 LUFS
[Parsed_ebur128_0 @ 0x55d3] Summary:

  Integrated loudness:
    I:         -14.1 LUFS
    Threshold: -24.4 LUFS

  True peak:
    Peak:       -2.2 dBFS
";
        let s = parse_scan(log, Some(8.0));
        assert_eq!(s.black, vec![(0.0, 1.2)]);
        assert_eq!(s.frozen, vec![(2.5, 4.6), (7.0, 8.0)]);
        assert_eq!(s.lufs, Some(-14.1));
        assert_eq!(s.true_peak, Some(-2.2));
        assert_eq!(stretches(&s.black), "0.00-1.20 s");
        let silent = parse_scan("Summary:\n  I: -70.0 LUFS\n  True peak:\n    Peak: -inf dBFS\n", None);
        assert_eq!(silent.true_peak, Some(f64::NEG_INFINITY));
    }

    /// (video, audio) stream durations in seconds.
    fn stream_durations(path: &Path) -> (f64, f64) {
        let out = Command::new(ffprobe_bin())
            .args(["-v", "error", "-show_entries", "stream=codec_type,duration", "-of", "csv=p=0"])
            .arg(path)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let get = |kind: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(kind).and_then(|r| r.trim_start_matches(',').parse::<f64>().ok()))
                .unwrap_or(f64::NAN)
        };
        (get("video"), get("audio"))
    }

    /// The brief's round trip: 90 frames of 1080×1920 at 30 fps, silent AAC.
    #[test]
    fn reel_round_trip_passes_forge_qc() {
        if !ffmpeg_available() {
            eprintln!("ffmpeg missing, skipping");
            return;
        }
        let path = out_dir().join("selftest-reel.mp4");
        let t = std::time::Instant::now();
        let n = encode_frames(&path, &VideoSpec::default(), (0..90).map(|i| test_frame(1080, 1920, i))).unwrap();
        eprintln!("encoded {n} frames in {:.1} s", t.elapsed().as_secs_f64());
        let report = forge_reel(&path).unwrap();
        eprint!("{report}");
        let names: Vec<&str> = report.checks.iter().map(|c| c.check.as_str()).collect();
        assert_eq!(names, NAMES);
        assert!(report.passed, "{report}");
        assert_eq!(report.get("30 fps").unwrap().detail, "30.00");
        assert_eq!(report.get("aac 48 kHz audio").unwrap().detail, "aac 48000");
        assert_eq!(report.get("video stream").unwrap().detail, "h264");
        assert_eq!(report.get("h264 yuv420p").unwrap().detail, "h264 yuv420p");
        assert_eq!(report.get("no black stretches over 0.5 s").unwrap().detail, "none");
        assert_eq!(report.get("duration 3-90 s").unwrap().detail, "3.0 s");
        let (v, a) = stream_durations(&path);
        assert!((v - 3.0).abs() < 1e-3 && (a - v).abs() < 0.025, "video {v} s, audio {a} s");
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.starts_with("{\"passed\":true,\"checks\":[{\"check\":\"video stream\""));
    }

    #[test]
    fn failures_are_reported() {
        if !ffmpeg_available() {
            return;
        }
        // Wrong size, no audio, 2 s long and black for the first second.
        let path = scratch("bad-reel.mp4");
        let spec = VideoSpec { width: 640, height: 360, audio: Audio::None, ..VideoSpec::default() };
        let frames = (0..60).map(|i| if i < 30 { crate::Rgb8::new(640, 360) } else { test_frame(640, 360, i) });
        encode_frames(&path, &spec, frames).unwrap();
        let r = forge_reel(&path).unwrap();
        assert!(!r.passed);
        let failed: Vec<&str> = r.failures().map(|c| c.check.as_str()).collect();
        assert_eq!(failed, ["resolution 1080x1920", "aac 48 kHz audio", "duration 3-90 s", "no black stretches over 0.5 s"]);
        assert_eq!(r.get("aac 48 kHz audio").unwrap().detail, "none");
        let black = &r.get("no black stretches over 0.5 s").unwrap().detail;
        assert!(black.starts_with("0.00-1.0"), "{black}");
        assert!(forge_reel(&scratch("missing.mp4")).is_err());
    }

    /// Full twelve checks with a real -14 LUFS tone muxed through `Audio::File`.
    #[test]
    fn full_qc_with_music_file() {
        if !ffmpeg_available() {
            return;
        }
        // 1 kHz sine at −14 dBFS peak on both channels → −14 LUFS, −14 dBTP.
        let tone = scratch("tone.wav");
        let st = Command::new(ffmpeg_bin())
            // Two explicit channels (a mono→stereo upmix would drop each by 3 dB).
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("aevalsrc=0.19953*sin(2*PI*1000*t)|0.19953*sin(2*PI*1000*t):s=48000:d=5")
            .arg(&tone)
            .status()
            .unwrap();
        assert!(st.success());
        let path = scratch("music-reel.mp4");
        let spec = VideoSpec { audio: Audio::File(tone), ..VideoSpec::default() };
        encode_frames(&path, &spec, (0..105).map(|i| test_frame(1080, 1920, i))).unwrap();
        let r = forge_reel_full(&path).unwrap();
        eprint!("{r}");
        assert_eq!(r.checks.len(), 12);
        assert!(r.passed, "{r}");
        let dur = &r.get("duration 3-90 s").unwrap().detail;
        assert_eq!(dur, "3.5 s", "audio (5 s) must be trimmed to the video (3.5 s)");
        let (v, a) = stream_durations(&path);
        assert!((v - 3.5).abs() < 1e-3 && (a - v).abs() < 0.025, "video {v} s, audio {a} s");
        // A silent track fails Forge's loudness check, as it would on Forge.
        let silent = scratch("silent-reel.mp4");
        encode_frames(&silent, &VideoSpec::default(), (0..90).map(|i| test_frame(1080, 1920, i))).unwrap();
        let r = forge_reel_full(&silent).unwrap();
        let failed: Vec<&str> = r.failures().map(|c| c.check.as_str()).collect();
        assert_eq!(failed, ["loudness -14 LUFS"], "{r}");
    }
}
