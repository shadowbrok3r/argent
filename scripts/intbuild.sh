#!/usr/bin/env bash
# Integration build: every crate's *working-tree* state (agents' in-progress
# edits included), with the studio server stubbed out so a half-edited
# serve/ never blocks testing the renderer.
set -euo pipefail
# No -t: files whose content changed get a fresh mtime, so cargo always
# notices them (preserved mtimes can be older than the last build's artifacts).
rsync -rlpgoD --checksum --delete --exclude target ~/argent/crates/ ~/argent-int/crates/
cp ~/argent/Cargo.toml ~/argent-int/Cargo.toml
cp ~/argent/Cargo.lock ~/argent-int/Cargo.lock 2>/dev/null || true
# Crates named in $PIN (space-separated) come from the last commit instead,
# for when an agent is mid-edit there.
for c in ${PIN:-}; do
  rm -rf ~/argent-int/crates/$c
  git -C ~/argent archive HEAD crates/$c | tar -x -m -C ~/argent-int
done
rm -rf ~/argent-int/crates/argent/src/serve
cat > ~/argent-int/crates/argent/src/serve.rs <<'RS'
use anyhow::Result;
use std::path::PathBuf;
pub fn serve(_h: &str, _p: u16, _l: Vec<PathBuf>, _o: PathBuf) -> Result<()> { anyhow::bail!("serve stubbed in integration builds") }
RS
cd ~/argent-int && CARGO_TARGET_DIR=$HOME/argent/target-int nice -n 5 cargo "$@"
