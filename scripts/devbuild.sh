#!/usr/bin/env bash
# Build my crates (argent-core, argent) against the last *committed* state of
# the other crates, so in-progress edits there don't block testing.
set -euo pipefail
rsync -a --delete ~/argent/crates/argent-core/ ~/argent-dev/crates/argent-core/
rsync -a --delete --exclude src/serve.rs --exclude src/serve/ ~/argent/crates/argent/ ~/argent-dev/crates/argent/
cp ~/argent/Cargo.toml ~/argent-dev/Cargo.toml
cd ~/argent-dev && CARGO_TARGET_DIR=$HOME/argent/target-dev nice -n 5 cargo "$@"
