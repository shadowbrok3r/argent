#!/usr/bin/env bash
# Install argent for this user: binary, the live studio service and the
# desktop catalog timer. Re-run after every build; idempotent.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -p argent
# Swap by rename so a running studio or render keeps its inode.
install -m755 target/release/argent "$HOME/.local/bin/.argent.new"
mv -f "$HOME/.local/bin/.argent.new" "$HOME/.local/bin/argent"
mkdir -p "$HOME/.config/systemd/user" "$HOME/argent/out/studio" "$HOME/argent/library"
cp deploy/argent-studio.service deploy/argent-desktop-catalog.service deploy/argent-desktop-catalog.timer "$HOME/.config/systemd/user/"
systemctl --user daemon-reload
systemctl --user enable --now argent-desktop-catalog.timer
systemctl --user enable argent-studio.service
systemctl --user restart argent-studio.service
sleep 1
systemctl --user --no-pager --lines=3 status argent-studio.service | head -6
echo "studio: http://100.96.121.37:8790"
