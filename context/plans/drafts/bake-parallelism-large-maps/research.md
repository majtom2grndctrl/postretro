# bake-parallelism-large-maps — research

Derivation and numbers behind the brief. Every path below is relative to `crates/level-compiler/src/`, and every line number was read at 4ae37c4af.

## Measurement conditions

| Item | Value |
|---|---|
| Map | `content/dev/maps/stress-warren-hallway-inspection.map` |
| Machine | Intel i9-9980HK: 8 physical cores, 16 logical, 32 GiB RAM, macOS, APFS SSD |
| Cache mode | Warm (cache enabled), with every entry missing in the first build. The default 2 GiB `--cache-max-size` is smaller than the hallway's live set, so the start-of-build prune evicts it. Base SH therefore runs the approximate grouped path. |
| Permits | Default `-j`, which is `logical − 2` = 14 (`cli.rs:39-45`). Rayon uses its default global pool of 16 threads, and nothing configures it. |
| Binary | The live rebake ran `target/debug/prl-build`. `[profile.dev]` gives workspace crates `opt-level = 1` (`Cargo.toml`). The first build's profile was not recorded. Pin this before any before/after comparison. |

## Where the 5 h 54 m goes

**First build.** From the end-of-build summary:
- Wall time: 21,262 s. User plus system CPU time: 189,875 s. That is 8.9 cores busy on average.
- SH Bake 11,939 s
- Lightmap Bake 7,395 s
- AnimWeightMaps 994 s
- Delta SH Bake 354 s
- Direct SH Delta Bake 296 s
- ShadowmaskAtlas 174 s
- Direct SH Bake 26 s
- Packing 26 s
- Billboard Direct Scatter 16 s

**SH runs at the permit cap.** In the second build (the live rebake), the SH Bake ran from 5.2 s to 11,038.6 s, so 11,033 s. At 13,246 s elapsed, the process had used 154,202 CPU-s.
- Everything after SH is small next to that. The later stages together are bounded by 14 cores × 589 s for the delta, direct and animated stages, plus about 230 CPU-s for Direct SH Delta.
- So SH used 145–154k CPU-s. That is **13.2–14.0 cores busy, which is the 14-permit cap**.
- The per-thread CPU time agrees. All 16 rayon workers had used about 150–165 CPU-min each.
- Base SH parallelism is saturated. On 8 physical cores, those 14 logical threads are already hyperthreaded.

**Everything else ran at about 3 cores.** In the first build, assume SH ran at the same 13.2–14.0 cores over 11,939 s. That accounts for 158–167k CPU-s.
- About 23–32k CPU-s remain for the other stages, which took 9,323 s of wall time. That is 2.4–3.5 cores on average.
- Even if the Lightmap Bake got every one of those CPU-s, it would average **no more than 4.4 of 14 permits** over its 7,395 s.
- This is the parallelism headline: the lightmap bake uses at most about a third of the cores it is allowed.

**Live sampler.** It took a sample every 10 s: `ps` %CPU (100 = one core), RSS and thread count. Samples, covering 12,798 s to the build's exit at 13,967 s, are in `evidence/cpu-samples.tsv`.

| Stage | Samples | Mean %CPU | p10 | p50 | p90 | Threads |
|---|---|---|---|---|---|---|
| Direct SH Delta Bake (last ~580 s of 1,717 s) | 58 | 20 | 12 | 14 | 18 | 17–19 |
| Billboard direct scatter bake | 1 | 1402 | — | — | — | 18 |
| Chunk light list bake | 1 | 108 | — | — | — | 18 |
| Lightmap Bake + ShadowmaskAtlas, warm hit, whole 557 s window (logged as "Shadowmask atlas bake" / "ShadowmaskAtlas"; both start at 13,371 s) | 56 | 100 | 99 | 100 | 100 | 19 |
| Packing and writing | 4 | 84 | 43 | 98 | 100 | 18 |

