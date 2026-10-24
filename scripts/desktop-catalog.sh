#!/usr/bin/env bash
# Once the studio desktop is up, pull its jewelry library (read-only) and
# render it: a listing catalog for every piece, plus Forge-spec turntable
# reels for the designs waiting on a turntable in Forge's idea list.
# Nothing is uploaded or posted anywhere; results stay on this box.
#
# Runs from argent-desktop-catalog.timer every 10 minutes; exits at once
# when the desktop is offline or today's run already finished.
set -euo pipefail

HOST=${ARGENT_DESKTOP:-shadowbroker@100.116.134.128}
SRC=${ARGENT_DESKTOP_LIBRARY:-jewelry-scan/}
LIB=$HOME/argent/library/jewelry-scan
OUT=$HOME/argent/out/catalog
STATE=$HOME/.local/state/argent
ARGENT=${ARGENT_BIN:-$HOME/.local/bin/argent}
STAMP=$STATE/desktop-catalog-$(date -u +%F).done
LOCK=$STATE/desktop-catalog.lock
mkdir -p "$STATE" "$LIB" "$OUT"

[ -e "$STAMP" ] && exit 0
exec 9>"$LOCK"
flock -n 9 || { echo "another run is in progress"; exit 0; }

if ! ssh -o BatchMode=yes -o ConnectTimeout=6 "$HOST" true 2>/dev/null; then
  echo "desktop offline; will retry"
  exit 0
fi

echo "== $(date -u +%T) pulling $HOST:$SRC (read-only)"
# Mesh files only, nothing over 600 MB, 20 MB/s cap so the desktop stays responsive.
rsync -a --prune-empty-dirs --bwlimit=20000 --max-size=600m \
  --include='*/' --include='*.stl' --include='*.STL' --include='*.obj' --include='*.OBJ' \
  --include='*.3mf' --include='*.ply' --exclude='*' \
  -e 'ssh -o BatchMode=yes -o ConnectTimeout=10' "$HOST:$SRC" "$LIB/"
echo "   $(find "$LIB" -type f | wc -l) mesh files, $(du -sh "$LIB" | cut -f1)"

echo "== $(date -u +%T) catalog stills"
"$ARGENT" --priority idle batch "$LIB" -o "$OUT" --no-turntable \
  --metals sterling,gold14k,bronze --views hero,front,side --spp 96 || echo "batch finished with errors"

# Forge's pending turntable ideas (design → metal, hook), matched by file name.
reel() {
  local pattern=$1 metal=$2 hook=$3 name=$4
  local f
  f=$(find "$LIB" -type f \( -iname "*${pattern}*.stl" -o -iname "*${pattern}*.obj" -o -iname "*${pattern}*.3mf" \) | head -1)
  if [ -z "$f" ]; then
    echo "   no mesh matching '$pattern' for $name"
    return
  fi
  echo "== $(date -u +%T) reel $name from $f"
  "$ARGENT" --priority idle turntable "$f" --metal "$metal" --studio noir --seconds 6 \
    --hook "$hook" -o "$OUT/forge-reels/${name}-turntable-${metal}.mp4" --qc \
    | tee "$OUT/forge-reels/${name}-qc.txt" || echo "   reel failed for $name"
}
mkdir -p "$OUT/forge-reels"
reel thanatos gold14k "Death had a name: Thanatos" thanatos_ring
reel pazuzu   gold14k "King of all wind demons" pazuzu
reel anubis   sterling "Would your heart pass judgment?" anubis

touch "$STAMP"
echo "== $(date -u +%T) done → $OUT/index.html"
