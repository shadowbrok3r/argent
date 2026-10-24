---
name: argent
description: Photoreal renders of Logan's jewelry from STL/OBJ/3MF with argent (spectral path tracer on the R9700 box's CPUs): stills in any metal and studio, Forge-spec turntable reels, listing sheets with weight estimates. Read it before rendering a ring or pendant for content, a listing or a design review.
---

# argent — jewelry renders from the mesh

`/home/shadowbroker/.local/bin/argent` renders a real casting file (STL, OBJ, PLY, 3MF; millimetres) the way a
studio photograph looks: measured metal optics, oxidised recesses, real gem dispersion, spectral opal. It runs
on this box's 32 CPU cores at low priority, so it never touches the GPUs ComfyUI and the LLMs use.

## Where the pieces are
- RingDesigner exports: `/home/shadowbroker/argent/assets/ringdesigner/<design>/` (metal STL; stones in
  `<name>_stones.obj` are picked up automatically, and each stone's gem comes from `<name>_stones.json`).
- The studio library pulled from the desktop: `/home/shadowbroker/argent/library/jewelry-scan/` (RING, PENDANTS).
- `argent info FILE` prints size, ring size, piece kind and the weight in sterling / 14k / 18k / bronze / platinum.

## Commands (always pass `--marker`, write under your workspace)
```sh
OUT=/home/shadowbroker/.zeroclaw/agents/comfy/workspace/renders/argent
argent --marker render FILE --metal gold14k --studio noir --view hero -o $OUT/LABEL.png
argent --marker render FILE --metal sterling --patina 0.9 --studio white --size listing -o $OUT/LABEL.png
argent --marker --detach turntable FILE --metal gold14k --hook "Death had a name: Thanatos" --qc -o $OUT/LABEL.mp4
tail -2 $OUT/LABEL.mp4.log      # later: progress, then the [VIDEO:...] line when it is done
argent --marker sheet FILE --metals sterling,gold14k,rose14k,bronze --views hero,front,side,top -o $OUT/LABEL
```
- The last line printed is the attachment, `[IMAGE:...]` or `[VIDEO:...]`. Copy it exactly, first, in the reply.
- Metals: `sterling`, `fine-silver`, `gold14k` (also the look of gold-filled), `gold18k`, `gold24k`, `rose14k`,
  `white14k`, `bronze`, `brass`, `copper`, `platinum`. Write "shown in gold", never a karat, in captions.
- Finish: `--patina 0..1` (0 bright polish, 1 black recesses; the studio's antique look is 0.8), `--roughness`
  (0.04 mirror, 0.07 default polish, 0.2 satin).
- Studios: `noir` (the brand look: black, strip lights, black acrylic), `white` (marketplace listing), `velvet`,
  `ember` (underworld: fire rim light), `window` (soft daylight).
- Views: `hero`, `front`, `side`, `top`, `three-quarter`, `back`, `low`, `detail`; or `--angle AZ,EL`. `--turn DEG`
  rotates the piece. `--flip` turns it around when the face points away.
- Stones: `--stone 'PART=GEM'` (globs over part names, e.g. `--stone 'stone*=sapphire'`). Gems: diamond,
  sapphire, ruby, emerald, amethyst, citrine, peridot, garnet, opal, black_opal, onyx and more (`argent render
  --stone x=?` lists them).
- Sizes: `story` 1080x1920 (default), `listing` 2000x2000, `square` 2048x2048, `wide` 1920x1080, or `WxH`.
- Time on this box: a 1080x1920 still at 128 spp ≈ 15–25 s; a 6 s reel ≈ 8–15 min. Reels always go through
  `--detach` (your shell call would time out): say it started, check the log on a later turn, then deliver.
- Reel motion: `--rig turntable` (full turn), `reveal` (macro rack-focus dolly-out, then a turn), `sweep` (a strip
  light glides across). `--audio TRACK` lands the track's drop on the rig's main moment.

## Forge
A turntable reel from `argent turntable` meets Forge's reel spec (1080x1920, 30 fps, h264 yuv420p, AAC 48 kHz,
3–90 s, < 25 Mbps) and `--qc` runs Forge's own checks. It is a file only: the owner decides what is posted.

## The live studio
`http://100.96.121.37:8790` — Logan's phone studio: open a piece, drag to orbit, change metal/studio/patina and
watch it converge; final stills, sheets and reels from there land in `/home/shadowbroker/argent/out/studio`.
