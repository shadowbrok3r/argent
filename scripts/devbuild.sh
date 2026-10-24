#!/usr/bin/env bash
# Build my crates (argent-core, argent) against the last *committed* state of
# the other crates, so in-progress edits there don't block testing.
set -euo pipefail
rsync -a --delete ~/argent/crates/argent-core/ ~/argent-dev/crates/argent-core/
rsync -a --delete --exclude src/serve.rs --exclude src/serve/ ~/argent/crates/argent/ ~/argent-dev/crates/argent/
# Workspace members that exist in the dev tree only (new crates may be mid-creation).
python3 - <<'PY'
import os, re
src = open(os.path.expanduser('~/argent/Cargo.toml')).read()
def keep(m):
    path = m.group(1)
    return m.group(0) if os.path.isdir(os.path.expanduser('~/argent-dev/' + path)) else ''
out = re.sub(r'^\s*"(crates/[^"]+)",\n', keep, src, flags=re.M)
open(os.path.expanduser('~/argent-dev/Cargo.toml'), 'w').write(out)
PY
cd ~/argent-dev && CARGO_TARGET_DIR=$HOME/argent/target-dev nice -n 5 cargo "$@"
