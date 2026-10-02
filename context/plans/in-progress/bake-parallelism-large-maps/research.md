# bake-parallelism-large-maps — research

Derivation and numbers behind the brief. Every path below is relative to `crates/level-compiler/src/`. Source was re-read by symbol at 4db4473f7 (`main`). The pipeline, atlas preparation, lightmap stage and BVH build were re-checked at b8b338400 (`main`, after the oversize merges), and their citations are symbols. Remaining line numbers were recorded at 4ae37c4af and may have drifted. Later additions cite symbols only.

## Measurement conditions

| Item | Value |
|---|---|
| Map | `content/dev/maps/stress-warren-hallway-inspection.map` |
| Machine | The owner's Windows PC: 6 cores. It bakes the hallway in about 9 h. Each run records its specs in `machine.txt`: CPU, physical cores, logical processors, RAM, OS, volume, git HEAD and working-tree status. |
| Other host | The owner's Mac: Intel i9-9980HK, 8 physical cores, 16 logical, 32 GiB RAM, macOS, APFS SSD; 14 permits. It bakes the hallway in about 6 h. It is not the yardstick. Every number on this page comes from it: context, not baselines. |
| Procedure | `evidence/measure-hallway.ps1` (PowerShell 5.1 or 7) is the canonical run. It builds the binary below, bakes once under these conditions, and samples the process every 10 s into `cpu-samples.tsv`, in the column layout `evidence/stats.sh` reads. `summary.txt` records exit code, wall time, CPU time, peak working set and the Build Summary. Not yet run. |
| Cache mode | Warm (cache enabled). The run starts on an empty cache directory, a fresh path passed to `--cache-dir`, so every entry misses in the first build. The script creates it inside its run folder and refuses a non-empty one. Warm base SH always runs the approximate grouped path. Under the 2 GiB `--cache-max-size` default, today's start-of-build prune evicts most of the hallway's live set; the brief's prune rule spares each map's record. |
| Peak memory | The process's peak working set over the whole build, the counter `Process.PeakWorkingSet64` reads. The script reads it after exit and records it in `summary.txt`. See §Peak memory. |
| Permits | Default `-j` from `default_jobs_for` (`cli.rs`): logical cores − 1 for 2 to 8 logical cores, logical cores − 2 above 8. The run records the value in `machine.txt`, from the binary's `--help` and checked against the rule. The global rayon pool is unconfigured, so it has one thread per logical core; the governor, not the pool, bounds concurrency. |
| Binary | `prl-build` built in cargo's release profile (`cargo build --release -p postretro-level-compiler --bin prl-build`: `opt-level = 3`, thin LTO), then run without its own `--release` flag, with plain progress (`--no-tui`). The script builds and runs it. `prl-build --release` is the cold ship bake, and it implies `--no-cache`. The numbers on this page predate that pin: the live rebake ran `target/debug/prl-build`, and the first build's profile was not recorded. `[profile.dev]` gives workspace crates `opt-level = 1` and dependencies, `bvh` included, `opt-level = 2`. Re-take the before numbers on the pinned machine and binary. |
| Baseline | Main after `lightmap-oversize-cells-and-faces` (#544). Byte-identity baselines and before numbers both come from there. That plan recorded no hallway RSS or stage timings; its recorded RSS runs are cold small-map bakes, unusable as before numbers. |

## Baseline record

What to do with a finished `measure-hallway.ps1` run. The before run happens once, on main at or after #544, before any lever lands. Each lever's after run repeats it on the lever's branch.

**Check before trusting the run.** All must hold, or the run is not a baseline.
- `summary.txt`: `exit_code: 0`.
- `machine.txt`: `git_status_porcelain: clean`, and `git_head` is at or after #544 with no compiler change since.
- `machine.txt`: the two default-`-j` values agree.
- `cpu-samples.tsv`: the stage column fills in after the first stage begins. A blank column means the stderr log was unreadable mid-run; stage wall times still hold, per-stage busy cores do not.
- `summary.txt`: the cache directory is non-empty at exit. The script refuses a non-empty cache at start, so every stage missed.

**Commit.** Copy `machine.txt`, `summary.txt`, `cpu-samples.tsv` and `prl-build.stdout.log` into `evidence/windows-before/` (an after run: `evidence/windows-after-<lever>/`). Leave out the `.prl`, the cache directory and the stderr log. `evidence/cpu-samples.tsv` is the Mac's and stays.

**Derive.** Run `evidence/stats.sh` on the committed TSV from Git Bash or WSL. It prints per-stage sample count, mean and percentile busy-core %, and peak RSS. Busy-core % divides by 100 for cores.

**Fill this table** in place, one column per run.

| Number | Source | Acceptance row | Before | After |
|---|---|---|---|---|
| Total wall time | `summary.txt` | Total wall time | 19,387.7 s (5 h 23 m) | |
| SH Bake wall time | Build Summary | SH Bake, traversal change alone | 11,535.7 s | |
| Wall time: Lightmap Bake, AnimWeightMaps, ShadowmaskAtlas, Delta SH, Direct SH Delta, Animated Direct | Build Summary | Per-stage wall time and busy cores | 6,310.9 / 664.4 / 210.2 / 150.7 / 321.8 / 24.9 s | |
| Mean busy cores per stage above | `stats.sh` | Per-stage wall time and busy cores | Lightmap Bake + ShadowmaskAtlas 2.88 (one sampler label, see note) / AnimWeightMaps 1.00 / Delta SH 4.35 / Direct SH Delta 0.67 / Animated Direct 1.34 (3 samples) | |
| Mean busy cores, Direct SH Delta Bake | `stats.sh` | Direct SH Delta busy cores | 0.67 (p50 0.95, 33 samples) | |
| Peak working set | `summary.txt` | Peak RSS | 3.62 GiB (3,799,832 KB) | |
| Mean busy cores, whole build | `summary.txt` | Context | 3.95 of 5 permits | |
| Cache size at exit | `summary.txt` | Context for the prune rule | 233,110 files, 12.76 GiB | |

**Before run (R0).** `evidence/windows-before/`, the owner's run of 2026-10-01 at 83489c52b. Between it and the pre-lever compiler (1527f5b26) only `level-loader` test code and a doc comment changed, so it is the pre-lever bake. SH Bake ran at 4.86 of 5 permits (sampler label "SH volume bake"). The fused walk's 6,521 s carry one sampler label, "Shadowmask atlas bake", because both stages begin together and the sampler keeps the latest label; its 2.88 cores cover Lightmap Bake and ShadowmaskAtlas together.

**Not covered by this run.**
- The cold Lightmap Bake row: a separate run with `prl-build --release` on the same binary, before lever 1 lands.
- The lever 3 gate: measured after levers 1 and 2 land (§Lever 3 gate).
- The second-build budget row: an after-only check under the new prune rule.

## Where the 5 h 54 m goes

All numbers in this section come from the Mac (§Measurement conditions, Other host).

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
- Lever 5 is the main lever on base SH's 11,939 s. Lever 2 also trims it: each SH group holds its permit across a cache put that ends in `sync_all`. Lever 4's `probe_grid_layout` candidate runs serially at its start. The other levers target the roughly 9,300 s around it.

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

- Direct SH Delta on this debug-binary rebake: median 0.14 busy cores, mean 0.20. Every other busy-cores figure here is a mean, so the brief's row compares means, re-taken under the pinned conditions.
- The 1402% Billboard sample is 14 cores, so that stage saturates the cap.
- For its first ~90 s the lightmap bake runs on exactly one core. A 2 s stack sample shows the main thread in `lightmap_layer::cache_keys::layer_input_hash` → `atlas_layout_fingerprint`: blake3 over every chart and placement, fed 4–8 bytes per `update` call. That hash does not depend on the light or layer, but the serial pre-pass in `lightmap_stage::bake_fused_prepared` runs it for every (layer, light) pair. Meanwhile all rayon workers wait on a condvar. The raw stack sample was not retained; the frames above are its content.
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
| Ray kernel (all bakes) | — | `bvh.traverse_iterator` is unordered and tests the infinite ray. It only filters `max_distance` after the triangle test. The BVH has one primitive, so one leaf, per face (`bvh_build::collect_primitives`). See §Ray traversal. | `sh_bake.rs:649`, `:732`; `lightmap_bake.rs:1169`; `billboard_direct_scatter_bake.rs:603`; `chunk_light_list_bake.rs:1027` |
| Delta SH / Direct SH Delta / Animated Direct | One (affinity cell, light) sub-block per item, about 13.8 KB | The permit covers only the bake; get and put run outside it on all 16 threads. The affinity decomposition is recomputed even though the plan already ran it. Validity masks are built serially. `world_aabb_for_directional` scans every vertex for every entry, including point lights, inside the permit. | `delta_sh_cache.rs:164-219`; `delta_sh_bake.rs:299`, `:370-390`, `:503-521`; `direct_sh_bake.rs:411`, `:539-576`; the three affinity decompositions in `pipeline::plan_delta_bakes` |
| Lightmap Bake + ShadowmaskAtlas | One (light, chart) per item, one level deep; texels within a chart run serially | The outer `for layer { for light }` loop is serial, and each iteration ends in a `.collect()` barrier. After it come, serially: sort; `to_bytes`; `put` with fsync; `fold_partition`; `shadowmask.consume_partition`. Per layer: accumulator init, 2× `dilate`, and serial BC6H and direction encode. Up front: serial `layer_input_hash` over layers × lights, each rescanning all vertices, primitives and charts, even in `--release`. There is no per-light chart cull, so most items are empty walks that still take a governor lock. | `lightmap_stage::bake_fused_prepared` (pre-pass, layer loop, second loop), `lightmap_stage::load_or_bake_partition`; `lightmap_layer.rs:432-449`, `:485-491`; `bc6h.rs:86-108`; `shadowmask_bake/fill.rs:146-160` |
| AnimWeightMaps | One animated chunk per item; texels × lights run serially inside | A chunk only splits above 4 lights, so chunks are uneven and have a long tail. `assert_no_overlapping_rects_per_layer` is O(n²) and runs in release. A single whole-stage cache entry. | `animated_light_weight_maps.rs:358-369`; `animated_light_weight_maps/static_atlas_frame.rs:184-233`; `animated_atlas_stage::bake_or_load_weight_maps` |
| Packing | None | Serial per-cluster BC6H re-encode; serial section writes and readback | `bc6h.rs:86`; `pack_output.rs:140-166` |
| Shared hot spot | — | `probe_indices(light, full_samples)` is computed for every lit texel or hit. It is about 128 lattice evaluations and depends only on the light. | `lightmap_bake.rs:907-951`, `:1022` |

## Stage dependencies

- **No cross-stage concurrency exists.** There is no `thread::scope`, `rayon::join` or `rayon::scope` in the pipeline; `pipeline::run_after_parsing` calls the stages one after another.
- **The five SH stages share one join point.** SH, Delta SH, Direct SH, Animated Direct and Direct SH Delta bake without reading each other's output.
  - After both bakes, Direct SH Delta's usability check (`pack::direct_sh_delta_is_usable_for_selection`, called in `run_after_parsing`) reads Direct SH's output section. An unusable result clears the entity-shadow selection. That is a join after both bakes, not a reason to serialize them.
  - The delta CSR plans and the entity-shadow selection come from `plan_delta_bakes`, before SH starts.
- **Atlas preparation stays after the SH family.** It reads no SH output; the SH stages borrow `geo_result` and `atlas_stage::prepare_atlas_stage` takes it `&mut`. That function plans charts, cuts oversize faces, rebuilds the face-identity set (leaf face ranges, BVH, CellDrawIndex) over the cut geometry, resolves the cell partition, packs blocks, and checks the animated block bound. The SH family, entity-shadow selection, Billboard direct scatter and ChunkLightList run before it, on uncut geometry and the pre-cut BVH. `build_pipeline.md` §Atlas preparation and SH ordering calls the order load-bearing: Direct SH Delta can clear entity-shadow selection wholesale, and deterministic channel assignment must finish before the fused atlas walk. The brief keeps that order; lever 3, if built, overlaps only the SH family among itself and the animated stages with the fused walk.
- **AnimLightChunks and AnimWeightMaps** read atlas placement and the BVH, not lightmap output. On a cut map that BVH is the post-cut tree, the same one the fused walk reads.
  - `bake_fused_prepared` takes `geometry: &mut` but never mutates it.
  - The only lightmap output read afterwards is `lightmap_section.blocks.is_empty()`, passed to `animated_atlas_stage::layout_animated_atlas`.

## Lever 3 gate

- **Absolute bar.** The hallway bakes in about 9 h on the pinned machine and about 6 h on the Mac. 15 minutes per build repays the added complexity; a percentage would move with the host.
- **The yardstick is the less favorable host.** With fewer permits, its stages saturate sooner and leave fewer idle. A skip decided there is conservative: the Mac, with 14 permits, may clear the bar where the pinned machine does not.
- **Overlap recovers only idle permits.** A stage beside the saturated base SH bake competes for the same global governor and gains nothing. While base SH runs, all 16 rayon workers are either permitted or parked in the governor.
- **Earlier levers shrink its target.** Lever 2 removes the I/O-bound delta idle time overlap mostly targets. Lever 1 shrinks the AnimWeightMaps gain.
- **Most invasive lever.** `pipeline/stage_registry.rs` has no cross-stage concurrency today (§Stage dependencies). `development_guide.md` §1.4: no measured bottleneck, no optimization.
- **Precedent.** The fused Lightmap + ShadowmaskAtlas bake already runs one foreground and one background stage under one governor.

## Lightmap loop details (lever 1)

- The `layer_input_hash` pre-pass in `bake_fused_prepared` has no `stage_cache` guard, so it also runs cold, where nothing reads the hashes. Each call recomputes `atlas_layout_fingerprint`, which depends on neither light nor layer, and `geometry_slice_hash`, which depends only on the light.
- `bake_light_layer_controlled` rebuilds `face_indices` by scanning every placement, once per (layer, light).
- When the lightmap section memo hits but the shadowmask memo misses, a second serial (layer, light) loop in `bake_fused_prepared` calls `load_or_bake_partition` again for the selected lights.
- `bake_shadowmask_atlas_with_window` and `SHADOWMASK_RESIDENT_LAYER_WINDOW` (4) survive in `shadowmask_bake`. Only tests reach them, through `bake_shadowmask_atlas`, `bake_shadowmask_atlas_cached` and the test-window wrappers. The fused path consumes partitions serially. They are the reuse candidate for lever 1's bounded window.
- Partition cache get and put run outside the governor permit, as `delta_sh_cache::bake_or_load_delta_subblocks` does ("Permit guards ray work, not cache I/O"; a hit checkpoints without a permit). Base SH's group bake is the other precedent: it holds its permit across get and put, so fsync time consumes permits.
- `IncrementalLayerAccumulator::fold_partition` documents that callers fold partitions in global light order and that float addition is neither reordered nor reduced. That order is the determinism guarantee lever 1 keeps.
- On main after the oversize merges the fused walk still loops layer by layer, pushing each finished layer into `BlockSectionBuilder`. Lever 1's premise survives multi-block cells and face cuts.

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
- **Base SH ray mix.** 256 closest-hit rays per probe, then 4–32 soft-visibility shadow rays per hit and reaching light. Shadow rays likely outnumber closest-hit rays, but each closest-hit ray is infinite and never exits early. Which kind dominates cost is unmeasured.
- **Left-first order blunts a best-hit bound.** `traverse_iterator` visits the left child first, whatever the ray direction. A closest-hit ray may find far hits before near ones, so its bound shrinks late and prunes little. Occlusion segments do not depend on this: their bound is the segment end, fixed from the start.
- **Rival shape: near-child-first walk with a tie key.** A stack-based walk that descends the nearer child first finds the nearest hit early, so a best-hit bound prunes most of the tree. It changes visit order, so it must pick the winner by key, not by first visit: (distance, leaf rank in today's depth-first left-first order, triangle offset within the leaf). Today's strict `dist < best` keeps the first-visited hit on a tie, and that key names the same one. A node may be pruned only when it is entered strictly beyond the best distance plus the rounding pad, so a tied hit with a lower rank is never skipped. The leaf rank can come from one depth-first pass at BVH build.
- **bvh 0.11 API.**
  - `nearest_traverse_iterator` exists (`Bvh`, `bvh/bvh_impl.rs`) and is unused. It pops nodes from a `BinaryHeap` by AABB entry distance and yields shapes only, not distances. A caller that stops at its best hit must recompute `Ray::intersection_slice_for_aabb` per leaf. The heap allocates per ray despite the type's "without memory allocations" doc, and its comparator `partial_cmp(..).unwrap()` panics on a NaN distance.
  - `nearest_child_traverse_iterator` is stack-based and allocation-free, but its order is best-effort.
  - `traverse_iterator` is generic over the public `IntersectsAabb` trait. A caller-defined query can wrap the ray and add a distance bound; interior mutability lets the bound shrink as hits land, without changing visit order.
  - `Ray::intersection_slice_for_aabb` treats an in-plane NaN slab as a miss, and the ray's own `intersects_aabb` path may not. A bounded query should reject a node only by the distance test, and keep it when the slab distance is undefined.
- **Byte identity.** Occlusion returns a boolean, so pruning nodes that lie wholly beyond the segment end cannot change it. For closest hit, pruning only nodes entered strictly beyond the best hit, with a small pad for slab-versus-Möller–Trumbore rounding, keeps depth-first visit order and so today's tie winner. Nearest-first ordering changes visit order: a tie at a shared edge could pick the other triangle and its normal, unless the tie key above picks the winner. Profile both shapes on one hallway SH group before choosing.
- **A cut map builds the BVH twice.** The pre-atlas build and atlas preparation's rebuild run in sequence, both through `collect_primitives`, so both keep one leaf per face.
- **Baseline leaf set.** Lever 5 is measured on today's per-face leaves. `bvh-leaf-clustering` keeps bakes on that set; see §Related drafts.

## Cache budget on large maps

- The cache directory reached **5.2 GB and about 142k entries** mid-build, against a 2 GiB budget.
- The prune runs once, at build start, right after `StageCache::new` in `main.rs`, LRU by mtime (`StageCache::prune_to_budget`). The next build therefore evicts most of this build's entries.
- An end-of-build warning already fires when this build's deduplicated live set exceeds the budget (`StageCache::warn_if_live_set_exceeds`). The live set it sums is tracked in memory for the build (`StageCache::live_set`) and not persisted.
- Orphan `<digest>.tmp` files are never swept: the prune skips them. The next put of the same key truncates and overwrites the file. Note only.
- Result: every hallway rebake re-bakes the SH family, yet still runs the approximate warm SH path. It pays cold SH cost and gets warm SH quality.
- The prune is LRU by mtime, so what survives depends on write order. In the second hallway build (feat/lightmap-cell-blocks, 2026-09-29, the same `target/debug` binary family), stages written last in the first build hit:
  - Lightmap Bake 388.7 s against 7,395.4 s, and AnimWeightMaps 1.0 s against 993.9 s.
  - SH Bake (11,033.4 s), Delta SH Bake (352.1 s) and Direct SH Delta Bake (1,687.5 s) missed.
  - Total 13,966 s (3 h 53 m), about 11.1 cores busy on average.

  The Lightmap Bake and AnimWeightMaps figures in the table above are all-miss numbers from the first build.
- `plans/done/lighting-scale--sparse-layer-cache-and-fused-walk` sized the 2 GiB budget against campaign-test layers (≤0.91 GB).
- **Bound under the brief's rule.** After the prune the cache is at most the larger of the budget and the total spared set; the build then adds its new generation, as today.
- **Why per map, and why the last success.** The cache directory is shared by every map under the workspace root (`build_pipeline.md` §Build Cache, Location). A campaign-test build between two hallway builds would otherwise expose the hallway's set. In plain mode the cache is constructed and pruned before parsing, so a parse error or a `--sh-delta-working-set-max-size` refusal still runs a prune while touching nothing. Stopping a 6 to 9 h bake is the most common hallway event. Sparing only the previous build would cost an SH re-bake (about 11,000 s) after any of these.
- **Why memo hits count.** A `lightmap_section` hit reads no per-light partition, and a `shadowmask_atlas` hit requests none. `build_pipeline.md` §Build Cache calls those partitions the recompose fallback when a light changes. Unmarked, a no-edit rebuild leaves them untouched, and the first light edit after it re-bakes every light.
- **Why not raise the default.** It would have to track the largest map anyone builds, and still evicts by write order once that map outgrows it.
- **Mechanism, the executor's call.** Nothing persists which entries a build touched: `live_entries` (`StageCache::live_set`) is in memory only, and prune sorts by mtime alone. Options: a per-map key list appended as the build touches entries and promoted to the map's record on success, or a per-map start marker read against mtime. Lever 2 removes or changes the second open in `touch_for_lru`, so an mtime scheme needs another way to record a hit. A record written only at end of build loses a stopped build's touches: the only end-of-build hook runs on success or a returned error, never on a kill. A build reads its map's record before recording its own start (pin P6).

## Cache write path
- `StageCache::put` delegates to `put_streamed`, which stages `<digest>.tmp` through `write_streamed_entry` and then renames it into place.
- `write_streamed_entry` ends with `File::sync_all`, which is `F_FULLFSYNC` on macOS and `FlushFileBuffers` on Windows. The brief drops it. A killed or torn write still leaves only a `.tmp` or a failed blake3 check, so either way the next build misses.
- Every `get` hit calls `touch_for_lru`, a second open for write, to bump the mtime.

## Peak memory

- **What raises co-residency.** Lever 1 holds several lights' baked partitions for a layer at once, plus any awaiting cache write, fold, or shadowmask consume. Lever 3, if built, runs a second stage's working set beside the first. `development_guide.md` §1.4 counts every representation that coexists across production, buffering, serialization, cache writes, return, and cleanup; the lever 1 window row counts partitions at each of those points, not only in the ray kernel.
- **Pre-cut geometry.** On a cut map that needs the SDF atlas, `AtlasStageOutput::pre_cut_geometry` holds a whole geometry copy from atlas preparation until the SDF stage drops it. It already spans the fused walk and the animated stages. If lever 3 overlaps stages in that span, count it as a co-resident term. The hallway is uncut, so its peak carries no copy.
- **The delta working-set gate assumes serial stages.** `plans/done/lighting-scale--compile-peak-ram` sets the `--sh-delta-working-set-max-size` gate at 3× the cumulative dense delta bytes (4× under coarsened `--sh-analyze`). Its accounting follows today's run order: the three delta bakes in sequence, the exact-zero drop rebuild freed before compaction allocates, and one share reserved for the base id34/id35 copies held between the delta bakes. Overlapping base SH or the delta bakes with each other puts in-flight bake state beside those buffers, which the factor does not count.
- **Hallway hosts.** `drafts/compiler-implausible-allocation-guard` records a `--release` hallway compile at lightmap density 0.04 dying on a 16 GiB machine. The request there was an implausible 42.9 TB, not a working set that outgrew the host, so it shows the hallway is compiled on 16 GiB hosts, not how close its peak is to 16 GiB.
- **Evidence so far.** The Mac sampler's RSS column (`evidence/cpu-samples.tsv`) peaks at about 3.3 GiB, in Direct SH Delta, on the debug-binary warm rebake. It covers only the last 1,169 s: no SH Bake and no all-miss lightmap bake. The before number must be re-taken on the pinned machine and binary.

## Ordering pins

Orderings an Acceptance row must exercise. Each row that rests on a pin cites it by id.

| Pin | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Lightmap window of two or more lights on one atlas layer | Light k+1's partition (baked or loaded) is ready before light k's | Fold and shadowmask consume run in ascending global light order. Bytes equal the window-1 bake. |
| P2 | Bounded traversal query (lever 5) | A node's slab distance is undefined (the ray lies in a bounding-box face plane). Or the first triangle reached is the one the ray starts on (hit at or below `RAY_EPSILON`). | The undefined-slab node is kept. A rejected sub-epsilon hit never tightens the bound. The answer equals today's scan. |
| P3 | Lightmap window partly admitted | Permits drop to 1, or pause is set, after some of the window's chart items are admitted and before the rest | Admitted items finish. Later items admit one at a time, and none while paused. No permitted item waits on another. The bake completes with unthrottled bytes. |
| P4 | Lever 3 built, with AnimWeightMaps overlapping the fused walk | The lightmap-emptiness input to `layout_animated_atlas` is taken before the walk finishes | It equals the finished section's `blocks.is_empty()` for no static lights, all-SDF static lights, and a section-memo hit. |
| P5 | A light whose bounds reach only some of a layer's charts, and a directional light that reaches all | The chart cull runs before the window bakes | Partitions are byte-identical to an unculled bake. Culled charts still count toward the published progress total. |
| P6 | An entry written before build N−1 began and only read by N−1, a successful build of the same map | N−1's hit takes lever 2's cheaper path, with no second open and no mtime bump. N reads the map's record in `construct_stage_cache` before recording its own start. | N's prune keeps it, and spares N−1's record rather than N's empty one. |
| P7 | Build N−1 of a map killed mid-build, after a successful build N−2 of the same map | N−1 read and wrote entries but never reached its end-of-build step | N's prune spares every entry N−1 read or wrote before the kill, and N−2's record. |
| P8 | Build N−1 hits both the lightmap and the shadowmask section memos, over budget | N−1 reads no per-light partition and succeeds, replacing the record; N changes one light's intensity | N's prune keeps every partition the memos summarize. N re-bakes only the edited light's partitions and hits the rest. |
| P9 | One cache directory, budget smaller than map A's set | Build A, then map B, then A | B's prune spares A's record. The third build's prune spares both records, and it has zero misses. |
| P10 | Build of A fails at parse, between two successful builds of A | The failed build constructs and prunes the cache, then touches nothing | A's record is unchanged. The third build evicts the same entries it would without the failed build, and has zero misses. |
| P11 | Lightmap window of two or more lights at one permit | One light's partition cache put, or in a warm case its get, is held open while another light, a cache miss, has chart ray work ready | The chart work admits and runs before the I/O is released; I/O holds no permit. At most one chart item runs at a time. The bake completes with unthrottled bytes. |

## Prior deferrals

- `plans/done/level-compiler-tui` made two choices here:
  - Non-goal: "Parallelizing the serial lightmap stage (it stays single-threaded)".
  - Default core count set below the logical core count.
- `plans/done/lightmap-bake-throughput` parallelized charts within one light. The outer light loop stayed serial.
- `plans/done/shadowmask-bake-scaling` shipped its first slice "serial over the light axis for now". Its Task 2 designed governed, bounded-window light-axis parallelism. The later fused walk folded shadowmask consumption into the lightmap's serial loop.
- `plans/done/perf-parallel-sh-group-bake`: "Follow-up (deferred, owner-accepted)". The per-entry `sync_all` cost about 45 s across about 900 puts on occlusion-test. It was expected to be minor because "the bake is ray-bound". On the hallway, Direct SH Delta is entirely I/O-bound.
- `plans/done/build-stage-cache` assumed a single builder and atomic rename, and did not budget fsync cost.

## Related drafts

These drafts cut ray count or peak RAM. This brief cuts idle cores and per-ray traversal cost. The savings add up.
- `lighting-scale--cold-sh-bake-contribution-early-out`
- `lighting-scale--cold-bake-reaching-light-spike`
- `lighting-scale--sh-delta-cell-major-two-pass-bake`

Resolved overlap:
- `bvh-leaf-clustering` clusters only the shipped BVH section. The live tree and primitive list that every bake ray walks through `traverse_iterator` (`sh_bake`, `lightmap_bake`, `chunk_light_list_bake`, `billboard_direct_scatter_bake`) stay one primitive per face, so bake bytes and lever 5's per-face baseline hold. That draft lands after this brief and re-takes the hallway SH Bake timing; with the per-face bake tree it should not move.
