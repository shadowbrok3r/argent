# argent test assets

Real jewellery meshes for local render testing. Everything in `ringdesigner/` and `gems/` comes from Logan's own **RingDesigner** (github `shadowbrok3r/RingDesigner`, licensed `MIT OR Apache-2.0`, built from commit `006e043`): his authored showcase designs, exported here for local render testing. `test/` holds one third-party mesh (CC BY 4.0, attribution below). The repo's `.gitignore` excludes everything under `assets/` except this README and the exporter source, so none of these meshes are in git.

## Conventions (RingDesigner rings)

- **Units and frame:** millimetres. The ring is centred on its bore with **Z along the finger axis** and the head/table toward **+Y**.
- **Size:** each design's own US size, not resized to 7. The imported factory bases are validated per size, and these are the sizes the designs were authored and checked at.
- **`*.stl`** (binary): the **finished ring at nominal size** (`mesh::try_build`). Made settings (seat cuts, collets, heads), stamps, CAD parts and bench-only layers are all in place; nothing is shrink-scaled. RingDesigner's own showcase saves this mesh as `finished-metal.stl`. Facets are unwelded, as in any STL.
- **Verification:** all 23 files were re-read from disk. Each is watertight (0 open and 0 non-manifold edges after welding), has no degenerate faces and is wound outward (positive volume).
- **`*_stones.obj`:** every stone as its own closed, outward-wound, flat-faceted solid (`o stone_NN` + `usemtl gem`), in the same frame as the STL. Render stones with facet normals: many adjacent facets meet at less than argent's default 50° crease angle and would otherwise be smoothed. Stones sit where RingDesigner draws them, with the girdle settled into the seat, so a pavilion may overlap the metal (not measured).
- **`*_stones.json`:** one record per stone: RingDesigner's label (often names the species), cut, form, `w_mm` × `l_mm`, carats, preview tint (linear RGB; `[0.72, 0.82, 0.92]` is RingDesigner's neutral default), girdle centre, table normal, triangle count and closed flag.
- **`*.glb`** (stone-set designs only): the same finished metal (same triangles and bounding box as the STL) with **no stones**, because RingDesigner's GLB writer never includes them. Units are metres (mm × 0.001), with one gold PBR material (metallic 1, roughness 0.22). argent-mesh's loaders don't cover GLB; these files are for outside viewers.

## RingDesigner rings (23)

Each folder holds `<stem>.stl`. Stone-set designs also hold `<stem>_stones.obj`, `<stem>_stones.json` and `<stem>.glb`.

