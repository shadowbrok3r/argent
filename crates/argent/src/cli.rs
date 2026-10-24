use crate::jobs::{self, PieceArgs, ShotArgs};
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "argent", version, about = "Photoreal jewelry renders from STL — stills, turntable reels, listing sheets.")]
struct Cli {
    /// CPU priority: normal, low (nice 10, default) or idle (nice 19).
    #[arg(long, global = true, default_value = "low")]
    priority: String,
    /// Render threads (default: all cores).
    #[arg(long, global = true)]
    threads: Option<usize>,
    /// Print `[IMAGE:path]` / `[VIDEO:path]` as the last line (ZeroClaw delivery markers).
    #[arg(long, global = true)]
    marker: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Mesh statistics, detected pose, weight per alloy.
    Info {
        #[command(flatten)]
        piece: PieceArgs,
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
    /// One still image.
    Render {
        #[command(flatten)]
        piece: PieceArgs,
        #[command(flatten)]
        shot: ShotArgs,
        /// Output file (.png, .jpg or .exr).
        #[arg(short, long, default_value = "render.png")]
        out: PathBuf,
        /// Turn the piece this many degrees about the vertical before shooting.
        #[arg(long, default_value_t = 0.0)]
        turn: f32,
    },
    /// A looping turntable reel (1080×1920, 30 fps, h264 + AAC: Forge's reel spec).
    Turntable {
        #[command(flatten)]
        piece: PieceArgs,
        #[command(flatten)]
        shot: ShotArgs,
        #[arg(short, long, default_value = "turntable.mp4")]
        out: PathBuf,
        #[arg(long, default_value_t = 6.0)]
        seconds: f32,
        #[arg(long, default_value_t = 30)]
        fps: u32,
        /// Burned-in hook over the first two seconds (≤7 words reads best).
        #[arg(long)]
        hook: Option<String>,
        /// Soundtrack to mux (default: silent AAC track).
        #[arg(long)]
        audio: Option<PathBuf>,
        /// Run Forge's reel QC on the result.
        #[arg(long)]
        qc: bool,
        /// Also keep the PNG frames in this directory.
        #[arg(long)]
        frames: Option<PathBuf>,
    },
    /// Listing set: every metal × view as stills plus a captioned contact sheet.
    Sheet {
        #[command(flatten)]
        piece: PieceArgs,
        #[command(flatten)]
        shot: ShotArgs,
        #[arg(short, long, default_value = "sheet")]
        out: PathBuf,
        #[arg(long, default_value = "sterling,gold14k,rose14k,bronze", value_delimiter = ',')]
        metals: Vec<String>,
        #[arg(long, default_value = "hero,front,side,top", value_delimiter = ',')]
        views: Vec<String>,
        /// Title on the contact sheet (default: the file name).
        #[arg(long)]
        title: Option<String>,
    },
    /// Every mesh in a directory: listing sheet + turntable each, plus an index.html.
    Batch {
        /// Directory to scan recursively for STL/OBJ/PLY/3MF.
        dir: PathBuf,
        #[arg(short, long, default_value = "catalog")]
        out: PathBuf,
        #[command(flatten)]
        shot: ShotArgs,
        #[arg(long, default_value = "sterling,gold14k,bronze", value_delimiter = ',')]
        metals: Vec<String>,
        #[arg(long, default_value = "hero,front,side", value_delimiter = ',')]
        views: Vec<String>,
        /// Skip turntables (stills only).
        #[arg(long)]
        no_turntable: bool,
        #[arg(long, default_value_t = 6.0)]
        seconds: f32,
    },
    /// Live studio in the browser (progressive renders, phone-friendly) + JSON API.
    Serve {
        #[arg(long, default_value = "0.0.0.0")]
        host: String,
        #[arg(long, default_value_t = 8790)]
        port: u16,
        /// Directories the studio may open meshes from.
        #[arg(long, value_delimiter = ',')]
        library: Vec<PathBuf>,
        /// Where renders requested through the API are written.
        #[arg(long, default_value = "argent-renders")]
        out: PathBuf,
    },
}

fn set_priority(p: &str) {
    let nice = match p {
        "normal" => 0,
        "idle" => 19,
        _ => 10,
    };
    if nice > 0 {
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, nice);
        }
    }
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    set_priority(&cli.priority);
    if cli.marker {
        crate::jobs::MARKERS.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(n) = cli.threads {
        rayon::ThreadPoolBuilder::new().num_threads(n).build_global().ok();
    }
    match cli.cmd {
        Cmd::Info { piece, json } => jobs::info(&piece, json),
        Cmd::Render { piece, shot, out, turn } => jobs::render(&piece, &shot, &out, turn),
        Cmd::Turntable { piece, shot, out, seconds, fps, hook, audio, qc, frames } => {
            jobs::turntable(&piece, &shot, &out, seconds, fps, hook.as_deref(), audio, qc, frames)
        }
        Cmd::Sheet { piece, shot, out, metals, views, title } => jobs::sheet(&piece, &shot, &out, &metals, &views, title).map(|_| ()),
        Cmd::Batch { dir, out, shot, metals, views, no_turntable, seconds } => {
            jobs::batch(&dir, &out, &shot, &metals, &views, !no_turntable, seconds)
        }
        Cmd::Serve { host, port, library, out } => crate::serve::serve(&host, port, library, out),
    }
}
