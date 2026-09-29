# Research: lightmap cell blocks

Derivation companion to `index.md`. Read at `10ceddb4f`. Tables already in `context/plans/large-map-spatial-residency.md` §Pre-planning measurements (the seed) are linked, not copied. Only figures the seed lacks are added here.

Dry-run outputs, all in the session scratchpad (`$SCRATCH` below):
`/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/d165815d-0a78-4d1a-8d9f-3589e788bcb2/scratchpad/`.
Hallway = `stress-warren-hallway-inspection`. Campaign = `campaign-test`.

## 1. Measured basis

Cell-granular M(c), 3×3×3 inset lattice. Layers are 2048². Pool layer = 14.0 MiB (ids 22 + 42).

| Figure | Hallway | Campaign | Source (`$SCRATCH/`) |
|---|---|---|---|
| Mandatory worst / p95 MiB, L = 0 | 97.0 / 35.3 | 29.4 / 25.4 | `cellblocks-hallway.txt`, `cellblocks-campaign-test.txt` |
| L = 16 m | 111.7 / 64.2 | 37.1 / 37.1 | same |
| L = 32 m | 125.7 / 94.1 | 43.4 / 43.4 | same |
| Static pool layers worst / p95, L = 0 / 16 / 32 | 8/3, 9/6, 10/8 | 3/2, 3/3, 4/4 | same |
| Shelf from scratch, L = 16 | 9 max, 6 p95 | 4 max, 4 p95 | same |
| Block overhead (block / chart texels) | 1.234× (p50 1.147×, p95 2.288×) | 1.381× (p50 1.163×, p95 4.118×) | same |
| Largest block | 1144×1844 | 696×1044 | same |
| Sightline p50 / p95 / max | 71.6 / 127.1 / 237.5 m | 60.5 / 87.2 / 99.5 m | `final-stress-warren-hallway-inspection.txt`, `final-campaign-test.txt` |

Pool walks at L = 16 m, shelf allocator. Cells are repack (defrag) steps as a share of 20,000 steps, random walk / far-point tour. The uncapped immediate-free peak is 10 / 15 layers on the hallway and 7 / 9 on campaign.

| Pool | Immediate free | LRU | Hard-fail steps |
|---|---|---|---|
| Hallway 9 layers (126 MiB) | 0.01% / 3.65% | 0.20% / 5.22% | 0 |
| Hallway 12 layers (168 MiB) | 0.00% / 0.30% | 0.03% / 1.33% | 0 |
| Campaign 3 layers (42 MiB) | 9.87% / 32.77% | 5.00% / 25.85% | 591 / 3,308 |
| Campaign 4 layers (56 MiB) | 0.77% / 5.24% | 0.04% / 3.34% | 0 |

Source: `cellblocks-hallway.txt` lines 470-482, `cellblocks-campaign-test.txt` lines 289-301. Pools are 100% and 125% of the static worst.

- The hypothesis "at most 12 layers at L = 16 m" is a pool cap, not a peak. The uncapped tour peaks at 15, so the cap must hold by repack, not by luck.
- Immediate free beats LRU on repack rate at every pool measured. That is the basis for the brief's default.
- The walks use a shelf allocator. Guillotine and merging allocators are unmeasured.

## 2. Rejected shapes with numbers

Tables in the seed. Deciding number per shape (hallway, L = 32 m unless noted):