- The 1402% Billboard sample is 14 cores, so that stage saturates the cap.
- For its first ~90 s the lightmap bake runs on exactly one core. A 2 s stack sample shows the main thread in `lightmap_layer::cache_keys::layer_input_hash` → `atlas_layout_fingerprint`: blake3 over every chart and placement, fed 4–8 bytes per `update` call. That hash does not depend on the light or layer, but the serial pre-pass runs it for every (layer, light) pair (`pipeline/lightmap_stage.rs:119-133`). Meanwhile all rayon workers wait on a condvar. The raw stack sample was not retained; the frames above are its content.
- **Even a warm lightmap hit is single-threaded.** In the second build the lightmap layers hit the cache. Lightmap Bake (388.7 s) and ShadowmaskAtlas (168.2 s) together sat at exactly one core for the whole window. That time is the serial pre-pass hash plus the per-partition work: `get` with its blake3 verify, fold, shadowmask fill, dilate, and BC6H. So about 9 minutes of every warm hallway rebake uses 1 of 14 permits. Only lever 1's serial-tail and hash-hoisting parts would change that; its light-axis ray parallelism would not.
- In this rebake, Direct SH Delta took 1,717 s. In the first build it took 296 s. The compute is the same, so the difference is cache I/O. The cache directory had grown to 5.2 GB across about 142k flat entries.
- Missed: every stage before Direct SH Delta, including all of SH Bake. The sampler started at 12,798 s elapsed.
- Also missed: Atlas Preparation, Cell Residency Set, AnimLightChunks and AnimWeightMaps. Each ran for under 10 s, which is less than one sample interval. The sampler ran until the build exited at 13,967 s. Run `evidence/stats.sh` over `evidence/cpu-samples.tsv` for per-stage averages and percentiles.

**Stack sample of Direct SH Delta.** A 3 s `sample` run shows all 16 workers inside cache syscalls, under `delta_sh_cache::bake_or_load_delta_subblocks` → `StageCache::put_streamed` / `get`:

| Syscall | Share of worker samples | Source |
|---|---|---|
| `open` | 36% | Includes the second open that `touch_for_lru` makes on every hit |
| `fcntl` | 25% | `F_FULLFSYNC`, from `File::sync_all` |
| `rename` | 25% | |
| `write` | 15% | Unbuffered |

No sample landed in ray code. The raw stack sample was not retained; the breakdown above is its content.

## Code survey (stages over ~10 s)

| Stage | Parallel unit | Serial sections / barriers | Source |
|---|---|---|---|
| SH Bake, warm | One 4³ group per item; its 64 probes run serially. 256 rays per probe; each hit scans the lights; soft visibility uses 4–32 shadow rays. | Placement and compaction. The permit is held across the cache get/put, so fsync time consumes permits. Every group writes an entry, even an all-invalid one. `probe_grid_layout` BSP queries run serially over the full grid. | `sh_group.rs:709-728`, `:373-394`, `:514`, `:535`; `sh_bake.rs:168-181`, `:1158`, `:1293` |
| SH Bake, cold | One probe per item | Probe-grid layout; atlas pack | `sh_bake.rs:258-279` |
| Ray kernel (all bakes) | — | `bvh.traverse_iterator` is unordered and tests the infinite ray. It only filters `max_distance` after the triangle test. The BVH has one leaf per (face, bucket). `nearest_traverse_iterator` exists and is unused. | `sh_bake.rs:649`, `:732`; `lightmap_bake.rs:1169`; `billboard_direct_scatter_bake.rs:603`; `chunk_light_list_bake.rs:1027` |
| Delta SH / Direct SH Delta / Animated Direct | One (affinity cell, light) sub-block per item, about 13.8 KB | The permit covers only the bake; get and put run outside it on all 16 threads. The affinity decomposition is recomputed even though the plan already ran it. Validity masks are built serially. `world_aabb_for_directional` scans every vertex for every entry, including point lights, inside the permit. | `delta_sh_cache.rs:164-219`; `delta_sh_bake.rs:299`, `:370-390`, `:503-521`; `direct_sh_bake.rs:411`, `:539-576`; `pipeline.rs:211`, `:247`, `:301` |
| Lightmap Bake + ShadowmaskAtlas | One (light, chart) per item, one level deep; texels within a chart run serially | The outer `for layer { for light }` loop is serial, and each iteration ends in a `.collect()` barrier. After it come, serially: sort; `to_bytes`; `put` with fsync; `fold_partition`; `shadowmask.consume_partition`. Per layer: accumulator init, 2× `dilate`, and serial BC6H and direction encode. Up front: serial `layer_input_hash` over layers × lights, each rescanning all vertices, primitives and charts, even in `--release`. There is no per-light chart cull, so most items are empty walks that still take a governor lock. | `pipeline/lightmap_stage.rs:119-133`, `:192-231`, `:349-360`; `lightmap_layer.rs:432-449`, `:485-491`; `bc6h.rs:86-108`; `shadowmask_bake/fill.rs:146-160` |
| AnimWeightMaps | One animated chunk per item; texels × lights run serially inside | A chunk only splits above 4 lights, so chunks are uneven and have a long tail. `assert_no_overlapping_rects_per_layer` is O(n²) and runs in release. A single whole-stage cache entry. | `animated_light_weight_maps.rs:358-369`; `animated_light_weight_maps/static_atlas_frame.rs:184-233`; `pipeline/animated_atlas_stage.rs:51-103` |
| Packing | None | Serial per-cluster BC6H re-encode; serial section writes and readback | `bc6h.rs:86`; `pack_output.rs:140-166` |
| Shared hot spot | — | `probe_indices(light, full_samples)` is computed for every lit texel or hit. It is about 128 lattice evaluations and depends only on the light. | `lightmap_bake.rs:907-951`, `:1022` |

