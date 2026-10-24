# argent

Photoreal jewelry renders straight from the casting file. Give it the STL you print wax from; it gives you the
listing photo, every metal variant, a turntable reel for Instagram/TikTok, and a live studio on your phone. No
photoshoot, no Houdini session, no desktop GPU: it runs on the R9700 box's 32 CPU cores at idle priority.

```sh
argent info thanatos.stl                                   # size, ring size, weight in every alloy
argent render thanatos.stl --metal gold14k -o hero.png     # brand look (noir studio), 1080x1920
argent render thanatos.stl --studio white --size listing   # marketplace white, 2000x2000
argent turntable thanatos.stl --hook "Death had a name: Thanatos" --qc -o reel.mp4
argent sheet thanatos.stl --metals sterling,gold14k,rose14k,bronze -o listing/
argent batch ~/argent/library/jewelry-scan -o catalog/    # every piece, index.html
argent serve                                               # live studio on :8790
```

More:

```sh
argent turntable ring.stl --rig reveal --audio track.mp3 -o reel.mp4   # macro rack-focus dolly-out; the drop lands on the reveal
argent turntable ring.stl --rig sweep -o glint.mp4                     # a strip light glides across the polish
argent render ring.stl --hdri library/hdri/studio_small_08_2k.hdr --hdri-rotate 120
argent render skull.ply --fit 26 --chain on --rotate z-90,y-90         # unknown units → 26 mm, upright, on a cable chain
argent --marker --detach turntable ring.stl -o out/reel.mp4            # background job for agents; tail out/reel.mp4.log
```

## What makes it jewelry-specific

| | |
|---|---|
| **Spectral metals** | Measured complex refractive indices (Johnson & Christy, Rakić) with exact Fresnel per wavelength. Gold, rose gold and bronze get their colour from physics, not a tint. Alloys (sterling, 14k/18k, rose, bronze, brass) are mixed from their pure metals. |
| **Antique finish** | Every piece gets a cavity mask (ambient occlusion + curvature) at load time. `--patina` blends a soft black tarnish into the recesses, polished highs stay bright: the liver-of-sulfur look of a hand-finished skull ring. |
| **Real gems** | Sellmeier dispersion with hero-wavelength sampling, so diamonds throw spectral fire. Absorption spectra give sapphire, ruby, emerald and the rest their colour through the stone's depth. |
| **Spectral opal** | Precious opal is diffraction, not pigment: each Voronoi domain is a lattice reflecting a narrow band at `λ = 2 d n cos θ`. Turn the piece and the flashes shift colour, as crushed opal really does. |
| **Photographed light** | `--hdri` lights the piece with an equirectangular HDR (importance-sampled, MIS with the softboxes); rotating it moves every highlight. |
| **Reels with timing** | Rigs: `turntable`, `reveal` (macro, rack focus, dolly out, turn), `sweep` (moving strip light). `--audio` finds the track's drop and lands it on the rig's main moment, as the studio PC does for Forge. |
| **Pendants hang** | A procedural cable chain runs through the bail in a V (`--chain auto` for detected pendants). |
| **Camera** | 100 mm macro on full frame, f/16 by default in mood studios with autofocus on the subject, sharp for listings; white balance follows each rig's strobes. |
| **Studio rigs** | `noir` (brand: black, strip softboxes, black acrylic), `white` (listing: tent + black flags so polish keeps its edges), `velvet`, `ember` (underworld fire rims), `window`. Softboxes are feathered so highlights grade like real diffusion. |
| **Auto-pose** | Finds a ring's finger hole and head, a pendant's bail and front, and frames the shot. `--flip`, `--rotate`, `--angle` override. |
| **Forge-ready reels** | 1080×1920, 30 fps, h264 yuv420p with BT.709 tags, AAC 48 kHz, < 25 Mbps; `--qc` runs Forge's own checks. Hooks are burned in without breaking words. |
| **CAD aware** | RingDesigner exports pair automatically: `ring.stl` brings `ring_stones.obj`, and each stone's gem comes from the `_stones.json` label. |

## Layout

| Crate | Does |
|---|---|
| `argent-core` | BVH (binned SAH → 4-wide SIMD), Owen-scrambled Sobol, spectral path tracer with NEE+MIS, GGX with multi-scatter compensation, materials, studios, framing, the `Prepared`/`Look` shot API |
| `argent-mesh` | STL/OBJ/PLY/3MF readers, welding, crease-aware normals, ring/pendant pose detection, cavity masks |
| `argent-spectra` | CIE 1931 observer, D65, RGB→spectrum (Jakob–Hanika), metal n,k tables, alloys, gem optics |
| `argent-post` | OIDN denoising (runtime-loaded) with an à-trous fallback, AgX tone mapping, PNG/JPEG/EXR, ffmpeg video, contact sheets, hooks, Forge QC |
| `argent` | the CLI and the live studio server |

## Deploy

```sh
cargo build --release -p argent && install -m755 target/release/argent ~/.local/bin/argent
cp deploy/argent-studio.service deploy/argent-desktop-catalog.* ~/.config/systemd/user/
systemctl --user daemon-reload && systemctl --user enable --now argent-studio argent-desktop-catalog.timer
```

`deploy/zeroclaw-skill/argent/SKILL.md` teaches a ZeroClaw agent the commands; outputs written inside the agent's
workspace come back as `[IMAGE:…]`/`[VIDEO:…]` attachments with `--marker`.
