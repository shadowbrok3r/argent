# argent-export

Turns a casting mesh (1–5 M triangles, millimetres) into a GLB for Shopify's 3D product media: an interactive spin on the product page, and "View in your space" AR on phones.

```rust
let mesh = argent_mesh::load_many(&[ring_stl, stones_obj], &Default::default())?;
let opts = argent_export::GlbOptions {
    base_color: [0.97, 0.96, 0.91],                    // metal F0, linear
    patina: Some((mask, [0.035, 0.03, 0.028], 0.85)),  // cavity mask, tarnish, strength
    name: "oriel".into(),
    ..Default::default()
};
let report = argent_export::export_glb(&mesh, &opts, Path::new("oriel.glb"))?;
```

## What Shopify expects

| | |
|---|---|
| Format | GLB (binary glTF 2.0). Shopify serves it to `<model-viewer>` on the web and converts it to USDZ for iOS AR Quick Look. |
| Size | The upload cap is 500 MB, but keep files ≤ 15 MB so the page loads fast. That is the default `max_bytes`. |
| Units and axes | glTF is in metres with +Y up and the front toward +Z. Positions are written as mm × 0.001. argent's canonical pose (ring head +Y, face +Z) therefore stands upright, facing the default camera. |
| Materials | PBR metallicRoughness. Metal parts get metallic 1 with `base_color` as the base colour (a metal's F0) and `roughness`. Stones get a glass-like material: `KHR_materials_transmission`, `KHR_materials_ior` and `KHR_materials_volume`, with a tint and IOR looked up from the gem word. Opaque stones (onyx, pearl and other `black` stones) get IOR only. |

## What it does

1. **Simplify** (`simplify`). This is Garland–Heckbert quadric-error edge collapse, under a few rules:
   - Weld: vertices are welded exactly per part, so crease-split loader output merges again, but parts never share a vertex.
   - Quadrics: each vertex starts from area-weighted face planes. Boundary edges add weighted planes.
   - Placement: the exact optimum is used when the quadric is well conditioned. Otherwise the placement is a Tikhonov-regularised optimum grown from the edge midpoint, with the midpoint, the best point on the edge and both endpoints as fallbacks.
   - Queue: a lazy-deletion radix heap.
   - Rejected collapses: any collapse that breaks the link condition, pinches two boundaries together, turns a neighbour triangle by more than ~75°, turns one more than 90° from its original orientation, degenerates it or creates a worse sliver.
   - Locked vertices: non-manifold edges, bow-tie vertices and part seams.
   - Stones: stone parts of ≤ 5,000 triangles are kept verbatim, as long as together they stay within a quarter of the budget.
   - Output: the returned map gives each input vertex its output vertex. `carry_scalars` averages any per-vertex value over the merged cluster.
2. **Patina** goes into `COLOR_0` as a multiplier of the base colour. Its value is `mix(base, tarnish, mask · strength) / base`, so a viewer that ignores vertex colours still shows the bare metal.
3. **Normals** are crease-split and angle-weighted:
   - metal, and stones over 400 triangles (cabochons): 50°;
   - cut stones of ≤ 400 triangles: 1°, i.e. facet normals.
4. **GLB** layout:
   - one mesh and node per part, under a root node;
   - u32 indices, POSITION min/max;
   - 4-byte-aligned chunks and buffer views;
   - `extensionsUsed` lists what the materials use.

   If the file is over `max_bytes`, the collapse loop continues from where it stopped to a proportionally smaller target, and the GLB is rebuilt (up to 8 passes; after that it returns an error).

### Defaults

| Option | Default |
|---|---|
| `target_triangles` | 150,000 |
| `max_bytes` | 15,000,000 |
| `base_color` | `[0.97, 0.96, 0.91]` (polished sterling) |
| `roughness` | 0.2 |
| `patina` | none |

RingDesigner's `_stones.obj` labels every stone `usemtl gem`, so its stones export as diamond-clear. To colour them, set each stone part's `material` to its species before exporting (for example from the `_stones.json` label, as argent's renderer does).

Part classification: if a part's material names a stone, it is a stone. If the material names a metal, it is metal. Otherwise a stone word in the part name decides (generic `stone`/`gem` → diamond). The exception is the part that holds most of the triangles: it stays metal unless its material says otherwise, because castings are often named after their stone (`ophidian-amethyst-serpent.stl`).

## Measured

Setup:
- RingDesigner rings from `assets/ringdesigner/`, read as raw STL triangle soups, simplified to 150 k triangles.
- 32 cores, `nice 10`, with a load average of 38–47 from other builds.
- Deviation: every original vertex, plus one random point per original triangle, measured to the simplified surface of the same part.

| Ring | Simplify (setup + loop) | Whole export | GLB | GLB vertices | Max dev. | Mean dev. |
|---|---|---|---:|---:|---:|---:|
| Aster signet, 2,621,440 tris | 7.3 s (0.9 + 6.1) | 7.6 s | 4.64 MB | 78,957 | 0.0044 mm | 0.00028 mm |
| Tessera, 2,621,440 | 7.6 s (1.0 + 6.4) | 6.9 s | 5.01 MB | 89,024 | 0.0100 mm | 0.00063 mm |
| Solstice signet, 2,621,440 | 6.6 s (1.0 + 5.5) | 7.1 s | 4.64 MB | 78,847 | 0.0044 mm | 0.00028 mm |
| Oriel + 37 stones, 2,595,028 | – | 7.3 s | 5.49 MB | 106,069 | 0.025 mm | 0.0011 mm |
| Aureole, 262,144 → 50 k | – | 0.9 s | 1.50 MB | 25,000 | 0.0020 mm | 0.00035 mm |

- The sizes above include a `COLOR_0` patina layer. Without it, Oriel is 4.37 MB.
- Oriel through `argent_mesh::load_many` (welded and crease-split, 38 parts, no patina) takes 6.8 s and gives 4.37 MB with the same 0.025 mm max deviation.
- Peak RSS is 1.3–1.4 GB. Most of that is the test's 7.8 M-vertex STL soup.
- Every file passes the Khronos glTF-Validator (npm `gltf-validator` 2.0.0-dev.3.10) with 0 errors, 0 warnings, 0 infos and 0 hints.
- `npx gltf-validator` itself fails: the npm package has no CLI.

```sh
cd ~/argent && CARGO_TARGET_DIR=$HOME/argent/target-export cargo test -p argent-export --release                # fast suite
cd ~/argent && CARGO_TARGET_DIR=$HOME/argent/target-export cargo test -p argent-export --release -- --ignored --nocapture   # 2.6 M benchmarks; GLBs land in target-export/tmp/
```

## Limits

- The deviation is one-sided (original → simplified) and sampled, not an exact Hausdorff distance.
- Patina detail is limited to vertex density: about 0.1 mm spacing at 150 k triangles on a ring.
- Shopify's USDZ conversion has not been checked here. Expect iOS AR to drop the transmission and volume extensions, so stones there look like tinted glossy dielectrics.
