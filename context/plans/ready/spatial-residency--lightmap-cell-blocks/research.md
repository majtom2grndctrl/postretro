# Research: lightmap cell blocks

Derivation companion to `index.md`. Read at `10ceddb4f`. Tables already in `context/plans/large-map-spatial-residency.md` §Pre-planning measurements (the seed) are linked, not copied. Only figures the seed lacks are added here.

Dry-run outputs, all in the session scratchpad (`$SCRATCH` below):
`/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/d165815d-0a78-4d1a-8d9f-3589e788bcb2/scratchpad/`.
Hallway = `stress-warren-hallway-inspection`. Campaign = `campaign-test`.

## 1. Measured basis

The brief's set (index Decisions, "Mandatory set"): `M(c, L) = W(c, L) ∪ Dil(PVS(W)) ∪ Pinned`. `W` is the camera cell plus every cell within untruncated portal-path distance L. `PVS` is sampled on a 3×3×3 inset lattice. `Dil` adds one portal hop. There is no camera-cluster term. Neither map has pinned clusters.

The dry run bakes it as the section would: a per-camera-cell lead map to 32 m. The map equals direct evaluation of `M(c, L)` for every camera cell at every L up to 32 m on both maps. The check compares at each breakpoint of either side: 58,540 (camera, lead) pairs on the hallway, 7,570 on campaign (`bandfix-*.txt`).

Layers are 2048². Pool layer = 14.0 MiB (ids 22 + 42). Source: `$SCRATCH/briefset-hallway.txt` lines 486–500, `briefset-campaign-test.txt` lines 305–319.

Cell-block bytes, worst / p95 MiB, and shelf layers packing each set from scratch, worst / p95:

| Set | L | Hallway MiB | Hallway layers | Campaign MiB | Campaign layers |
|---|---|---|---|---|---|
| Dilated (brief) | 0 m | 121.4 / 48.9 | 10 / 5 | 37.0 / 35.0 | 4 / 3 |
| Dilated (brief) | 16 m | 136.5 / 84.7 | 12 / 7 | 44.1 / 44.1 | 4 / 4 |
| Dilated (brief) | 32 m | 170.0 / 120.2 | 14 / 10 | 49.3 / 46.2 | 5 / 4 |
| Undilated | 0 m | 97.0 / 34.6 | 8 / 3 | 27.7 / 25.0 | 3 / 3 |
| Undilated | 16 m | 111.7 / 63.5 | 9 / 6 | 37.1 / 37.1 | 4 / 4 |
| Undilated | 32 m | 125.7 / 94.0 | 10 / 8 | 43.4 / 43.4 | 4 / 4 |

- **Low fits at every lead.** No camera cell exceeds 256 MiB in any row. The worst is 170.0 MiB, on the hallway at L = 32 m.
- **Dilation cost.** On the hallway, dilation adds 22–35% to the worst cell and 28–41% to p95. On campaign it adds 14–33% to the worst and 7–40% to p95. It adds 2, 3 and 4 layers to the hallway's worst shelf count at L = 0, 16 and 32 m.
- **No camera-cluster term.** Dropping it changes little. For comparison, the older cell-granular set (`cellblocks-*.txt`) adds cluster(c) and has no dilation. Its hallway worst matches the undilated rows at every lead: 97.0, 111.7 and 125.7 MiB. p95 falls by at most 0.7 MiB. The one change is campaign's worst at L = 0, which drops from 29.4 to 27.7 MiB.

Prefetch band at L = 16 m, max lead 32 m (dilated):

| Figure | Hallway | Campaign |
|---|---|---|
| Band cells, mean / p95 / max | 47.8 / 109 / 227 | 16.6 / 49 / 68 |
| Band block MiB, worst / p95 | 123.1 / 52.5 | 23.9 / 21.4 |
| Mandatory + band MiB, worst / p95 (= M at 32 m) | 170.0 / 120.2 | 49.3 / 46.2 |

Would-be cell residency section at max lead 32 m. The CSR spans every cell id (5,671 hallway, 464 campaign), with 8 B per entry. No entry names an exterior cell on either map:

| Set | Hallway | Campaign |
|---|---|---|
| Dilated | 322,029 entries (mean 154.7, max 380 per camera cell), 2.48 MiB | 21,000 entries (mean 106.1, max 159), 166 KiB |
| Undilated | 254,261 entries, 1.96 MiB | 19,249 entries, 152 KiB |

Pool walks run the dilated set at L = 16 m with the shelf allocator, over 20,000 steps. Cells show random walk / far-point tour. Source: `$SCRATCH/bandfix-hallway.txt` lines 502–524, `bandfix-campaign-test.txt` lines 321–339.