## Stage dependencies

- **No cross-stage concurrency exists.** There is no `thread::scope`, `rayon::join` or `rayon::scope` in the pipeline (`pipeline/stage_registry.rs:113-141`).
- **The five SH stages are independent.** SH, Delta SH, Direct SH, Animated Direct and Direct SH Delta read none of each other's outputs. After the bakes, a Direct SH Delta result that cannot be used clears the entity-shadow selection (`pipeline.rs:1291-1321`).
- **Atlas preparation** needs no SH output. It follows SH only because the SH stages borrow `geo_result` and atlas preparation takes it `&mut` (`pipeline.rs:1692-1710`).
  - `build_pipeline.md` §Atlas preparation and SH ordering calls the pre-atlas ordering load-bearing. That holds for the fused lightmap/shadowmask walk, which needs the final selection. It does not hold for atlas prep, billboard or ChunkLightList, which are only serialized.
- **AnimLightChunks and AnimWeightMaps** read atlas placement and the BVH, not lightmap output.
  - `bake_fused_prepared` takes `geometry: &mut` but never mutates it (`pipeline/lightmap_stage.rs:56`).
  - The only lightmap output read afterwards is `blocks.is_empty()`, in the layout step (`pipeline.rs:1937`).

## Cache budget on large maps

- The cache directory reached **5.2 GB and about 142k entries** mid-build, against a 2 GiB budget.
- The prune runs only at build start, so the next build evicts most of this build's entries (`cache.rs` `prune_to_budget`).
- Result: every hallway rebake re-bakes the SH family, yet still runs the approximate warm SH path. It pays cold SH cost and gets warm SH quality.
- The prune is LRU by mtime, so what survives depends on write order. In the second hallway build (feat/lightmap-cell-blocks, 2026-09-29, the same `target/debug` binary family), stages written last in the first build hit:
  - Lightmap Bake 388.7 s against 7,395.4 s, and AnimWeightMaps 1.0 s against 993.9 s.
  - SH Bake (11,033.4 s), Delta SH Bake (352.1 s) and Direct SH Delta Bake (1,687.5 s) missed.
  - Total 13,966 s (3 h 53 m), about 11.1 cores busy on average.

  The Lightmap Bake and AnimWeightMaps figures in the table above are all-miss numbers from the first build.
- `plans/done/lighting-scale--sparse-layer-cache-and-fused-walk` sized the 2 GiB budget against campaign-test layers (≤0.91 GB).

## Prior deferrals

- `plans/done/level-compiler-tui` made two choices here:
  - Non-goal: "Parallelizing the serial lightmap stage (it stays single-threaded)".
  - Default core count set below the logical core count.
- `plans/done/lightmap-bake-throughput` parallelized charts within one light. The outer light loop stayed serial.
- `plans/done/shadowmask-bake-scaling` shipped its first slice "serial over the light axis for now". Its Task 2 designed governed, bounded-window light-axis parallelism. The later fused walk folded shadowmask consumption into the lightmap's serial loop.
- `plans/done/perf-parallel-sh-group-bake`: "Follow-up (deferred, owner-accepted)". The per-entry `sync_all` cost about 45 s across about 900 puts on occlusion-test. It was expected to be minor because "the bake is ray-bound". On the hallway, Direct SH Delta is entirely I/O-bound.
- `plans/done/build-stage-cache` assumed a single builder and atomic rename, and did not budget fsync cost.

## Related drafts (not overlapping)

These drafts cut ray count or peak RAM. This brief cuts idle cores. Both kinds of saving add up.
- `lighting-scale--cold-sh-bake-contribution-early-out`
- `lighting-scale--cold-bake-reaching-light-spike`
- `lighting-scale--sh-delta-cell-major-two-pass-bake`
- `bvh-leaf-clustering` (render-side leaf granularity; the bake shares the same BVH)