| Path (under `assets/ringdesigner/`) | Source design (`showcase/…`) | US size | Steps | Triangles | STL MB | Stones | GLB MB |
|---|---|---:|---|---:|---:|---|---:|
| `arachne/arachne-the-weaver_size7.stl` | Arachne — the weaver (`bestiarium/arachne`) | 7 | 2048x640 | 2,464,936 | 123.2 | yes, 2 | 59.2 |
| `aster/aster-botanical-sand-signet_size8.11.stl` | Aster - botanical sand signet (`aster`) | 8.11 | 2048x640 | 2,621,440 | 131.1 | no | – |
| `aurelia/aurelia-sovereign-sun_size8.85.stl` | Aurelia — sovereign sun (`stock-masterworks/aurelia`) | 8.85 | 1536x448 | 1,376,256 | 68.8 | no | – |
| `aureole/aureole-half-turn-ribbon_size8.11.stl` | Aureole — half-turn ribbon (`workshop-collection/aureole`) | 8.11 | 2048x640 | 262,144 | 13.1 | no | – |
| `caiman/caiman-armoured-hide_size9.09.stl` | Caiman — armoured hide (`stock-masterworks/caiman`) | 9.09 | 1536x448 | 1,365,380 | 68.3 | yes, 1 | 32.8 |
| `cushion-seal/aster-cushion-seal_size8.11.stl` | Aster — cushion seal (`workshop-collection/aster`) | 8.11 | 2048x640 | 2,621,440 | 131.1 | no | – |
| `ecdysis/ecdysis-ventral-scales_size8.6.stl` | Ecdysis — ventral scales (`reptilia/ecdysis`) | 8.6 | 2048x640 | 2,621,440 | 131.1 | no | – |
| `lantern/lantern-pierced-octagonal-signet_size8.11.stl` | Lantern — pierced octagonal signet (`workshop-collection/lantern`) | 8.11 | 2048x640 | 1,608 | 0.1 | no | – |
| `lorica/lorica-crocodile-armour_size8.6.stl` | Lorica — crocodile armour (`reptilia/lorica`) | 8.6 | 2048x640 | 2,621,440 | 131.1 | no | – |
| `manticora/manticora-the-tail-that-throws_size8.6.stl` | Manticora — the tail that throws (`bestiarium/manticora`) | 8.6 | 2048x640 | 2,504,922 | 125.2 | yes, 11 | 60.1 |
| `nocturne-signet/nocturne-night-garden-signet_size8.11.stl` | Nocturne — night garden signet (`masterwork-signets/nocturne`) | 8.11 | 2048x640 | 2,621,440 | 131.1 | yes, 3 | 62.9 |
| `nocturne-stock/nocturne-night-garden_size8.11.stl` | Nocturne — night garden (`stock-masterworks/nocturne`) | 8.11 | 1536x448 | 1,727,562 | 86.4 | yes, 3 | 41.5 |
| `ophidian/ophidian-amethyst-serpent_size8.6.stl` | Ophidian — amethyst serpent (`reptilia/ophidian`) | 8.6 | 1536x480 | 1,454,442 | 72.7 | yes, 1 | 34.9 |
| `oriel/oriel-jewelled-lantern_size7.stl` | Oriel — jewelled lantern (`oriel`) | 7 | 2048x640 | 2,587,420 | 129.4 | yes, 37 | 62.1 |
| `saurian/saurian-beaded-skin_size8.11.stl` | Saurian — beaded skin (`stock-masterworks/saurian`) | 8.11 | 1536x448 | 1,372,770 | 68.6 | yes, 1 | 32.9 |
| `solstice-signet/solstice-solar-palmette-signet_size8.11.stl` | Solstice — solar palmette signet (`masterwork-signets/solstice`) | 8.11 | 2048x640 | 2,621,440 | 131.1 | no | – |
| `solstice-stock/solstice-sun-seal_size8.11.stl` | Solstice — sun seal (`stock-masterworks/solstice`) | 8.11 | 1536x448 | 1,376,256 | 68.8 | no | – |
| `tessera/tessera-shield-mosaic_size8.6.stl` | Tessera — shield mosaic (`reptilia/tessera`) | 8.6 | 2048x640 | 2,621,440 | 131.1 | no | – |
| `thalassa/thalassa-tidal-reliquary-signet_size8.23.stl` | Thalassa — tidal reliquary signet (`thalassa`) | 8.23 | 2048x640 | 2,621,440 | 131.1 | yes, 7 | 62.9 |
| `tide/tide-twelve-reeds_size8.11.stl` | Tide — twelve reeds (`workshop-collection/tide`) | 8.11 | 2048x640 | 2,621,440 | 131.1 | no | – |
| `varanus/varanus-sovereign-scales_size8.6.stl` | Varanus — sovereign scales (`reptilia/varanus`) | 8.6 | 1536x480 | 1,474,560 | 73.7 | no | – |
| `vesper/vesper-celestial-reliquary_size9.34.stl` | Vesper — celestial reliquary (`stock-masterworks/vesper`) | 9.34 | 1536x448 | 1,851,678 | 92.6 | yes, 15 | 44.4 |
| `zenith/zenith-the-hunter-s-belt_size8.6.stl` | Zenith — the hunter's belt (`stock-masterworks/zenith`) | 8.6 | 1536x448 | 1,316,936 | 65.8 | yes, 3 | 31.6 |

**Steps** is RingDesigner's sweep resolution: samples around the ring × samples across the section.

- Procedural designs use 2048x640.
- Designs on imported factory bases (`ophidian`, `varanus` and the seven stock masterworks) keep their authored release steps. RingDesigner refuses imported relief past 2 M triangles, and Vesper fails at 2048x640.
- `aureole` and `lantern` are built by the CAD kernel. At these steps `aureole` comes out at 262,144 triangles (196,608 at its saved 768x256). `lantern` is a 1,608-triangle solid of flat faces at any steps.

