#!/usr/bin/env bash
# Render a showcase of argent on Logan's RingDesigner designs: hero stills in
# several metals/studios, a listing sheet, an opal variant, and one reel.
set -uo pipefail
A=${ARGENT_BIN:-$HOME/argent/target/release/argent}
RD=$HOME/argent/assets/ringdesigner
OUT=${1:-$HOME/argent/out/showcase}
SPP=${SPP:-160}
mkdir -p "$OUT"
log() { echo "== $(date -u +%T) $*"; }
stl() { ls "$RD/$1"/*.stl 2>/dev/null | head -1; }

shoot() { # design metal studio view name [extra...]
  local f; f=$(stl "$1"); [ -z "$f" ] && { echo "missing $1"; return; }
  log "$5"
  "$A" render "$f" --metal "$2" --studio "$3" --view "$4" --spp "$SPP" -o "$OUT/$5.png" "${@:6}"
}

shoot nocturne-signet sterling noir hero   nocturne-sterling-noir
shoot nocturne-signet gold14k  noir hero   nocturne-gold-noir
shoot thalassa        sterling velvet hero thalassa-sterling-velvet
shoot oriel           gold18k  noir three-quarter oriel-gold18k-noir
shoot manticora       bronze   ember hero  manticora-bronze-ember
shoot arachne         sterling ember hero  arachne-sterling-ember
shoot caiman          sterling noir side   caiman-sterling-side
shoot saurian         gold14k  window hero saurian-gold-window
shoot zenith          rose14k  noir top    zenith-rose-top
shoot vesper          sterling white hero  vesper-sterling-white --size listing
shoot nocturne-signet sterling noir detail nocturne-macro --fstop 4
shoot nocturne-signet gold14k  noir hero   nocturne-black-opal --stone 'stone*=black_opal'

f=$(stl solstice-signet)
log "sheet solstice"
"$A" sheet "$f" --metals sterling,gold14k,rose14k,bronze --views hero,front,side,top --spp 96 -o "$OUT/sheet-solstice"

f=$(stl nocturne-signet)
log "reel nocturne (reveal)"
"$A" turntable "$f" --metal gold14k --rig reveal --seconds 6 --hook "Night garden, cast in gold" --qc -o "$OUT/nocturne-reveal.mp4"

log "index"
{
  echo "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'><title>argent showcase</title>"
  echo "<style>body{background:#0d0d0e;color:#e9e3d6;font-family:Georgia,serif;margin:0;padding:16px}.g{display:grid;grid-template-columns:repeat(auto-fill,minmax(260px,1fr));gap:12px}img,video{width:100%;border-radius:8px;display:block}figcaption{font-size:13px;color:#b8ad9b;margin:4px 2px 0}</style><h1>argent</h1><div class=g>"
  [ -f "$OUT/nocturne-reveal.mp4" ] && echo "<figure><video src=nocturne-reveal.mp4 autoplay muted loop playsinline></video><figcaption>nocturne · reveal reel</figcaption></figure>"
  for p in "$OUT"/*.png; do n=$(basename "$p" .png); echo "<figure><img loading=lazy src='$n.png'><figcaption>$n</figcaption></figure>"; done
  [ -d "$OUT/sheet-solstice" ] && for p in "$OUT"/sheet-solstice/*-sheet.png; do echo "<figure><img src='sheet-solstice/$(basename "$p")'><figcaption>listing sheet</figcaption></figure>"; done
  echo "</div>"
} > "$OUT/index.html"
log "done → $OUT/index.html"
