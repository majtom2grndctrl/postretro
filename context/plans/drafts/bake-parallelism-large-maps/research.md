# bake-parallelism-large-maps — research

Derivation and numbers behind the brief. Every path below is relative to `crates/level-compiler/src/`. Source was read at 329fbe07b (`feat/lightmap-cell-blocks`). Line numbers were recorded at 4ae37c4af; spot-checks at 329fbe07b land within a few lines. Later additions cite symbols only.

## Measurement conditions

| Item | Value |
|---|---|
| Map | `content/dev/maps/stress-warren-hallway-inspection.map` |
| Machine | Intel i9-9980HK: 8 physical cores, 16 logical, 32 GiB RAM, macOS, APFS SSD |
| Cache mode | Warm (cache enabled), with every entry missing in the first build. Warm base SH always runs the approximate grouped path. Under the old 2 GiB `--cache-max-size` default the start-of-build prune evicts most of the hallway's live set; the brief raises the default. |
| Permits | Default `-j` from `default_jobs_for` (`cli.rs`): logical cores − 1 for 2 to 8 logical cores, logical cores − 2 above 8. This machine: 16 → 14. The global rayon pool is unconfigured, so it has one thread per logical core (16); the governor, not the pool, bounds concurrency. |
| Binary | A cargo `--release` build of `prl-build`, run warm. Cargo's release profile (`opt-level = 3`, thin LTO) is not `prl-build --release`, which is the cold, uncached ship bake. The numbers on this page predate that pin: the live rebake ran `target/debug/prl-build`, and the first build's profile was not recorded. `[profile.dev]` gives workspace crates `opt-level = 1` and dependencies, `bvh` included, `opt-level = 2`. Re-take the before numbers on the pinned binary. |
| Baseline | Main after `spatial-residency--lightmap-cell-blocks` lands. Byte-identity baselines and before numbers both come from there. |

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
| Ray kernel (all bakes) | — | `bvh.traverse_iterator` is unordered and tests the infinite ray. It only filters `max_distance` after the triangle test. The BVH has one leaf per (face, bucket). See §Ray traversal. | `sh_bake.rs:649`, `:732`; `lightmap_bake.rs:1169`; `billboard_direct_scatter_bake.rs:603`; `chunk_light_list_bake.rs:1027` |
| Delta SH / Direct SH Delta / Animated Direct | One (affinity cell, light) sub-block per item, about 13.8 KB | The permit covers only the bake; get and put run outside it on all 16 threads. The affinity decomposition is recomputed even though the plan already ran it. Validity masks are built serially. `world_aabb_for_directional` scans every vertex for every entry, including point lights, inside the permit. | `delta_sh_cache.rs:164-219`; `delta_sh_bake.rs:299`, `:370-390`, `:503-521`; `direct_sh_bake.rs:411`, `:539-576`; `pipeline.rs:211`, `:247`, `:301` |
| Lightmap Bake + ShadowmaskAtlas | One (light, chart) per item, one level deep; texels within a chart run serially | The outer `for layer { for light }` loop is serial, and each iteration ends in a `.collect()` barrier. After it come, serially: sort; `to_bytes`; `put` with fsync; `fold_partition`; `shadowmask.consume_partition`. Per layer: accumulator init, 2× `dilate`, and serial BC6H and direction encode. Up front: serial `layer_input_hash` over layers × lights, each rescanning all vertices, primitives and charts, even in `--release`. There is no per-light chart cull, so most items are empty walks that still take a governor lock. | `pipeline/lightmap_stage.rs:119-133`, `:192-231`, `:349-360`; `lightmap_layer.rs:432-449`, `:485-491`; `bc6h.rs:86-108`; `shadowmask_bake/fill.rs:146-160` |
| AnimWeightMaps | One animated chunk per item; texels × lights run serially inside | A chunk only splits above 4 lights, so chunks are uneven and have a long tail. `assert_no_overlapping_rects_per_layer` is O(n²) and runs in release. A single whole-stage cache entry. | `animated_light_weight_maps.rs:358-369`; `animated_light_weight_maps/static_atlas_frame.rs:184-233`; `pipeline/animated_atlas_stage.rs:51-103` |
| Packing | None | Serial per-cluster BC6H re-encode; serial section writes and readback | `bc6h.rs:86`; `pack_output.rs:140-166` |
| Shared hot spot | — | `probe_indices(light, full_samples)` is computed for every lit texel or hit. It is about 128 lattice evaluations and depends only on the light. | `lightmap_bake.rs:907-951`, `:1022` |

## Stage dependencies