### Stones

| Folder | Stones | Stone triangles |
|---|---|---:|
| `arachne/` | 2 oval cabochons: a garnet of 5 × 3.5 mm (carapace) and an onyx of 9 × 7 mm (abdomen) | 2,688 |
| `caiman/` | 1 emerald cut of 6 × 4 mm ("Emerald on the nuchal shield") | 136 |
| `manticora/` | 1 oval of 7 × 5 mm with a ruby tint, plus 10 graded princess cuts of 1.08–1.87 mm with a black-spinel tint | 1,628 |
| `nocturne-signet/` | 3 rounds: a 2.3 mm "Emerald dew" and two 1.5 mm shoulder emeralds | 522 |
| `nocturne-stock/` | 3 rounds: a 2.8 mm "Emerald dew" and two 1.5 mm shoulder gems | 522 |
| `ophidian/` | 1 oval amethyst of 7 × 5 mm, flush-set | 174 |
| `oriel/` | 37: an oval cabochon of 7.5 × 6 mm in a collet, 18 halo melee of 1.3 mm and 18 pavé rounds of 1.25 mm | 7,608 |
| `saurian/` | 1 round of 2.8 mm, flush on the spine | 174 |
| `thalassa/` | 7: a central oval of 5.44 × 3.4 mm ("sea-glass" bezel), 4 "pearl" settings of 1.25–1.3 mm and 2 shoulder rounds of 1.4 mm | 1,218 |
| `vesper/` | 15: an Asscher of 4.4 mm in a sculpted bezel, 12 constellation rounds of 1.15 mm and 2 shoulder marquises of 3.6 × 1.8 mm | 2,550 |
| `zenith/` | 3 rounds of 1.8–2.2 mm: Alnitak, Alnilam and Mintaka | 522 |

Only Arachne, Manticora and Ophidian carry preview tints. The rest use the neutral default, so the species comes from the label. Thalassa's "pearl" settings are modelled as small faceted rounds.

## `gems/`: 20 cut meshes

Copied from RingDesigner's `bundled/gems/*.obj`. Every file is headed `# CrossGems gem mesh: <cut>`, so the geometry appears to come from the CrossGems Rhino plug-in. RingDesigner ships these files in its repo, but their upstream licence isn't documented; use them for local testing only.

- **Shape and scale:** unit-normalised, with a plan extent of 1 along the longer axis (`hexagonal` is 1.155 across its corners). The girdle sits at about z = 0, with the crown along +Z and the culet along −Z.
- **Topology:** mixed triangles and quads, with vertices repeated per facet. Every file is closed and outward-wound once welded.
- **Mapping:** RingDesigner maps 14 of them to its cuts (`brilliant` = Round, plus oval, cushion, princess, emerald, baguette, pear, marquise, trillion, heart, radiant, asscher, hexagonal and half-moon). `barion`, `calf`, `flower`, `octogonal`, `trilliant` and `trillion-straight` are extra cuts that RingDesigner doesn't map.

| File | Triangles | x × y × z extent |
|---|---:|---|
| `asscher.obj` | 126 | 1.000 × 1.000 × 0.640 |
| `baguette.obj` | 48 | 0.625 × 1.000 × 0.381 |
| `barion.obj` | 142 | 1.000 × 1.000 × 0.600 |
| `brilliant.obj` | 174 | 1.000 × 1.000 × 0.600 |
| `calf.obj` | 72 | 1.000 × 1.000 × 0.490 |
| `cushion.obj` | 176 | 1.000 × 1.000 × 0.600 |
| `emerald.obj` | 136 | 0.666 × 1.000 × 0.440 |
| `flower.obj` | 196 | 1.000 × 1.000 × 0.574 |
| `half-moon.obj` | 106 | 1.000 × 0.576 × 0.460 |
| `heart.obj` | 222 | 0.998 × 0.997 × 0.600 |
| `hexagonal.obj` | 70 | 1.000 × 1.155 × 0.661 |
| `marquise.obj` | 168 | 0.500 × 1.000 × 0.283 |
| `octogonal.obj` | 110 | 1.000 × 1.000 × 0.640 |
| `oval.obj` | 174 | 0.624 × 1.000 × 0.381 |
| `pear.obj` | 172 | 0.624 × 1.000 × 0.381 |
| `princess.obj` | 86 | 1.000 × 1.000 × 0.745 |
| `radiant.obj` | 102 | 1.000 × 1.000 × 0.710 |
| `trilliant.obj` | 58 | 1.000 × 1.000 × 0.452 |
| `trillion-straight.obj` | 82 | 1.000 × 1.000 × 0.500 |
| `trillion.obj` | 118 | 1.000 × 1.000 × 0.500 |

