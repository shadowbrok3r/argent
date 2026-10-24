#!/usr/bin/env bash
# The morning showcase: final-quality renders of Logan's RingDesigner
# designs for the report. ~20 min on the 32-core box.
set -uo pipefail
A=${ARGENT_BIN:-$HOME/argent/target-int/release/argent}
RD=$HOME/argent/assets/ringdesigner
OUT=${1:-$HOME/argent/out/showcase}
mkdir -p "$OUT"
log() { echo "== $(date -u +%T) $*"; }
stl() { find "$RD/$1" -maxdepth 1 -name '*.stl' | head -1; }
r() { # name design [args...]
  local name=$1 d=$2; shift 2
  local f; f=$(stl "$d"); [ -z "$f" ] && { echo "missing $d"; return; }
  log "$name"; "$A" render "$f" -o "$OUT/$name.png" "$@" 2>&1 | tail -1
}

# Hero (wide for the report header, tall for social)
r hero-wide-nocturne-gold nocturne-signet --metal gold14k --view hero --size 1920x1080 --spp 256
r hero-nocturne-gold nocturne-signet --metal gold14k --view hero --spp 256
# One file, every metal
for m in sterling gold14k rose14k bronze; do r "metal-solstice-$m" solstice-signet --metal $m --view hero --size 1080x1350 --spp 128; done
# Antique finish on vs off
r patina-ophidian-off ophidian --metal sterling --patina 0 --view side --size 1080x1080 --spp 160
r patina-ophidian-on  ophidian --metal sterling --patina 1 --view side --size 1080x1080 --spp 160
# Stones
r stones-manticora manticora --metal sterling --view hero --size 1080x1350 --spp 192
r stones-arachne   arachne   --metal sterling --view hero --size 1080x1350 --spp 192
r stones-oriel     oriel     --metal gold18k  --view three-quarter --size 1080x1350 --spp 192
r stones-thalassa  thalassa  --metal sterling --view top --size 1080x1350 --spp 192
# Opal
r opal-nocturne-top   nocturne-signet --metal gold14k --stone 'stone*=black_opal' --view top --size 1080x1350 --spp 192
r opal-nocturne-macro nocturne-signet --metal gold14k --stone 'stone*=black_opal' --view detail --fstop 5.6 --size 1080x1350 --spp 256
# Studios on one piece
for s in noir white velvet ember window; do r "studio-zenith-$s" zenith --metal sterling --studio $s --view hero --size 720x1280 --spp 128; done
r studio-zenith-hdri zenith --metal sterling --hdri "$HOME/argent/library/hdri/photo_studio_loft_hall_2k.hdr" --view hero --size 720x1280 --spp 128
# A listing in white
r listing-vesper-white vesper --metal sterling --studio white --size listing --spp 160
# Pendant on a chain (CC BY 4.0 NIH skull scan, scaled to 26 mm)
log pendant-skull
"$A" render "$HOME/argent/assets/test/vhm_skull_nih3d_3DPX-012260.ply" --fit 26 --chain on --rotate z-90,y-90 --metal sterling --patina 0.9 --view front --spp 192 -o "$OUT/pendant-skull.png" 2>&1 | tail -1
# The catalog: every exported design, sterling, noir
mkdir -p "$OUT/catalog"
for d in "$RD"/*/; do
  n=$(basename "$d"); [ "$n" = tools ] && continue
  f=$(stl "$n"); [ -z "$f" ] && continue
  "$A" render "$f" --metal sterling --view hero --size 720x900 --spp 64 -o "$OUT/catalog/$n.png" 2>&1 | tail -1
done
log "reel"
f=$(stl nocturne-signet)
"$A" turntable "$f" --metal gold14k --rig reveal --seconds 6 --hook "Night garden, cast in gold" \
  --audio "$HOME/argent/library/audio/ES_Lone_Wolf-Toby_Tranter.mp3" --qc -o "$OUT/reel-nocturne-reveal.mp4" 2>&1 | grep -v '^\s*frame' | tail -16
log done
