# Research — lighting-scale--lightmap-layer-budget-gate

Read at 45c4fe5.

## What `MAX_ATLAS_LAYERS` guards

- Shipped sections address lightmap data by cell block only:
  - id 22: `block_count` u32, capped at `MAX_LIGHTMAP_BLOCKS` = u16::MAX by the vertex `id + 1` field.
  - id 42: SMB7 per-block BC5 groups.
  - id 25: keys by id-22 block.
  - No bake-layer index or count ships.
- Runtime places blocks into its own pool of `LIGHTMAP_POOL_LAYER_EDGE` = 2048 layers.
  - All-resident mode (`plan.rs::plan_static_pool`) rejects a level whose pool layers exceed the device's `max_texture_array_layers`, logs an error and binds a placeholder.
  - Streaming defers instead.
- The 256 value matches `prm.rs::PORTABLE_MAX_TEXTURE_ARRAY_LAYERS` and the renderer's required floor (`renderer_init_resources.rs::REQUIRED_MAX_TEXTURE_ARRAY_LAYERS`, requested as the device limit). It dates from when the atlas shipped whole: `plans/done/lightmap-array-atlas` added the constant beside the renderer's `max_texture_array_layers` floor, and `context/research/spatial-streaming.md` names it with the device limit. Its doc comment now gives only compiler reasons (cache partition unit, one-plane working set).
- Bake-layer edge is the smallest power of two that holds the largest block (`atlas_pack.rs::choose_layer_dim`). `check_planned_area` assumes 2048² per layer, so with small blocks the pack, not the area check, overflows first.
- `pack_groups_into_layers` reports `layer + 1` at the first overflow, so the error always reads "257".

## Measured: hallway map

`content/dev/maps/stress-warren-hallway-inspection.map`, debug `prl-build`, warm SH cache. Each run stopped at the `-v` predicted-peak line.

| density m/texel | bake layers (2048²) | blocks | predicted lightmap-stage peak |
|---|---|---|---|
| 0.05 | 62 | 2080 | 3058 MiB |
| 0.04 (default) | 104 | 2080 | 4445 MiB |
| 0.03 | 197 | 2082 | 7487 MiB |
| 0.027 | 249 | 2085 | 9095 MiB |
| 0.0265 / 0.026 / 0.025 | overflow | passed | — |

- Fit at 0.025: about 265–290 layers.
- Map scale:
  - 764 lights.
  - 2,082 interior leaves and 8,445 faces.
  - At 0.025, 12 faces are cut into 24 sub-faces.
- Cold compile: 1112 s to the failure. SH stages take ~950 s and set the observed 5.46 GiB RSS.
- Warm reruns (SH cache hit) reach the predicted-peak line or the overflow in 82–98 s wall, debug, max RSS ~5.5 GiB. Only the first cold build pays the SH stages before the lightmap gate can run.

### Peak decomposition (`peak.rs::predicted_peak`, layer texels 2048² = 4,194,304)

| term | per | 0.04 (104 layers) | 0.027 (249 layers) |
|---|---|---|---|
| shadowmask fill | 4 B × layer texels × layers | 1664 MiB | 3984 MiB |
| face map | 4 B × layer texels | 16 | 16 |
| layer plane | 57 B × layer texels | 228 | 228 |
| partitions | 16 B × layer texels × window 8 | 512 | 512 |
| sections ×2 | encoded block bytes | ~2025 | ~4355 |

- Sections grow with shipped texel area (∝ 1/d²); the shadowmask fill grows with layer count.
- Projected at 0.025 with ~280 layers:
  - ~10.4 GiB as predicted today.
  - ~5.9 GiB once the fill is per-layer.
  - ~3.3 GiB once the fill is per-layer and each section is counted once, plus the cut-face carry.

## Section copies at the cache write

