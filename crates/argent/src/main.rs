//! `argent` — photoreal jewelry renders from STL.
//!
//! ```text
//! argent info ring.stl
//! argent render ring.stl --metal gold14k --studio noir --view hero -o hero.png
//! argent turntable ring.stl --hook "Death had a name: Thanatos" -o reel.mp4 --qc
//! argent sheet ring.stl --metals sterling,gold14k,rose14k,bronze -o listing/
//! argent batch ~/jewelry-scan/RING -o catalog/
//! argent serve --port 8790
//! ```

mod cli;
mod jobs;
mod serve;

fn main() {
    if let Err(e) = cli::run() {
        eprintln!("argent: {e:#}");
        std::process::exit(1);
    }
}