- **No cross-stage concurrency exists.** There is no `thread::scope`, `rayon::join` or `rayon::scope` in the pipeline (`pipeline/stage_registry.rs:113-141`).
- **The five SH stages share one join point.** SH, Delta SH, Direct SH, Animated Direct and Direct SH Delta bake without reading each other's output.
  - After both bakes, Direct SH Delta's usability check (`pack::direct_sh_delta_is_usable_for_selection`) reads Direct SH's output section. An unusable result clears the entity-shadow selection (`pipeline.rs:1291-1321`). That is a join after both bakes, not a reason to serialize them.
  - The delta CSR plans and the entity-shadow selection come from `plan_delta_bakes`, before SH starts.
- **Atlas preparation stays after the SH family.** It reads no SH output; the SH stages borrow `geo_result` and atlas preparation takes it `&mut` (`pipeline.rs:1692-1710`). `build_pipeline.md` §Atlas preparation and SH ordering calls the order load-bearing: Direct SH Delta can clear entity-shadow selection wholesale, and deterministic channel assignment must finish before the fused atlas walk. The brief keeps that order; lever 3 overlaps only the SH family among itself and the animated stages with the fused walk.
- **AnimLightChunks and AnimWeightMaps** read atlas placement and the BVH, not lightmap output.
  - `bake_fused_prepared` takes `geometry: &mut` but never mutates it (`pipeline/lightmap_stage.rs:56`).
  - The only lightmap output read afterwards is `blocks.is_empty()`, in the layout step (`pipeline.rs:1937`).

## Lightmap loop details (lever 1)

- The `layer_input_hash` pre-pass in `bake_fused_prepared` has no `stage_cache` guard, so it also runs cold, where nothing reads the hashes. Each call recomputes `atlas_layout_fingerprint`, which depends on neither light nor layer, and `geometry_slice_hash`, which depends only on the light.
- `bake_light_layer_controlled` rebuilds `face_indices` by scanning every placement, once per (layer, light).
- When the lightmap section memo hits but the shadowmask memo misses, a second serial (layer, light) loop in `bake_fused_prepared` calls `load_or_bake_partition` again for the selected lights.
- `bake_shadowmask_atlas_with_window` and `SHADOWMASK_RESIDENT_LAYER_WINDOW` (4) survive in `shadowmask_bake`. Only tests reach them, through `bake_shadowmask_atlas`, `bake_shadowmask_atlas_cached` and the test-window wrappers. The fused path consumes partitions serially. They are the reuse candidate for lever 1's bounded window.
- `IncrementalLayerAccumulator::fold_partition` documents that callers fold partitions in global light order and that float addition is neither reordered nor reduced. That order is the determinism guarantee lever 1 keeps.
- On `feat/lightmap-cell-blocks` the fused walk still loops layer by layer, pushing each finished layer into `BlockSectionBuilder`. Lever 1's premise survives cell blocks.

## Ray traversal (lever 5)

bvh 0.11 (`crates/level-compiler/Cargo.toml`). Every site below builds a stock `Ray`, which has no length, and calls `Bvh::traverse_iterator`.

| Site | Query | Loop | Callers |
|---|---|---|---|
| `sh_bake::closest_hit` | Closest hit | Tests every leaf the iterator yields. A strict `dist < best` keeps the first-visited hit on a tie. `max_distance` is checked only after the triangle test, and the one caller passes `f32::INFINITY`. | `sh_bake::sample_radiance_rgb`: 256 rays per probe, base SH and the delta indirect path |
| `sh_bake::segment_clear` | Occlusion | Returns on the first hit with `0 < dist < max_distance` | Soft-visibility closures in `bake_probe_direct_rgb` and `sample_radiance_rgb` |
| `lightmap_bake::segment_clear` | Occlusion | Same | `lightmap_layer` texel bake, `animated_light_weight_maps`, `entity_shadow_select`, `lightmap_bake::reference` |
| `billboard_direct_scatter_bake::segment_clear` | Occlusion | Same | Billboard direct scatter |
| `chunk_light_list_bake::segment_clear` | Occlusion | Same. Stops `SAMPLE_END_TOLERANCE_METERS` short of the sample; a directional light's segment is 10,000 m long. | Chunk light list |

