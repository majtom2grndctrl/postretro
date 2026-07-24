# Lightmap Bake Throughput

## Goal

The lightmap bake casts every shadow ray through a BVH traversal that heap-allocates twice per call, and it casts all of them on one thread while every SH-family bake next door runs under rayon. Remove the per-ray allocation, then parallelize both lightmap bake paths across charts. Cold-build wall clock is the target; output must stay byte-identical.

## Scope

### In scope

- Replace the allocating BVH traversal in the shared shadow-ray primitive with the crate's lazy iterator, preserving its early-out on first hit.
- Apply the same replacement at the remaining bake-time traversal sites.
- Parallelize the monolithic whole-atlas lightmap bake across charts.
- Parallelize the per-light layer bake across charts — the path warm incremental builds actually run.
- Preserve byte-identical output, deterministic across repeated builds and independent of thread count.

### Out of scope

- Lightmap storage, atlas dimensions, direction-atlas format, or per-surface density. Sibling draft `lighting-scale--lightmap-bake-scaling` owns those.
- Compile-time peak RAM and incremental bake-and-flush. Sibling draft `lighting-scale--lightmap-bake-incremental-flush`.
- The SH-family bakes. They are already parallel; they pick up the allocation fix through the shared ray primitive and need no other change.
- Parallelizing BVH construction. The `bvh` crate offers a rayon executor, but the dependency is declared with default features off and the build stage is documented as fast enough to skip caching. See Open questions.
- Changing the bake cache, its keys, or what it stores.
- Changing sampling: ray counts, stratification, seeds, falloff, or the soft-shadow area-sampling model. Any of these would change output bytes, which the acceptance criteria forbid.

## Acceptance criteria

- [ ] The composited pre-BC6H lightmap atlas is byte-identical before and after this change, on every fixture in `content/dev/maps/`.
- [ ] The existing equivalence gate still passes: the per-light layer composite remains byte-identical to the monolithic bake's pre-BC6H atlas.
- [ ] Two builds of the same map produce byte-identical lightmap output, and output does not vary with the number of worker threads.
- [ ] SH, animated-weight-map, chunk-light-list, and entity-shadow-select outputs are byte-identical before and after, since they share the modified ray primitive.
- [ ] The allocation change alone, measured single-threaded, reduces cold lightmap bake wall clock on a heavily-lit fixture by a measurable margin, recorded in this plan.
- [ ] After both changes, cold whole-map lightmap bake wall clock on a heavily-lit fixture drops by at least 4× on a machine with at least 8 physical cores.
- [ ] Bake progress reporting stays monotonic, and the published total still equals the units eventually advanced.
- [ ] Build cancellation and pause remain responsive during a parallel bake — no worker continues past a cancellation for longer than one chart.
- [ ] Peak resident memory during the bake does not exceed the pre-change peak by more than 25%.

## Tasks

### Task 1: Non-allocating shadow-ray traversal

`segment_clear` in `crates/level-compiler/src/lightmap_bake.rs` is the shared shadow-ray primitive: it builds a ray, calls `bvh.traverse(&ray, primitives)`, then walks the returned candidates testing triangles and returning `false` on the first hit within range. In `bvh` 0.11 that `traverse` allocates twice per call — it fills a `Vec` of indices during recursion, then maps that into a second `Vec` of shape references. Switch it to `traverse_iterator`, which yields candidates lazily from the same crate version and needs no cargo feature. This is a double win here: the allocations disappear, and because the function early-returns on the first hit, traversal now stops there instead of first materializing every candidate. Preserve the exact triangle-test order the current code walks, since changing which hit is found first would change nothing for a boolean result but would change it for the distance-returning callers — verify none of the five callers depend on candidate ordering. The callers are the monolithic lightmap bake, the per-light layer bake, `animated_light_weight_maps.rs`, `chunk_light_list_bake.rs`, and `entity_shadow_select.rs`; none of their signatures change.

### Task 2: Non-allocating traversal at the remaining bake sites

Three further bake-time traversal sites call the allocating `traverse` directly rather than going through `segment_clear`: two in `crates/level-compiler/src/sh_bake.rs` and one in `crates/level-compiler/src/chunk_light_list_bake.rs`. Convert each to `traverse_iterator`. These sites already run under rayon, so the allocation is also a source of cross-thread allocator contention, not just per-call overhead. Where a site needs the full candidate set rather than an early-out, it may collect into a caller-owned buffer reused across rays instead of allocating per ray — but do not introduce a reused buffer where a lazy walk suffices.

### Task 3: Parallelize the monolithic atlas bake

