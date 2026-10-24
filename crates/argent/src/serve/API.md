# argent studio API

`argent serve --library DIR[,DIR…] --out DIR [--host 0.0.0.0] [--port 8790]`

One shared **set** (a piece + look + camera) that the phone page and agents art-direct together. The server
refines it progressively and streams frames; final renders run as queued jobs that write under `--out`.
All bodies are JSON. Errors come back as `{"error": "…"}` with a 4xx status. Unknown fields are rejected
(a typo is an error, not a silent no-op).

**Paths:** only meshes, `.hdr` environments and audio under the `--library` roots can be opened. Paths are
canonicalised and prefix-checked (absolute, relative to a root, or a library `id`). Nothing is written or served
outside `--out`.

## Library and piece

| Call | Does |
|---|---|
| `GET /api/library` | `{roots, demo, current, pieces[], hdris[], audio[]}`. `pieces[]`: `{id, name, path, rel, group, bytes, triangles?, stones, cached}`. A RingDesigner `X_stones.obj` rides along with `X.stl` (its gems come from `X_stones.json`). |
| `POST /api/open` | `{path \| id, flip?, rotate?: "x,y,z", units?: mm\|cm\|m\|in, fit?: mm, scale?, chain?: auto\|on\|off, no_orient?, look?: {…}, wait?: bool}`. Puts the piece on set. Cached pieces (last 3) are instant. Otherwise it returns `{"status": "preparing", "token", "estimate"}` while posing, BVH and patina mask build (about 3 s for 1.5M triangles). `wait: true` blocks until `{"status": "ready", "piece": {…}}` or a 422 error. `look` applies once it is on set. |
| `GET /api/state` | `{params, piece, live, jobs, library, out, uptime_s, version}`. `piece.status` is `none`, `preparing` (with `progress`, `elapsed`, `estimate`), `ready` or `failed` (with `error`). `piece.current` gives `name`, `triangles`, `kind`, `size_mm`, `bore_mm`, `us_size`, `weights_g` per alloy (metal parts only), and `parts[]` with each part's effective `gem` and its `source` (`set`, `file`, `name`, `auto` or `metal`). |
| `GET /api/options` | Every valid value: metals (key, display name, swatch), studios, views, gems, grades, formats, defaults. |

## The look (live set)

`GET /api/look` returns the set. `POST /api/look` applies a **partial** update and returns the whole set:

| Field | Values |
|---|---|
| `metal` | alloy key or alias: `sterling`, `gold14k` (= gold-filled look), `bronze`, `rose14k`, `white14k`, `gold18k`, `platinum`, … |
| `patina` | 0 (bright polish) … 1 (black recesses) |
| `roughness` (alias `polish`) | 0.01 … 0.6 (0.03 mirror, 0.35 satin) |
| `studio` | `noir` (brand), `white` (listing), `velvet`, `ember`, `window` |
| `hdri` | library `.hdr` path or id: lights the piece with that environment instead of the softboxes (the studio's floor stays); `"none"` returns to the studio lights |
| `hdri_rotate`, `hdri_exposure` | degrees; EV (−6 … 6) |
| `exposure` | EV, −8 … 8 |
| `grade` | `neutral` (default), `punchy`, `base`, `golden` (AgX looks), `aces` |
| `stones` | replace all rules: `[{"part": "stone_*", "gem": "sapphire"}]` (case-insensitive globs over part names) |
| `stone` | set one: `{"part": "stone_03", "gem": "ruby"}`; `"gem": "metal"` removes that rule |
| `view` | preset: `hero`, `front`, `side`, `top`, `three-quarter`, `back`, `low`, `detail` (sets azimuth, elevation, fill) |
| `azimuth`, `elevation`, `fill` | degrees (−180 … 180), degrees (−30 … 89), share of the frame (0.1 … 3). The view becomes `custom` |
| `turntable` | turn the piece, degrees; non-zero frames the full turn (as `argent render --turn`) |
| `focal`, `fstop` | mm; f-number (`0` sharp, `"auto"` = f/16 in mood studios, sharp in `white`) |
| `format` | `story` (1080×1920), `listing` (2000×2000), `wide` (1920×1080) or `WxH`. The preview renders at this aspect, and a still renders at its full size |
| `denoise` | live-preview denoising on/off |
| `max_spp` | where the live view stops refining (default 256; it also stops once successive frames differ by < 0.5 %) |

Exposure, grade and `denoise` only re-develop the frame (the samples are kept). Everything else restarts
accumulation.

## Frames

* **`GET /api/frame.jpg?min_spp=N&wait=S`**: the latest preview JPEG. `min_spp` + `wait` (seconds, ≤ 300)
  block until the *current* set has refined to N samples per pixel, or converged. Response headers:
  `x-argent-frame` (the JSON header below), `x-argent-spp`, `x-argent-gen`, `x-argent-seq`.
* **Idle policy (24/7 on a shared box):** the server starts with nothing on set. The preview renders only
  while a visible page holds the stream, or for 20 s after the last frame fetch or API call. It stops
  refining at `max_spp`, or earlier once frames settle. An idle studio uses no CPU.
* **`GET /api/ws`** (WebSocket): what the page uses.
  * Server → client, **binary**: `[u32 little-endian header length][JSON header][JPEG]`. Header:
    `{seq, gen, spp, max_spp, w, h, half, denoised, denoiser, converged, ms, bytes}`. `ms` is the time from
    the change to this frame; `half` marks the quick half-resolution look right after a change.
  * Server → client, **text**: `hello` `{options, state}` on connect, then one message per change:
    `params`, `piece`, `jobs` (`{active[], log_version}`; refetch `/api/jobs` when `log_version` moves),
    `live` (preview paused/resumed), and `error`.
  * Client → server: `{"type": "ack", "seq": n}` after drawing each frame (at most 2 frames are in flight
    unacknowledged; a client that never acks gets one every 3 s); `{"type": "look", …same fields as POST
    /api/look}`; `{"type": "visibility", "visible": false|true}` when the page is hidden or shown (a hidden
    page doesn't keep the renderer awake); `{"type": "ping"}`.

Pacing: up to ~12 fps of half-resolution frames while the set is changing (orbiting), then full-resolution
refinement frames at ≤ 6 fps that slow down as samples accumulate. Frames go out only when something changed.
In-progress frames use OIDN at Fast quality; the converged frame is re-denoised at full quality.

## Final renders (jobs)

`POST /api/render` queues a job (202, the job record). Jobs run one at a time, and the live preview pauses
while one renders. Every job snapshots the set when it is queued; `look` (same fields as `/api/look`)
overrides it for this job only. `piece` (library path or id) renders another piece without disturbing the
set.

| `kind` | Options (defaults) | Writes |
|---|---|---|
| `still` | `size` (the set's `format`), `spp` (128) | `stills/<piece>-<metal>-<view>-<WxH>-<time>-<id>.png`, exactly the live frame |
| `sheet` | `metals` (sterling, gold14k, bronze), `views` (hero, front, side), `size` (1600x1600), `spp` (128), `title` | `sheets/<piece>-<time>-<id>/`: one PNG per metal × view, a captioned contact sheet and a JSON manifest (via `jobs::sheet_prepared`) |
| `reel` | `seconds` (6; 1 … 90), `fps` (30), `spp` (32), `rig`: `turntable` \| `reveal` \| `sweep`, `hook` (burned-in text), `audio` (library audio path or id; default silent AAC) | `reels/<piece>-<metal>-<rig>-<time>-<id>.mp4` (1080×1920 h264 + AAC, Forge's reel spec, via `jobs::render_reel`) plus a `-poster.jpg` |

* `GET /api/jobs?limit=N`: newest first, `{jobs[], log_version}`. Each job:
  `{id, kind, status: queued|running|done|failed|cancelled|interrupted, piece, piece_id, created, started,
  finished, progress (0…1), eta_s, elapsed_s, message, error, spec: {job, set}, outputs: [{file, url,
  thumb, kind: image|video|data, label}]}`.
* `GET /api/jobs/{id}?wait=S`: one job. `wait` long-polls (≤ 300 s) until it finishes; poll again for longer
  renders.
* `POST /api/jobs/{id}/cancel`: a queued job is dropped; a running still stops at its next chunk; a running
  sheet or reel can't be stopped part-way (409).
* `GET /files/<path>[?dl=1]`: outputs under `--out` (single byte ranges supported, so mp4 plays on iOS).
  Thumbnails live in `.thumbs/`.
* `<out>/jobs.json`: the job log (last 500). Jobs still queued or running when the server stopped come back
  as `interrupted`.

At most 24 jobs may wait in the queue.

## Agent recipe

```sh
S=http://100.96.121.37:8790
curl -s $S/api/library | jq '.pieces[] | {id, name}'
curl -s -X POST $S/api/open -d '{"id": "5113d439ba", "wait": true, "look": {"metal": "gold14k", "studio": "noir"}}'
curl -s -o preview.jpg "$S/api/frame.jpg?min_spp=64&wait=60"                 # look before spending a final
J=$(curl -s -X POST $S/api/render -d '{"kind": "still", "spp": 256}' | jq -r .id)
curl -s "$S/api/jobs/$J?wait=300" | jq '.status, .outputs[].url'
curl -s -X POST $S/api/render -d '{"kind": "reel", "rig": "reveal", "hook": "Death had a name: Thanatos"}'
```