| Shape | Deciding number |
|---|---|
| Whole layers, cluster-keyed | 420 MiB worst at L = 16, 512 cells over Low; 518 MiB at L = 32 |
| Whole layers, cell-granular | 280 MiB worst at L = 16, 7 cells over Low; today's packing does not fit |
| Fixed tiles | Oversize charts (larger than a tile) hold 91.7% of chart texels at P = 128, 72.4% at 256, 31.7% at 512. They force chart splitting. Tile/exact 1.72× (cell, 128) to 4.10× (cell, 512). |
| Cluster closure | 165.4 MiB texel-exact worst vs 96.4 for cells, 1.7×. Cluster P = 128 tiles still exceed Low on 6 cells. |
| Fragment-stage virtual texturing | Needs a fragment binding. Forward FRAGMENT sits at 8/8 storage and 15/16 sampled. Also needs chart splitting (the 91.7% above). |
| `first_instance` cell identity | Not requested as a device feature. DX12 reads it as 0 (gfx-rs/wgpu#2471). Not re-verified in this pass. |

Cell blocks fit Low at every measured L (worst 125.7 MiB vs 256). No block exceeds 2048 in either dimension.

## 3. Blast radius of block-local UVs

Block-local UVs and block ids replace static-atlas texel coordinates. Every reader or writer of that frame changes.

**Shaders** (`crates/renderer/src/shaders/forward.wgsl`)

| Symbol | Change |
|---|---|
| `vs_main` | Decodes the packed UV and static-layer field (`lightmap_uv_packed`, location 4). Now reads the block table and emits block-local texel, flat pool layer and offset. |
| `sample_lightmap_irradiance` | Takes pool-relative UV and layer. |
| Direction `textureSample` (`lightmap_direction`) | Same UV and layer change. |
| `sample_shadowmask_atlas` | Clamps to half-texels of the whole layer today. Must clamp to the block. Callers: `shadowmask_union_subtraction` and the specular pass. |
| `animated_block_uv` | Computes `static_uv * static_layer_size + offset`. Must rebase to block-local texels. |

**Loader** (`crates/level-loader/src/`)

| Symbol | Change |
|---|---|
| `prl_animated_atlas.rs`: `first_block_id_mismatch`, `static_texel_inside_block`, `usable_static_layer_size` | Static-atlas assumptions. Rewrite over block extents. |
| `prl_loader.rs`, id 42 check (width, height, layer_count equal id 22's `irr_*`) | Becomes block-count equality. |

**Renderer** (`crates/renderer/src/`)

| Symbol | Change |
|---|---|
| `lighting/lightmap.rs::usable_static_layers` | Feeds `AnimatedLightmapResources::new` and `validate_cross_section` (`render/animated_lightmap.rs`). Layer count becomes pool layers. |
| `filter_usable_section`, `filter_usable_shadowmask_section` | Validate whole-layer payload shape. |
| `upload_irradiance_texture`, `upload_direction_texture`, `upload_shadowmask_texture` (+ placeholders) | Whole-payload upload becomes per-block region upload. |
| `render/lightmap_residency.rs` | Residency report grows real state. |

**Compiler** (`crates/level-compiler/src/`)

| Symbol | Change |
|---|---|
| `lightmap_bake.rs`: `prepare_atlas` → `pack_layers` / `pack_layers_with_layer_limit` → `assign_lightmap_uvs` | Pack per cell block. |
| `scatter_chart_into_atlas`, `bake_atlas_layer_controlled`, `encode_atlas_layer`, `assemble_layered_section` | Placement and encode by block. `dir_width = atlas_w / direction_texel_scale` becomes per-block. |
| `shadowmask_bake/encode.rs::encode_side_by_side_bc5`, `fill.rs::shadowmask_data_offset` | Layer offsets become block offsets. |
| `lightmap_layer.rs::SharedAtlas`; `shadowmask_bake.rs` width check | Atlas model and width equality. |
| `animated_light_chunks::build_animated_light_chunks`, `animated_light_weight_maps::chunk_atlas_rect`, `assert_no_overlapping_rects_per_layer`, `AnimatedBlock{static_layer, static_x, static_y}` | Rebase to block id + local x/y. |
| `animated_atlas_layout::choose_compact_layout`, `identity_placements` | Consume the rebased blocks. |
| `animated_block_ids::validate_block_guards`, `check_vertex_footprint` | Vertex footprint moves to block frame. |
| `pipeline.rs::layout_animated_atlas` | Call site. |

**Bake seeds.** `texel_seed(atlas_x, atlas_y)` (`lightmap_bake.rs`) and `soft_visibility_texel_seed(ax, ay)` (`animated_light_weight_maps.rs`) key noise on atlas coordinates. A repack changes bake noise. The brief's non-goal accepts that.

**Cache keys.**
- `atlas_layout_fingerprint` (`lightmap_layer.rs`) feeds `layer_input_hash` and `section_input_hash`, so layers and sections rebake.
- The SDF key hashes the whole post-`prepare_atlas` `geo_result` (`pipeline.rs`, comment above `stamp_animated_block_ids`). A UV repack misses the SDF cache. That costs a rebake, not correctness.

**Unaffected**
- `depth_prepass.wgsl` declares location 4 and never reads it.
- `kinematic_brush.wgsl` and the skinned shaders have no lightmap attribute.
- `billboard.wgsl`, `wireframe.wgsl`: only the inert `spec_shadowmask_force_one` flag.
- No CPU lightmap sampling at runtime.
- CELL_VISIBILITY, cell draw index and SH bakes all run before `prepare_atlas`.

## 4. Version constants to bump

| Constant | Where | Now |
|---|---|---|
| `LIGHTMAP_SECTION_VERSION` (id 22) | `level-format/src/lightmap.rs` | 2 |
| `LIGHTMAP_SECTION_VERSION` | `level-compiler/src/lightmap_layer.rs` | 3 |
| `LAYER_FORMAT_VERSION` | `level-compiler/src/lightmap_layer.rs` | 6 |
| `SHADOWMASK_ATLAS_STAGE_VERSION` (id 42) | `level-compiler/src/shadowmask_bake.rs` | 4 |
| id-42 container version | `pack/section_plan.rs` | 2 |
| `ANIMATED_LIGHT_WEIGHT_MAPS_VERSION` (id 25) | `level-format/src/animated_light_weight_maps.rs` | 4 |
| `STAGE_VERSION` (id 25 stage) | `level-compiler/src/animated_light_weight_maps.rs` | 7 |

- Id 17 Geometry has no section version constant. Its container version is 1 in `section_plan.rs`. The global `CURRENT_VERSION` is 4 (`level-format/src/container.rs`). Bump whichever gates the vertex-field meaning.
- Id 22's container version is 1 and id 25's is 1 in `section_plan.rs`. Their section constants carry the change.
- `CLUSTER_DIRECTORY_VERSION` (id 49, currently 2) is untouched.
- Loader rejects older versions (see index Acceptance).

## 5. Lifetimes

Per `development_guide.md` §1.4 and `testing_guide.md` §Resource bounds.

- `GpuLightingPayloads` (`level-loader/src/prl_lighting.rs`) holds the whole id 22 and id 42 payloads (`Vec<u8>`) until upload. Streaming mode must not construct it.
- The loader reads each section whole into a `Vec`: `read_vec_at` (`sh_stream/positional_io.rs`), through `PrlContainer::from_positional` or `from_whole_bytes` (`prl_container.rs`). There is no mmap.
- `ShStreamManifest` retains an `Arc<File>` for positional reads (`sh_stream/manifest.rs`). A PRL without id 50 retains no handle. Block streaming needs its own retained handle or a shared manifest.

Representations a build must count, each with an owner and a release point:

| Representation | Notes |
|---|---|
| Block index | Header and records of ids 22/42, kept CPU-side for the life of the level. |
| In-flight read buffers | One per issued block read, until installed or discarded. |
| Staging upload | `StagedUploads::write_texture` region copies awaiting submit. |
| Active pool | Irradiance, direction and shadowmask textures. |
| Retiring pool | Alive during growth or repack until submitted-work-done. |
| Residency set | Baked per-camera-cell entries plus runtime state. |

## 6. Shared-issuer extraction notes

SH-specific today:

- `ShWorkerSource` trait (`session/sh_async_workers/`), used by `issuer.rs`.
- `ShClusterRequest {generation, content_tag, cluster_id, chunk_hash, mandatory}` (`sh_streaming/controller.rs`).
- Target bitset sized by `cluster_count`.
- Thread names `sh-probe-read` and `sh-probe-decode-{index}` (`sh_async_workers/mod.rs`).
- `MAX_INSTALL_DECODED_BYTES_PER_DRAIN = 8 MiB` (`controller.rs`), applied by `sh_streaming/lifecycle.rs::select_install_budget`. The first request is always admitted.
- `TargetClass` order: Visible, Pinned, SeamWarm, Prefetch, Hysteresis.
- Stale-result gate: generation, content tag, target membership and chunk hash must all match.
- `renderer/src/render/sh_streaming/install_journal.rs` undoes newest-first.
- `sh_streaming/gpu/growth.rs` keeps one active and one retiring generation.

Blocks are raw BC, so they carry no decoded-byte cost. The budget unit for a block is its upload bytes. That choice, and how demands interleave, is delegated (index Open questions).

`drafts/sh-streaming--reveal-gate-and-warm-horizon` expects lightmap streaming to join its one settle chokepoint. Its non-goals list streaming lightmap-shaped data. Both drafts must agree on the settled check's per-resource question.

## 7. Stated divergences

| From | Divergence |
|---|---|
| `plans/done/sh-probe-streaming` Slice 2 | That slice makes the cluster directory the substrate later resources key on. Blocks key on cells. Id 49 stays unchanged. |
| Seed stage 5, "Generalize the same cluster state to lightmap-shaped layers" | Blocks generalize the issuer and budget, not the cluster state. |
| `resource_management.md` §9, texture streaming non-goal | Covers material textures. Promotion amends it to allow baked lighting residency. |
| SH miss policy | SH tolerates a miss with an ambient floor. Mandatory and visible blocks are never refused, and the pool grows past its cap. |

## Pin rows

Orderings the Decisions imply. Acceptance cites them by id.

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Camera-cell change and L change | Same frame | Demand is recomputed once. It equals the new cell's entries with lead ≤ new L. |
| P2 | Pair halves complete apart | Lightmap first, or shadowmask first | Neither half is sampleable. Both install in the drain after the later half completes. |
| P3 | One half completes in generation N | Reload, then the other half completes in N+1 | Both are discarded and their buffers released. No table write. |
| P4 | Block leaves demand while its read is in flight | Read completes after | Discarded and released. If the block re-entered demand first, it installs exactly once. |
| P5 | N requests for one in-flight block | Before completion | One read, one install. |
| P6 | A evicted, B placed in A's region | Same drain | A's entry is non-resident in the same batch that writes B's texels. No frame samples A through B's texels. |
| P7 | Growth or repack while installs are journaled | An install then fails | After undo, every entry addresses its own block in the active pool. The retiring pool is released only on submitted-work-done. |
| P8 | Pool cap lowered below residency | Next drain | Optional blocks are evicted down to the cap. No mandatory or visible block is evicted. |
| P9 | Reload while a retiring pool is alive | Before release | Both pools, the workers and the handle are released. No old-level entry reaches the new level's first frame. |
| P10 | Spawn cell has an empty baked range | Level install | Install completes with nothing resident. The first frame renders without waiting. |
| P11 | Portal-walk frame draws a cell outside the baked set | Any frame, including the first | The block is demanded as visible and never refused. It renders SH-only until its pair installs, and counts as a miss. |
| P12 | SH plus block mandatory demand exceeds one drain | Consecutive drains | Each drain admits at least one request. A pair is admitted or deferred whole. Neither resource's mandatory demand starves. |

## Other pins

| Fact | Value | Source |
|---|---|---|
| Lightmap-shaped bytes outside this brief (animated atlas + id-25 weight maps) | 144 + 35.4 MiB on the hallway map; the same on `campaign-test` | seed §Pre-planning measurements |
| Forward pipeline bind groups | groups 0–5 used (`FORWARD_BIND_GROUP_COUNT`); device requests 8; group 6 free | `render/pipeline_layout.rs`, `render/renderer_init_resources.rs` |
| Forward vertex-stage storage buffers | 5 of 8 (group 2) | `pipeline_budget_tests.rs` billboard group-2 inventory |