- `predicted_peak` sets `sections: 2 * encoded` over ids 22 and 42: the section beside its cache copy.
- Id 22: `pipeline/lightmap_stage.rs` calls `cache.put(key, &section.to_bytes())`, a second contiguous copy. The shadowmask raw fill is still live then; `shadowmask.finish` runs after.
- Id 42 already streams: `shadowmask_bake/memo.rs::cache_shadowmask_section` writes the report, a presence byte, `index_bytes`, then each block's groups through `put_streamed`. Test seams `SHADOWMASK_STREAMED_CACHE_WRITE_COUNT` and `RAW_FILL_LIVE_BYTES_AT_CACHE_WRITE`.
- The predicted term therefore overcounts id 42 today; only id 22 is truly held twice.
- Warm memo hit: `cache.get` returns the whole entry as one `Vec`, and `LightmapSection::from_bytes` copies every blob, so entry bytes and the parsed section coexist inside the lightmap memo read (`pipeline/lightmap_stage.rs`). The shadowmask memo read has the same shape. Accepted and unpriced: `development_guide.md` §1.4 lets warm iteration approximate; the cold build is the ship source of truth.
- `cache.rs::put_streamed(key, payload_len, write_payload)` stages the same header and blake3 payload hash as `put`; length mismatch and I/O failure follow `put`'s logging and cleanup.
- `LightmapSection::to_bytes` writes header, records, then each block's irradiance and direction. `byte_len` exists; no index-only writer does.
- Commitment: `plans/done/lighting-scale--shadowmask-cold-working-set` ("one full-size copy of the section exists at a time, from fill through write"); `development_guide.md` §1.4 (count coexisting representations; persistence can duplicate output).
- PRL write: `pack/section_plan.rs` plans ids 22 and 42 as `|| Ok(section.to_bytes())`, a contiguous copy beside the live section. `pack_output.rs::PlannedSection::with_writer` streams a section into the file; `pack/cluster_sh_payloads.rs` already uses it.
- PRL read-back: `pack_output/readback.rs::validate_readback` reads each section except id 50 into one whole buffer through `read_section_data`, beside the live section, only to check its framed length (and id 49's structure). Id 50 validates streamed (`validate_cluster_sh_payloads_readback`).

## Shadowmask fill lifetime

- Both walks are layer-major (`pipeline/lightmap_stage.rs` `items`, `window::consume_in_order`). `LayerFaceMap` already keeps one layer's map live.
- `encode_blocks_bc5` loops per layer.
- Cross-layer work is limited to cut faces in `OwnershipFill::finish`:
  - `release_owners_that_baked_nothing` reads `cuts[c].exact`, summed across sub-faces.
  - The cut's `ascending_order` drives `reorder_channels` and `dilate_owned_padding` on every sub-face.
- Faces are cut only past the pool edge (`plan_face_cuts`), so sub-faces typically land on different layers.

## Cache density

- The reach cull is per chart (`plan_charts` → `light_may_reach`).
- `partitions.rs::finish` still `cache.put`s every (layer, light) item, including zero-texel items with a 20-byte header.
- Entry count is layers × non-SDF static lights: ~280 × 764 ≈ 214k at 0.025.
- The cache bounds bytes only (`DEFAULT_MAX_BYTES` 2 GiB); entry count is reported, never capped.
- The entry count is calculated, not measured as a cost.
- The shadowmask section key folds every eligible light's per-layer hash (`shadowmask_bake.rs`, `eligible_layer_hashes`). Dropping empty pairs must keep it stable.
- Split out of this brief as a separate direct build.

## Existing gates

- `pipeline.rs::gate_delta_working_set`: SH-delta CSRs only (ids 27/41/45).
  - Budget `--sh-delta-working-set-max-size`, default 16 GiB (`delta_sections.rs::DEFAULT_MAX_WORKING_SET_BYTES`).
  - Runs cold and warm, before atlas preparation.
  - Deliberately conservative: `delta_working_set_copy_chain_factor` treats all three dense payloads as co-resident through compaction and reserves headroom for base-volume originals and clones.
- `peak.rs::predicted_peak` is itemized by term; no headroom factor.
- Neither estimator predicts process RSS. Each prices its own stage's working set; earlier stages' outputs stay resident beside it (`delta_sections` is still live and passed into the fused stage, and the SH stages set the hallway's RSS).
- `log_predicted_peak` runs only under `-v`, after `prepare_atlas_stage`. Nothing gates on it.

## Other `MAX_ATLAS_LAYERS` consumers

- `check_planned_area`: the P11 test (`atlas_stage/tests.rs`) relies on it to stop a scale region cut into millions of pieces.
- `lightmap_residency_dry_run/layouts.rs::RUNTIME_LAYER_FLOOR` aliases it. Its label means the device floor.
- `pack_layers_overflows_past_max_atlas_layers` (`lightmap_bake/tests.rs`).
- `atlas_layout.rs::pack_cut_charts`, no-static-light path: any `pack_cell_blocks_within` error, layer overflow included, returns no placements, so the animated passes plan no chunks. A chart past the pool edge also returns none (no cuts without static lights), and so does a block-count overflow. Only the layer-overflow arm is the cap's. The fused stage then returns before allocating anything and ships an empty id 22 (`pipeline/lightmap_stage.rs`).
- No u8/u16 narrowing of layer counts anywhere: shadowmask atlas, animated weight-map baker, `LightmapLayer.layer_count`, `CompositedAtlas`.

## Runtime pool-layer count at compile time

- All-resident mode: `plan.rs::plan_static_pool` places blocks with `postretro_render_cpu::lightmap_pool::place_all_resident(extents, header.block_alignment(), LIGHTMAP_POOL_LAYER_EDGE)`, then `array_layers_fit` rejects a count past the device's `max_texture_array_layers` and binds the placeholder.
- The mode applies to a level with blocks that lacks a usable id 51 or usable portals, or under `POSTRETRO_LIGHTMAP_STREAMING=all-resident` (`rendering_pipeline.md` §4).
- The compiler holds every input after the block pack: block extents and the id-22 header's `block_alignment` (`lightmap.rs`, from `direction_texel_scale`).
- `place_all_resident` lives in `postretro-render-cpu`, which `level-compiler` takes as a dev-dependency only. The dry run that reuses it (`layouts.rs`, `cell_blocks.rs`, `block_pool_sim.rs`) is `#[cfg(test)]` in `main.rs`.
- `BlockPool` (`lightmap_pool/block_allocator.rs`) has no imports; `place_all_resident` needs only it. `postretro-level-format` already owns `LIGHTMAP_POOL_LAYER_EDGE`.
- `BlockPool` also backs the runtime streaming policy (`render-cpu` `lightmap_pool/policy`), with free and restore beyond what the all-resident walk uses.
- A normal compiler → render-cpu edge adds no wgpu and no external crate (render-cpu is CPU-only), but puts `level-compiler` above render-cpu (both layer 3 in `crate-graph.md`) and makes `prl-build` recompile on every `entities` change, the chokepoint `development_guide.md` §Workspace names.

### Whether the level will stream

- Runtime mode rule: `level-loader` `lightmap_stream/storage.rs::select_lightmap_residency`. Stream needs streaming requested, usable portals, an id-51 section, a retained file and at least one block (`LightmapResidencyInputs::blocker`).
- Usable portals at load: `prl_loader.rs::convert_usable_portals` — a non-empty Portals section whose every portal has at least three in-range, finite vertices and non-zero area. A malformed section falls back to no portals.
- Usable id 51 at load: present. A present section that fails to parse against the cell count is a load error, not a fallback.
- Retained file: `prl_streaming.rs::load_prl_with_modes` forces the positional container whenever streaming is requested and the table holds ids 22, 51 and Portals (`table_may_stream_lightmaps`). A compiled level with all three always has one.
- Compiler: `cell_residency_bake.rs::cell_residency_set_bake_cached` returns `None` (id 51 omitted) exactly when `residency_world` finds no portals or `LevelWorld::visibility_only_from_sections` reports none usable. That constructor calls the loader's `convert_usable_portals`, so the compiler applies the loader's own test to the Cells and Portals it ships.
- So a level with blocks and no id 51 always loads all-resident; one with id 51 streams unless `POSTRETRO_LIGHTMAP_STREAMING=all-resident`.
- Ordering: the CellResidencySet stage runs after atlas preparation and before the fused lightmap stage (`pipeline.rs`), so the check has the packed layout and the id-51 result before any lightmap bake. On a warm build id 51 comes from its own memo with the same `Option`.

## Pre-cut lower bound

- After the per-layer fill, only the sections term of `peak.rs::predicted_peak` grows with texel area. The fill (one layer plus cut carry), face map, layer plane and partition window are per layer or per window.
- Per block, `predicted_peak` charges `w·h·irr + (w/s)·(h/s)·2 + 2·w·h`: irradiance (`irr` = 1 B, 8 B under `--uncompressed-irradiance`), direction Rg8 at the reduced scale `s`, and the two BC5 shadowmask groups at 1 B/texel each.
- Floor per texel: `irr` alone, 1 B or 8 B. Id 22 ships whenever blocks exist; id 42 does not ship on a map with no owner-eligible light (`shadowmask_bake.rs`), and direction is non-negative, so neither enters the floor.
- Area: `face_cut.rs::planned_chart_area` counts every grid window of a cut face, including windows its polygon misses, which `apply_face_cuts` never emits (`build_pipeline.md` documents the overcount). The bound instead counts an uncut chart's padded extent and a cut face's own polygon area at its resolved density; every emitted sub-chart covers its share of the polygon, so packed block area is at least the bound's area. The bound therefore never exceeds the predicted working set (pin P14).
- P11: scale 2000 plans ~3.2e10 texels; its faces fill their windows, so ~30 GiB at 1 B, refused at the 16 GiB default. 1e6 and 1e7 refuse the same way; their products fit in a u64.
- At the default budget the bound alone admits ~17 G texels at 1 B.

## Scale-ceiling inventory (holistic context; not this brief's scope)

| class | ceilings | owner |
|---|---|---|
| Count standing in for compiler memory | `MAX_ATLAS_LAYERS` | this brief |
| Whole-map GPU resource, not streamed | animated lightmap atlas (`ANIMATED_ATLAS_VRAM_BUDGET_BYTES` 1 GiB; `ANIMATED_BLOCK_CAP` 8,190); SDF atlas (3D 2048, no guard); SH depth-moment 3D texture (2048/axis, limit never requested), probe indirection and compose metadata buffers; billboard scatter volume; legacy SH / all-resident lightmap | `plans/large-map-spatial-residency.md` |
| Format field width | u16 lightmap block id; `MAX_CELL_ID_EXCLUSIVE` 131,072; u16 shadowmask owner; SH indirection 2²⁷ | keep; named error each; widen on measured need |
| Fails badly, not by name | SH atlas tiles panic (`sh_bake.rs`, `direct_sh_bake.rs`, `sh_group.rs` `unwrap_or_else(panic!)`); SDF atlas unguarded (wgpu validation); `MAX_ANIMATED_BAKED_LIGHTS` 256 silent (no promotion past index 256) | separate sweep |

## Pin table

Orderings the Acceptance rows cite.

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Warm cache whose lightmap_section and shadowmask_atlas memos both hit; budget one byte below the prediction | block pack → gate → shadowmask memo probe → cut-face estimate → lightmap memo probe → walk | Refuses with the same error as `--no-cache`, before the lightmap stage bakes: no lightmap_section, shadowmask_atlas or lightmap_layer read, no cut-face estimate trace, no chart bake. SH stages have already run |
| P2 | Window holds layer L+1 partitions while layer L is open | consumer takes L+1's first item → L's uncut faces finish, its blocks with no open cut encode, its raw layer releases → L+1's raw layer allocates → the item folds | Two raw layers are never live outside the carry; resident L+1 partitions do not open L+1's fill early |
| P3 | Cut face whose sub-faces share one bake layer | finishes with that layer's close | No carry; bytes as today |
| P4 | Cut face whose first sub-face's layer closes before its last sub-face's layer opens, up to spanning the first and last layer | its blocks stay raw through the layers between → at the last sub-face's layer close: release owners that baked nothing over the summed lit counts, ascending reorder, padding dilate, then encode and release every block it held | Id 42, owners and drop report equal pre-change; carry is that cut's blocks only, for the whole walk in the first-to-last case |
| P5 | Bake layer that no eligible light lights | its close encodes a zero fill | Its blocks still ship both BC5 groups, equal to the pre-change encode |
| P6 | No face owned on any layer, including a cut whose owners all baked nothing | layers encode as they close; ownership is known only at the last close | No id-42 section, as today |
| P7 | Lightmap memo hit, shadowmask memo miss | reuse pass over owner-eligible items | Same layer close and carry as the compose pass |
| P8 | No static lights, or no placed chart, at any bake-layer count | the fused stage returns before allocating | The gate predicts zero and never refuses. No placements and no animated chunks are planned, at any chart area |
| P9 | Stage end on a cache miss | walk drains → last lightmap layer closes → id-22 streamed write → shadowmask closes its last layer, encodes, writes id 42 | At the id-22 write, live raw fill is at most one layer plus the carry of cuts ending on the last layer, beside one id-22 copy |
| P10 | Warm build: shadowmask memo hit, then lightmap memo hit | id-42 decode → id-22 entry read → id-22 decode → entry bytes drop | Accepted warm-path overhead (`development_guide.md` §1.4): entry bytes and decoded id 22 coexist briefly beside decoded id 42, unpriced by the gate |
| P11 | P11 test scales 2000, 1e6, 1e7, three-leaf fixture, 2048 pool edge | chart plan → pre-cut lower bound against the budget → cut | At the default budget every scale fails by name before cutting, under 5 s |
| P12 | --working-set-max-size between the SH-delta projection and the lightmap prediction; and below both | SH-delta gate → base SH → … → atlas preparation → lightmap gate | Between: SH-delta logs admitted, then the lightmap gate refuses. Below both: SH-delta refuses and the lightmap gate never runs |
| P13 | Warm build whose lightmap_section memo hits, on a layout past the floor | block pack → id-51 bake or memo hit → pool-layer check → lightmap memo probe | No id 51: the same refusal as the cold build, before the memo probe. Id 51 present: the same warning as the cold build, once |
| P14 | Planned area whose minimum cost equals the budget | chart plan → pre-cut lower bound passes → cut → pack → gate | The gate then decides on the predicted working set, which is never below the lower bound |