## `test/`

| File | Source | Licence | Triangles | Bytes |
|---|---|---|---:|---:|
| `vhm_skull_nih3d_3DPX-012260.ply` | "Visible Human Male Skull" by kbrowne, NIH 3D entry [3DPX-012260](https://3d.nih.gov/entries/3DPX-012260) | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | 580,286 | 11,024,810 |

**Attribution:** "Visible Human Male Skull" by kbrowne, NIH 3D (3DPX-012260), licensed CC BY 4.0. It is derived from the NLM Visible Human Project male data set.

**What was changed:** the entry's `vhm_skull_0-nih3d.x3d` (sha256 `1b5aa287f7745473dfa43d17589a8cbe17cae58909faa8c6371bf901ffef9eab`) was converted to binary little-endian PLY. Vertex colours were dropped and the geometry is unchanged.

**What to know before using it:**

- **Size:** 290,065 vertices, wound outward.
- **Defects:** 7 open edges and 5 non-manifold edges. The NIH page reports its WRL export as watertight.
- **Units:** not stated. The bounding box is 2.38 × 3.57 × 3.32, probably inches. Rescale it to a ring head of about 15–20 mm on load.
- **Purpose:** it stands in for the head of a skull ring.

## How these were made

- **CLI build:** in the RingDesigner clone, `CARGO_TARGET_DIR=$HOME/argent/target-ringdesigner CARGO_BUILD_JOBS=12 nice -n 10 cargo build --release -p ringdesign-cli`. This took 1 min 11 s wall time on a fresh target directory, with default features and no system libraries needed.
- **Exporter:** [`ringdesigner/tools/rd-finished-export`](ringdesigner/tools/rd-finished-export) is a small crate linked to the clone's `ringdesign-core` (45 s build into the same target directory). It exists because `ringdesign export` has two gaps:
  - Its STL is `try_build_pattern`, the **casting pattern**. That pattern drops bench-only layers on every design, and on sand designs it also drops made settings, leaving a drill-start dot in their place. Ophidian's pattern has 1,474,560 triangles and 1,060 mm³ with the amethyst seat filled; the finished ring has 1,454,442 triangles and 974 mm³.
  - It never writes stones.
- **Per design:**
  1. `rd-finished-export showcase/<src>/design.ring.json --out ringdesigner/<dir> --steps <Steps>` writes the STL, `_stones.obj` and `_stones.json` (0.2–22 s each).
  2. For stone-set designs, `ringdesign export showcase/<src>/design.ring.json --formats glb --steps <Steps> --out <tmp>` writes the GLB, which is then moved in (6–42 s each).
- **GLB exception:** the CLI's GLB is the finished ring only when a design has made settings, stamps or CAD parts. Vesper and Nocturne (stock) differ from their pattern only by bench-only layers ("Silk ground / bench finish", "Palm signature / bench engraving"), so the CLI wrote their **pattern** (1,696,600 and 1,603,742 triangles). Their GLBs here come from `rd-finished-export … --glb` instead. That option calls the same `gltf::write_glb` on the finished mesh, and on Ophidian its output is byte-identical to the CLI's GLB.
- **Determinism:** re-running the exporter reproduced the STL byte for byte on the three designs checked (Ophidian, Vesper and Nocturne-stock).

To rebuild the exporter, run `CARGO_TARGET_DIR=$HOME/argent/target-ringdesigner cargo build --release` inside `ringdesigner/tools/rd-finished-export`. It runs as `$HOME/argent/target-ringdesigner/release/rd-finished-export <design.ring.json> --out <dir> [--steps TxP] [--size US] [--glb]`.