- **Closest hit has no early exit.** It tests every leaf the infinite ray crosses, however far beyond the nearest hit.
- **Occlusion already exits on its first blocker.** Its waste is range and order. A clear segment, the common case for a lit texel, still tests every leaf the ray crosses beyond the light. A blocked segment may test far leaves before the near blocker, because `traverse_iterator` walks depth-first, left child first.
- **Base SH ray mix.** 256 closest-hit rays per probe, then 4–32 soft-visibility shadow rays per hit and reaching light. Which kind dominates is unmeasured.
- **bvh 0.11 API.**
  - `nearest_traverse_iterator` exists (`Bvh`, `bvh/bvh_impl.rs`) and is unused. It pops nodes from a `BinaryHeap` by AABB entry distance and yields shapes only, not distances. A caller that stops at its best hit must recompute `Ray::intersection_slice_for_aabb` per leaf. The heap allocates per ray despite the type's "without memory allocations" doc, and its comparator `partial_cmp(..).unwrap()` panics on a NaN distance.
  - `nearest_child_traverse_iterator` is stack-based and allocation-free, but its order is best-effort.
  - `traverse_iterator` is generic over the public `IntersectsAabb` trait. A caller-defined query can wrap the ray and add a distance bound; interior mutability lets the bound shrink as hits land, without changing visit order.
  - `Ray::intersection_slice_for_aabb` treats an in-plane NaN slab as a miss, and the ray's own `intersects_aabb` path may not. A bounded query should reject a node only by the distance test, and keep it when the slab distance is undefined.
- **Byte identity.** Occlusion returns a boolean, so pruning nodes that lie wholly beyond the segment end cannot change it. For closest hit, pruning only nodes entered strictly beyond the best hit, with a small pad for slab-versus-Möller–Trumbore rounding, keeps depth-first visit order and so today's tie winner. Nearest-first ordering changes visit order: a tie at a shared edge could pick the other triangle and its normal. Matching today's bytes that way would need a tie key that reproduces depth-first order.

## Cache budget on large maps

- The cache directory reached **5.2 GB and about 142k entries** mid-build, against a 2 GiB budget.
- The prune runs once, at build start, right after `StageCache::new` in `main.rs`, LRU by mtime (`StageCache::prune_to_budget`). The next build therefore evicts most of this build's entries.
- An end-of-build warning already fires when this build's deduplicated live set exceeds the budget (`StageCache::warn_if_live_set_exceeds`). Its reported total is the sizing source for the new default.
- Orphan `<digest>.tmp` files are never swept: the prune skips them. The next put of the same key truncates and overwrites the file. Note only.
- Result: every hallway rebake re-bakes the SH family, yet still runs the approximate warm SH path. It pays cold SH cost and gets warm SH quality.
- The prune is LRU by mtime, so what survives depends on write order. In the second hallway build (feat/lightmap-cell-blocks, 2026-09-29, the same `target/debug` binary family), stages written last in the first build hit:
  - Lightmap Bake 388.7 s against 7,395.4 s, and AnimWeightMaps 1.0 s against 993.9 s.
  - SH Bake (11,033.4 s), Delta SH Bake (352.1 s) and Direct SH Delta Bake (1,687.5 s) missed.
  - Total 13,966 s (3 h 53 m), about 11.1 cores busy on average.

  The Lightmap Bake and AnimWeightMaps figures in the table above are all-miss numbers from the first build.
- `plans/done/lighting-scale--sparse-layer-cache-and-fused-walk` sized the 2 GiB budget against campaign-test layers (≤0.91 GB).

## Cache write path
- `StageCache::put` delegates to `put_streamed`, which stages `<digest>.tmp` through `write_streamed_entry` and then renames it into place.
- `write_streamed_entry` ends with `File::sync_all`, which is `F_FULLFSYNC` on macOS. The brief drops it. A killed or torn write still leaves only a `.tmp` or a failed blake3 check, so either way the next build misses.
- Every `get` hit calls `touch_for_lru`, a second open for write, to bump the mtime.

## Prior deferrals

- `plans/done/level-compiler-tui` made two choices here:
  - Non-goal: "Parallelizing the serial lightmap stage (it stays single-threaded)".
  - Default core count set below the logical core count.
- `plans/done/lightmap-bake-throughput` parallelized charts within one light. The outer light loop stayed serial.
- `plans/done/shadowmask-bake-scaling` shipped its first slice "serial over the light axis for now". Its Task 2 designed governed, bounded-window light-axis parallelism. The later fused walk folded shadowmask consumption into the lightmap's serial loop.
- `plans/done/perf-parallel-sh-group-bake`: "Follow-up (deferred, owner-accepted)". The per-entry `sync_all` cost about 45 s across about 900 puts on occlusion-test. It was expected to be minor because "the bake is ray-bound". On the hallway, Direct SH Delta is entirely I/O-bound.
- `plans/done/build-stage-cache` assumed a single builder and atomic rename, and did not budget fsync cost.

## Related drafts (not overlapping)

These drafts cut ray count or peak RAM. This brief cuts idle cores and per-ray traversal cost. The savings add up.
- `lighting-scale--cold-sh-bake-contribution-early-out`
- `lighting-scale--cold-bake-reaching-light-spike`
- `lighting-scale--sh-delta-cell-major-two-pass-bake`
- `bvh-leaf-clustering` (render-side leaf granularity; the bake shares the same BVH, so a leaf change shifts lever 5's per-leaf cost)