`bake_monolithic_atlas_controlled` in `crates/level-compiler/src/lightmap_bake.rs` loops placements sequentially, calling `bake_face_chart` with `&mut` slices of the whole atlas — `irradiance`, `direction`, and `coverage` — which each chart indexes through its own layer offset. Charts occupy disjoint atlas rectangles by construction, so the writes never overlap, but the shared `&mut` slices block direct parallelization. Restructure so each chart bakes into small per-chart local buffers in a rayon parallel map over placements, followed by a sequential scatter into the atlas; the scatter is a memcpy per chart against the ray-casting cost it replaces. Per-texel determinism is already independent of iteration order because the sample-lattice seed is derived from atlas coordinates rather than a sequential counter, so no seed threading is needed. Keep the existing `Governor` checkpoint and the per-chart progress advance inside the parallel body — `Governor` is built on a `Mutex` and `Condvar` and is safe to call from workers. Run the dilation pass after the scatter, unchanged and sequential. Rayon is already a dependency of this crate.

### Task 4: Parallelize the per-light layer bake

`bake_light_layer_controlled` in `crates/level-compiler/src/lightmap_layer.rs` mirrors the monolithic bake's per-texel structure for a single light and is the path warm incremental builds run, so it matters at least as much as Task 3. It loops placements sequentially and pushes `LayerTexel` records onto one growing `Vec`, which makes push order the output order. Restructure as a rayon parallel map over placements producing one `Vec<LayerTexel>` per chart, then concatenate in chart order — rayon's indexed parallel `collect` preserves input order, which is what keeps the output byte-identical rather than merely equivalent. Charts with a degenerate UV extent must still advance progress and contribute an empty vector, matching the current skip-and-advance behavior. Keep the `Governor` checkpoint and progress advance per chart as in Task 3.

### Task 5: Determinism gates and measurement

Run the existing equivalence tests that compare the per-light layer composite against the monolithic bake's pre-BC6H atlas — they live in the `lightmap_layer` test module and already assert bit-for-bit equality — and confirm they pass unchanged. Add a check that a bake run with a single worker thread and a bake run with many produce identical bytes, so thread count is proven not to influence output. Record, in this plan, cold lightmap bake wall clock at three points: before any change, after Tasks 1 and 2 measured single-threaded, and after Tasks 3 and 4; plus peak resident memory before and after, and the core count and CPU of the measuring machine. Separating the allocation and parallelism measurements is what makes the two changes independently justifiable.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — different call sites, no shared edits; both are pure allocation changes.
**Phase 2 (concurrent):** Task 3, Task 4 — different files, independent bake paths, both built on the Phase 1 primitive.
**Phase 3 (sequential):** Task 5 — measures and gates the finished state.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Lightmap output is byte-identical to the pre-change bake | Task 1 (traversal order preserved), Tasks 3–4 (write order preserved) | Any change to sampling, seeding, or accumulation order; float addition is not associative, so per-texel accumulation order must not change | AC 1, AC 4 |
| The layer composite equals the monolithic bake bit-for-bit | Pre-existing gate | Tasks 3 and 4 change the two sides of this equality independently — one landing without the other breaks the gate | AC 2 |
| Output is independent of thread count | Tasks 3–4 (per-chart isolation, order-preserving merge) | An unordered collect, or shared mutable accumulation, silently reintroduces dependence | AC 3 |
| Chart atlas rectangles are disjoint | Pre-existing packer guarantee | Task 3's scatter assumes it; a packer change that allowed overlap would corrupt output rather than fail loudly | AC 1 |
| Progress and cancellation stay correct | Tasks 3–4 (checkpoint and advance kept per chart) | Moving progress calls outside the parallel body would make totals wrong or cancellation unresponsive | AC 7, AC 8 |

## Rough sketch

The two changes are independent but compound. Removing the per-ray allocation is worth doing on its own — it is a few lines in a shared primitive and it lifts every bake stage, including the ones already parallel. Doing it first also matters for the parallel work: handing eight threads a function that mallocs twice per ray converts a latency problem into an allocator contention problem, and much of the parallel gain would be spent there.

The parallel structure is the same on both paths: rayon map over charts into per-chart local output, then an ordered merge. The monolithic path merges by scattering into atlas slices; the layer path merges by concatenating texel vectors. Determinism survives because the per-texel seed is a function of atlas coordinates, not of visit order — that property already exists and is the reason this is a tractable change rather than a rewrite.

The measured baseline to beat: the incremental-bake plan records a cold build at 228 s with the SH bake's 631 s since moved behind a cache, so the lightmap bake is a large share of what cold builds still pay.

## Open questions

- The `bvh` dependency is declared with `default-features = false`, which switches off the crate's rayon feature and with it the `rayon_executor` for parallel BVH construction. `traverse_iterator` needs no feature, so this plan is unaffected — but whether to enable the feature for parallel tree builds is a separate call. The build pipeline documents the BVH stage as fast enough to skip caching, which argues against bothering.
- The 4× target in the acceptance criteria assumes chart count greatly exceeds core count and that charts are roughly balanced. A map dominated by one enormous chart will parallelize poorly. If Task 5 finds a fixture like that, the fix is splitting within a chart by texel rows, which is a larger change and should become its own plan rather than expanding this one.
- Peak memory rises with worker count, since each worker holds per-chart buffers. The 25% bound in the acceptance criteria is a guess; if it proves tight on large atlases, bounding rayon's thread count for this stage is the cheaper answer than restructuring the merge.
