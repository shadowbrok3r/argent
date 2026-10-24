# Rendering Forge reels with argent

Forge's Producer renders ordered items on the studio PC "whenever it is on and its GPUs are free". The
`turntable` rig is the one rig that needs neither Houdini nor a GPU, and argent renders it on the always-on
R9700 box's CPUs instead. Nothing here changes Forge itself; these are the calls a Producer step can make.

## The reel argent makes

`argent turntable` writes exactly what Forge's QC asks for, and `--qc` runs the same checks with the same names:

| Forge check | argent output |
|---|---|
| video stream | h264 (libx264, High) |
| resolution 1080x1920 | 1080×1920 (default `--size`) |
| 30 fps | 30 (default `--fps`) |
| h264 yuv420p | yuv420p, BT.709 matrix and tags |
| aac 48 kHz audio | silent AAC 48 kHz stereo, or `--audio track.m4a` muxed and trimmed |
| duration 3-90 s | `--seconds` (default 6) |
| bitrate under 25 Mbps | CRF 18 capped at 16 Mbps |
| file under 300 MB | ~10–20 MB for 6 s |
| no black stretches over 0.5 s | the noir backdrop is near-black, not black; the piece fills ~60% of the frame |

Hooks are burned in for the first ~2 s with `--hook`, wrapped only at spaces (never at a hyphen: the
"SOUL-/FIRE" break from the Thanatos pendant reel cannot happen).

## From the studio PC

```sh
# One-off, over Tailscale (the mesh is copied to the box, the reel copied back):
scp design.stl ubuntu-ai-amd:/home/shadowbroker/argent/library/incoming/
ssh ubuntu-ai-amd /home/shadowbroker/.local/bin/argent turntable \
    /home/shadowbroker/argent/library/incoming/design.stl \
    --metal gold14k --studio noir --hook "Death had a name: Thanatos" --qc \
    -o /home/shadowbroker/argent/out/forge/item_c8bc4ca0e655.mp4
scp ubuntu-ai-amd:/home/shadowbroker/argent/out/forge/item_c8bc4ca0e655.mp4 .
```

Or through the studio's JSON API (`argent serve`, see `crates/argent/src/serve/API.md`): open the piece,
then `POST /api/render {"kind":"reel", ...}` and poll `GET /api/jobs/{id}` for the file.

## Mapping Forge fields

| Forge item | argent |
|---|---|
| `rig_id: turntable` | `--rig turntable --studio noir` (the brand strip-light look) |
| `metal: gold` | `--metal gold14k` (the gold-filled look; captions say "shown in gold") |
| `metal: silver` | `--metal sterling` |
| `hook` | `--hook "…"` |
| `audio` | `--audio <track>` once the Epidemic track is cut, else silent |

## New rigs worth adding to Forge's rig table

Both run on argent alone and suit every design, so `coverage_report` would treat them like `turntable`:

| id | duration | description |
|---|---|---|
| `reveal` | 6–8 s | Macro on the face with shallow focus, dolly out while focus racks to the whole piece, then a full turn. |
| `sweep` | 4–6 s | A hot strip light glides across the piece while it turns 40°: the jewelry-commercial glint. |