- A mandatory miss first evicts band blocks, farthest lead first.
- With none left, the pool repacks in place if `M(c, L)` fits the cap. The repack moves `M(c, L)`, then the band blocks resident at step start (nearest lead first), under the cap without reads. Otherwise the pool grows past the cap.
- Band retain keeps resident band blocks under the cap and frees any past it. It prefetches the rest into free space under the cap, but not a block freed in the same step. Immediate free keeps only `M(c, L)`.
- Growth steps are steps where `M(c, L)` opened a layer at or past the cap.
- Hit rate counts blocks joining `M(c, L)` that were already resident. Blocks larger than a pool layer are excluded.
- Demand reads make a mandatory block resident. Read MiB counts demand and prefetch bytes. Thrash reads are demand or prefetch reads of a block freed within the last 8 steps. The window is an arbitrary measurement choice.
- No drain budget is modelled.

| Pool | Policy | Repacks | Growth steps | Hit rate | Demand reads/step | Prefetch reads/step | Read MiB/step | Thrash reads/step |
|---|---|---|---|---|---|---|---|---|
| Hallway 7 (98 MiB, shelf p95) | Immediate | 4.28% / 10.08% | 153 / 828 | — | 8.45 / 15.76 | — | 3.58 / 8.09 | 6.09 / 3.40 |
| | Band | 6.06% / 17.94% | 142 / 904 | 84.3% / 72.1% | 1.33 / 4.41 | 9.40 / 14.70 | 3.70 / 8.89 | 8.22 / 4.87 |
| Hallway 12 (168 MiB, shelf worst) | Immediate | 0.00% / 2.49% | 0 / 0 | — | 8.45 / 15.76 | — | 3.58 / 8.09 | 6.09 / 3.40 |
| | Band | 0.19% / 6.47% | 0 / 0 | 96.9% / 92.4% | 0.26 / 1.20 | 12.84 / 18.55 | 5.42 / 9.91 | 10.42 / 4.60 |
| Hallway 15 (210 MiB, 125%) | Immediate | 0.00% / 0.09% | 0 / 0 | — | 8.45 / 15.76 | — | 3.58 / 8.09 | 6.09 / 3.40 |
| | Band | 0.01% / 2.54% | 0 / 0 | 97.3% / 95.8% | 0.23 / 0.66 | 12.99 / 19.05 | 5.62 / 10.12 | 10.53 / 4.39 |
| Campaign 4 (56 MiB, shelf p95 = worst) | Immediate | 0.24% / 6.61% | 0 / 0 | — | 1.55 / 4.19 | — | 0.52 / 1.32 | 1.20 / 0.96 |
| | Band | 0.51% / 11.38% | 0 / 0 | 98.2% / 92.4% | 0.03 / 0.32 | 2.37 / 3.59 | 0.80 / 1.23 | 1.79 / 1.35 |
| Campaign 5 (70 MiB, 125%) | Immediate | 0.03% / 2.10% | 0 / 0 | — | 1.55 / 4.19 | — | 0.52 / 1.32 | 1.20 / 0.96 |
| | Band | 0.06% / 5.00% | 0 / 0 | 98.8% / 97.0% | 0.02 / 0.13 | 2.48 / 3.81 | 0.86 / 1.25 | 1.86 / 1.39 |

Uncapped peaks are 12 / 17 layers with immediate free and 19 / 22 with band retain on the hallway. On campaign they are 7 / 8 and 7 / 9.

- **12 layers at L = 16 m has no headroom.** The brief set's worst cell alone needs 12 shelf layers at L = 16 m. A 12-layer cap therefore holds by repack: 2.49% of tour steps with immediate free and 6.47% with band retain. At 15 layers (210 MiB) band retain repacks on 2.54% of tour steps.
- **A cap at or above the shelf worst never grows.** A from-scratch repack of any `M(c, 16 m)` fits it. Growth appears only below the worst. At the hallway's p95 cap, `M(c, L)` opens a layer past it on 153 / 828 steps with immediate free and 142 / 904 with band retain. Steps ending over the cap number 717 / 5,338 and 738 / 3,795; peaks reach 10 / 16 and 10 / 17 layers.
- **Band retention trades bandwidth for misses.** At the worst cap it keeps 92–98% of joining blocks resident, and it cuts demand reads 13–52×. The churn moves to the band's outer edge instead of disappearing:
  - total reads are 0.93–1.55× immediate free's, in blocks and in bytes; on campaign's tour band retention reads less;
  - thrash reads rise 1.35–1.71×;
  - repacks rise 1.7–2.6× wherever immediate free repacks at all.

  The older cell-granular walks favoured immediate free over LRU on repacks. That result does not settle the brief's policy, which is band retention.
- The walks use a shelf allocator. Guillotine and merging allocators are unmeasured.

Set-independent figures:

| Figure | Hallway | Campaign | Source (`$SCRATCH/`) |
|---|---|---|---|
| Block overhead (block / chart texels) | 1.234× (p50 1.147×, p95 2.288×) | 1.381× (p50 1.163×, p95 4.118×) | `briefset-*.txt` |
| Largest block | 1144×1844 | 696×1044 | same |
| Sightline p50 / p95 / max | 71.6 / 127.1 / 237.5 m | 60.5 / 87.2 / 99.5 m | same |

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

Cell blocks fit Low at every measured L: the brief set peaks at 170.0 MiB worst at 32 m, against 256. No block exceeds 2048 in either dimension.

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
