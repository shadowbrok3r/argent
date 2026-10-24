#!/usr/bin/env bash
# Integration build: every crate's *working-tree* state (agents' in-progress
# edits included), with the studio server stubbed out so a half-edited
# serve/ never blocks testing the renderer.
set -euo pipefail
rsync -a --delete --exclude target ~/argent/crates/ ~/argent-int/crates/
cp ~/argent/Cargo.toml ~/argent-int/Cargo.toml
cp ~/argent/Cargo.lock ~/argent-int/Cargo.lock 2>/dev/null || true
rm -rf ~/argent-int/crates/argent/src/serve
cat > ~/argent-int/crates/argent/src/serve.rs <<'RS'
use anyhow::Result;
use std::path::PathBuf;
pub fn serve(_h: &str, _p: u16, _l: Vec<PathBuf>, _o: PathBuf) -> Result<()> { anyhow::bail!("serve stubbed in integration builds") }
RS
cd ~/argent-int && CARGO_TARGET_DIR=$HOME/argent/target-int nice -n 5 cargo "$@"
