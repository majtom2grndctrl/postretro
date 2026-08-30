# Warm-Cache Delta-SH and Graph Bakes

## Goal

Give the five currently-uncached expensive compile stages `StageCache` coverage so a warm rebuild of an
animated-light-heavy map skips the ray-traced work it already did. Today a no-edit warm rebuild of
`campaign-test.map` (18 animated lights) saves only ~20 s because the three per-light SH *delta* bakes plus
the `CellVisibility` and `ChunkLightList` graph bakes recompute in full every build. Caching them at the
right grain makes iteration fast without touching runtime output.

## Scope

### In scope

- Per-CSR-entry (affinity-cell × light) caching for the three SH delta bakes: `DeltaShBake`,
  `AnimatedDirectShBake`, `DirectShDeltaBake`. Exact / byte-identical — no warm approximation.
- Whole-section caching for `CellVisibility` and `ChunkLightList`.
- A shared per-entry cache helper (new module) driving the delta trio, so the large bake files grow minimally.
- Threading the existing `stage_cache: Option<StageCache>` into all five stages at their `pipeline.rs` call sites.
- Preserving the exact uncached path under `--no-cache` / `--release` (cache neither read nor written).
- `build_pipeline.md` updates at promotion (the "delta bakes are cache-less" and "participating stages"
  statements — see Direction).

### Out of scope

- Per-chunk or per-cell caching of `ChunkLightList` / `CellVisibility`. They are whole-section here; their
  inputs (geometry topology, portals, static light set) change coarsely, so section grain is the right fit.
- Any warm/cold *approximation* for the delta channel. Per-entry delta caching is exact; the "judge final
  lighting on `--release`" contract is unchanged and applies as before only to the base indirect SH volume.
- Caching Parse / BSP / Visibility / Geometry / BVH — they are mandatory inputs to every downstream key and
  fast enough that the existing doc rationale still holds.
- Algorithmic speedups to any bake. This is cache coverage only; bake math is untouched.
- Changing `AFFINITY_FACTOR`, the affinity/reach decomposition, or the drop policy.

## Direction

**Problem.** Three per-light SH delta bakes and two whole-graph bakes were never given `StageCache`
integration, so they re-run their per-probe ray tracing on every build. Recent carried-dynamic-light work
routes more animated lights into the delta bakes, and `CellVisibility`/`ChunkLightList` are recent additions
that postdate the cache design. On a map with many animated lights these five stages dominate a warm build,
so caching the cheap stages (lightmap, base SH) buys almost nothing. Observed: `campaign-test.map` warm
rebuild only ~20 s faster than cold.

**Prior commitments.** `build_pipeline.md` commits to the delta path being cache-less: "The SH
indirect-bounce delta path is cache-less but low-frequency" and "Delta bakes are invoked directly from the
compiler rather than through the build cache." This spec **diverges**: the delta path is *not* low-frequency
for animated-light-heavy maps, and it is the dominant warm-build cost, so it earns caching. The divergence is
safe because per-entry delta caching is byte-identical (see below) — it changes build time, never output. The
doc's "participating stages" list ("Parse, BSP, portals, geometry, and BVH run uncached — they are fast
enough") predates `CellVisibility` and `ChunkLightList`; adding those two to the cached set does not
contradict it. The existing per-element caching philosophy (lightmap per-light layers, SH per-4³-group,
whose stated purpose is "Bounding the light set is what localizes a light edit") is the commitment this spec
*follows* — the delta caches extend the same fine grain so a single animated-light edit re-bakes only that
light's sub-blocks.

**Why exact, not warm-approx.** Each delta CSR entry bakes a single light in isolation, so bounding a
light's reach never changes a kept cell's radiance (culled cells are provably zero). Soft-visibility is
seeded off each light's global static/source index (direct bakes) or a stable position-derived probe index
(indirect bake), so a per-entry payload is byte-identical across processes and across bounded-vs-full light
sets. Per-entry caching is therefore exact — strictly better than the warm-approx path this work was scoped
to accept, and it keeps the delta channel out of the "approximate warm SH" carve-out entirely.

**Alternatives rejected.** *Whole-section caching for the delta bakes* (one wrapper each, mirroring the base
`direct_sh_volume` template) is simpler and also exact, and it fully fixes the *reported* no-edit rebuild.
Rejected as the primary shape because its invalidation is coarse: editing any one of 18 animated lights
re-bakes all of them, making the edit→rebuild iteration loop — the actual dev workflow — no faster than
today. That regresses below the fine grain the sibling lightmap/SH caches already provide. Per-entry grain
costs a shared helper and the reassemble-then-drop seam, and buys localized single-light edits, matching the
rest of the cache design.

*Phasing* — ship whole-section now and defer the per-entry grain to a follow-up gated on a measured
edit-loop cost — was weighed and declined by the owner: the per-entry grain is the house grain (lightmap
per-light, SH per-4³-group), the edit-light-then-rebake loop is the documented lighting-dev workflow, and
reversibility is high (a disposable cache and one helper module), so building the right grain now costs
little and avoids a second pass. Per-entry stays in this spec.

## Acceptance criteria

- [ ] A warm no-edit rebuild of `campaign-test.map` registers a cache hit for every present stage among
      `DeltaShBake`, `AnimatedDirectShBake`, `DirectShDeltaBake`, `CellVisibility`, `ChunkLightList` — the two
      whole-section wrappers via their `[cache] … hit` log line, the three delta stages via the helper's
      returned all-hit tally (they carry no per-stage log). That the freed per-probe ray work drops each
      stage's Build-Summary time to decode-only is the intended human-observable effect, checked by inspection,
      not an automated metric.
- [ ] For each of the five sections, warm (cached) output is **byte-identical** to the `--no-cache` output
      for the same map — for the three delta sections, after the full downstream post-bake pipeline (exact-zero
      drop, coarsening, valid-probe compaction, payload cap), not merely after the drop. Verified by a test
      that bakes each section both ways and compares serialized bytes.
- [ ] Editing a single light's parameters and rebuilding re-bakes only that light's delta CSR entries; every
      unchanged light's entries are served from cache. Verified against the helper's hit/miss tally in a test,
      not by wall-clock — for `DeltaShBake` and `AnimatedDirectShBake` (both animated-light-driven) by editing
      one animated light, and for `DirectShDeltaBake` (driven by selected entity-shadow *static* lights, keyed
      on `static_index`) by editing one selected entity-shadow static light. An animated-light edit produces no
      `DirectShDeltaBake` misses, so its localization must be checked with a static-light edit.
- [ ] A light-only edit (no geometry/portal change) yields a `CellVisibility` cache hit — its key excludes
      lights. A dynamic-light-only edit yields a `ChunkLightList` cache hit — its key covers only static lights.
- [ ] `--no-cache` and `--release` runs neither read nor write any of the five new cache entries and produce
      output identical to a build with the cache directory deleted.
- [ ] Each stage's `stage_version` is folded into its `CacheKey`, so two versions produce different keys: an
      entry written at version N misses at version N+1, then hits again at N+1. Verified per stage by a
      key-difference test (a compile-time const cannot be bumped mid-run), which needs each stage's key builder
      reachable from tests.
- [ ] A corrupted or truncated cache entry for any of the five stages is treated as a miss (rebake), not a
      crash — matching the existing `StageCache` corruption contract.

## Tasks

### Task 1: Per-entry delta cache helper + indirect delta bake (thin slice)

Add a shared per-CSR-entry cache module (new file, e.g. `delta_sh_cache.rs`) and wire the **indirect** delta
bake through it end to end, proving the reassemble-then-drop seam. The delta bakes share one shape: a
cell-keyed CSR (`affinity_grid::build_csr` → `affinity_offsets`, `affinity_lights`; `csr_entry_cells` gives
each entry's cell), then one dense 64-probe sub-block baked per entry from a single light in isolation. The
helper takes the CSR (`affinity_lights`, per-entry cell indices, `affinity_dims`), the whole-map geometry
fingerprint (`sh_group::geometry_content_hash`, `pub(crate)`), `probe_spacing`, per-cell probe validity, a
stage id + version, an `Option<&StageCache>`, a `&BakeControl`, and a per-entry bake closure; for each entry
it builds a `CacheKey` folding `{ geometry hash, affinity_dims, entry cell coord, probe_spacing, that cell's
probe validity bytes, the single light's `postcard` encoding, and the light's `u64` seed index (the axis its
soft-visibility seed is derived from — `static_index` for the direct entity-shadow bake, `source_index` for
the animated-direct bake; the indirect bake's seed is `cell * PROBES_PER_CELL + local` (affinity cell index +
in-cell local slot), both captured by the folded cell coord + affinity_dims, so it folds `0`) }`, returns the
cached sub-block bytes on hit, else calls the closure and `put`s the result. The two **unitizing** bakes
(indirect and animated-direct) force `color=[1,1,1]`, `intensity=1.0` before tracing, so their sub-block bytes
are independent of authored color/intensity: fold the *unitized* light (color/intensity normalized to unit)
into their keys, not the raw postcard, so an authored color/intensity edit still hits. The direct entity-shadow
bake reads the light's color/intensity, so it folds the full postcard. The per-cell probe validity the helper
folds is the 64-bit mask of which of the cell's 64 probes are valid; `delta_sh_bake` computes it with the same
`probe_is_valid_pub(tree, exterior_leaves, pos)` gate the closure applies while baking (the indirect bake
gates probes inline and does not build this mask today), so the key captures the tree / exterior-leaf
dependence that `geometry_content_hash` does **not** cover (see `direct_cache_key`'s per-probe validity fold).
It returns sub-blocks in the exact
CSR-entry order so the caller reassembles the pre-drop section unchanged, plus a small hit/miss tally
(entries served from cache vs baked) so tests and callers can assert localization without wall-clock timing.
With `cache == None` it bakes every entry and reports every entry as a miss (the `--no-cache`/`--release`
path). Then change `delta_sh_bake::bake_delta_sh_volumes_controlled` to
add an `Option<&StageCache>` parameter and drive its existing `par_iter` sub-block bake through the helper
(each closure invocation is the current `bake_subblock` body); assemble the pre-drop `DeltaShVolumesSection`
exactly as today and return it. The bake does **not** run the drop: the existing post-bake delta pipeline in
`pipeline.rs` (`delta_sections::apply_exact_zero_drop_policy`, then coarsening, valid-probe compaction, and
`enforce_payload_cap`) runs unchanged, downstream of both the cache and the reassembly. The cache sits at
pre-drop sub-block grain, so `ScriptMutableDescriptorSlots` (a drop input derived in `pipeline.rs`, not a bake
input) must never enter a cache key. Thread `stage_cache.as_ref()` into the `DeltaShBake` call site in
`pipeline.rs` (the block building `DeltaBakeInputs`, guarded by non-empty animated lights). Keep the
`BakeControl` accounting correct: the delta path advances **per entry**, not per section, so `advance(1)` must
fire for every CSR entry whether it hits or misses (today it lives inside the `par_iter` body that becomes the
bake closure — the helper must lift it out so hits still advance), with `publish_total(affinity_lights.len())`
emitted once. Take the governor permit only on a miss, around the closure call (a `governor().checkpoint()` on the hit path
keeps pause responsive on an all-hit rebuild; not AC-load-bearing). (Do not copy
`bake_direct_sh_volume_cached_controlled`'s re-emit literally — that is a whole-section skip re-emitting
`advance(total)` once; here the grain is per entry.) Define a new `stage_id`/`stage_version` for this bake (none exist in the module today;
`DIRECT_SH_STAGE_ID` is taken by the base direct bake). The three delta bakes' `stage_id`s must be distinct
string literals: all three sub-block payloads are identically-shaped `Vec<u16>`, and a seed-0 light folds the
same `0` seed in every bake, so the `stage_id` is the sole guard against one bake's entry being served for
another — a duplicated id would cross-serve silently, and no per-section byte-identity check (each stage baked
in isolation) would catch it (Task 6's shared-cache cross-bake test does). This is the thin slice: it falsifies the
per-entry-cache → reassemble → drop boundary before the other two bakes fan out. AC: byte-identity (indirect
section), single-light localization, `--no-cache` path, version bump, corruption.

### Task 2: Animated-direct delta bake through the helper

Route `animated_direct_sh_bake::bake_animated_direct_sh_delta_volumes_controlled` through the Task 1 helper.
Add an `Option<&StageCache>` parameter; its per-entry bake closure is the current `bake_direct_subblock` body
(single unit-radiance light, `sh_bake::bake_probe_direct_rgb` seeded off the light's global source index).
Reassemble the pre-drop `AnimatedDirectShDeltaVolumesSection`; the existing downstream `delta_drop_policy`
animated-direct drop (a `pipeline.rs` pass, not run by the bake) is unchanged. The per-entry cache stores raw
sub-block payload bytes, not section-encoded bytes, so the cache read/write path never touches the section
codec. Where the whole section is serialized for the byte-identity comparison (Task 6), use `try_to_bytes()`
rather than `to_bytes()`, which panics on an invalid wire payload for this section type; decode with
`from_bytes`. Thread `stage_cache.as_ref()` into
the `AnimatedDirectShBake` call site in `pipeline.rs` (guarded by non-empty animated lights). Define a
distinct `stage_id`/`stage_version` for this bake. Preserve the `BakeControl` hit-path accounting as in
Task 1. This task only adds a call site plus a small amount of section-assembly glue in
`animated_direct_sh_bake.rs`; it does not modify the shared helper. AC: byte-identity (animated-direct
section), single-light localization, `--no-cache` path, version bump, corruption.

### Task 3: Direct (entity-shadow) delta bake through the helper

Route `direct_sh_bake::bake_direct_sh_delta_volumes_controlled` through the Task 1 helper. Add an
`Option<&StageCache>` parameter; its per-entry closure is the current `bake_direct_delta_subblock` body
(single selected static light, seeded off the light's global static index). This bake returns
`Option<(DirectShDeltaVolumesSection, DirectDeltaBakeStats)>`: cache the per-entry sub-block bytes (the
helper's grain) and reassemble the **pre-drop** section from cached/baked sub-blocks, exactly as the bake
produces it today. The bake does **not** run the drop — the existing direct drop is part of the downstream
`pipeline.rs` pass (`delta_sections::apply_exact_zero_drop_policy`, which retains each selection's highest
canonical cell record even when zero), unchanged. **Recompute** `DirectDeltaBakeStats` from the reassembled
**pre-drop** section plus the `selected` light list the bake already builds: the per-selection CSR-entry
counts and byte totals come from the pre-drop section, but each row's `static_index` is **not** in the section
(the section stores selection slots, not static indices — `static_index` lives on the selected light), so the
recompute reads `selected`. It needs no cache because both the pre-drop section and `selected` are rebuilt
every build, and the values match today's pre-drop stats byte-for-byte. The selection set comes from the
`EntityShadowLightsSection` and `AlphaLightsNs` the function already takes; fold the selected lights (postcard
+ `u64` global static index) into the per-entry keys exactly as the other two bakes fold their lights. Thread
`stage_cache.as_ref()` into the `DirectShDeltaBake` call site in `pipeline.rs` (inside the
`raw_entity_shadow_lights_section.as_ref().and_then(...)` block). Define a distinct `stage_id`/`stage_version`
— it must differ from `DIRECT_SH_STAGE_ID` (the base direct bake in the same module). Preserve `BakeControl`
hit-path accounting. AC: byte-identity (direct-delta section, including recomputed pre-drop stats consistency
— warm `DirectDeltaBakeStats` equal the `--no-cache` build's), single-light localization, `--no-cache` path,
version bump, corruption.

### Task 4: Whole-section CellVisibility cache

Wrap `cell_visibility_bake::cell_visibility_bake(tree, portals, control)` in a whole-section `StageCache`
memo. The section depends only on BSP leaf topology and the portal graph — **no lights** — so its cache key
folds exactly what the bake reads from `tree` and `portals`: `{ leaf count, per-leaf `bounds` + `is_solid`,
the portal front/back leaf adjacency pairs, per-portal polygon centroid + `minimum_width` (aperture), and the
fixed `CELL_VISIBILITY_DISTANCE_CAP` / `CELL_VISIBILITY_FANOUT_K` constants }` (a hash of those; the hashing
mechanism is the implementer's). Do **not** key on `sh_group::geometry_content_hash` here: it hashes
`GeometryResult` (render geometry only — vertices, faces, texture names), which by construction excludes the
BSP tree, leaf solidity/exterior status, and portal polygons — the shipped `direct_sh_bake::direct_cache_key`
folds per-probe validity separately for exactly this reason ("derives from tree / exterior_leaves, which the
geometry content hash does not cover"). The coupling pass grades each kept pair by distance and aperture from
portal-polygon centroids / `minimum_width` and leaf `bounds` (`cell_visibility_bake::portal_edges` →
`portal_metrics`, plus the leaf-centroid distances in `portal_hub_graph`) and gates on leaf `is_solid`, so a
structural edit that changes the tree/portals while leaving render geometry — and thus a geometry hash —
unchanged (a clip or non-rendered brush, a solidity flip, a moved portal) must still miss. Fold the portals in
input (`portal_index`) order, not as a canonicalized set: `maximum_spanning_tree` breaks equal-aperture ties
by `PortalEdge.portal_index` (the portal's slot in the input slice), so the fold must cover that order to stay
total over the bake's inputs. Lights stay absent from the key, so a light-only edit still hits.
Mind the payload API: `to_bytes()` is **fallible** (`-> Result`) and `from_bytes(data, expected_cell_count)`
takes an extra `expected_cell_count: u32` — pass `tree.leaves.len() as u32` on the hit-decode path. On a hit,
return the decoded section's `to_bytes()` bytes as today (the call site currently does
`cell_visibility_bake(...)?.to_bytes()?`); a decode/validation failure is a miss (rebake), matching the
`StageCache` corruption contract. Thread `stage_cache.as_ref()` into the `CellVisibility` call site in
`pipeline.rs`, and preserve the existing `BakeControl` progress accounting on both hit and miss (the bake
publishes `cell_count * 2` units). On hit and miss, emit the `[cache] cell_visibility hit`/`miss` line matching
the existing whole-section wrapper convention (`sh_group.rs`, `shadowmask_bake.rs`) — this is the per-stage
signal AC #1 checks by inspection. Define a new `stage_id`/`stage_version`. Because lights are absent from the
key, this stage hits on every light-only edit — the common iteration case. AC: byte-identity, light-only-edit
hit, `--no-cache` path, version bump, corruption.

### Task 5: Whole-section ChunkLightList cache

Wrap `chunk_light_list_bake::bake_chunk_light_list(inputs, cell_size, cap)` in a whole-section `StageCache`
memo. It returns `Result<ChunkLightListSection, ChunkLightListError>` and takes **no `BakeControl`** (serial
bake), so its wrapper has no worker to re-`publish_total` against. The cache key folds
`{ geometry content hash (`sh_group::geometry_content_hash`), a BSP-`tree` fingerprint covering leaf
solidity/exterior status and the point-location partition `find_leaf_for_point` walks, the portal front/back
leaf adjacency pairs, the ordered `postcard` encoding of the static (`!is_dynamic`) light set in its compacted
bake order, and the `cell_size_meters` / `per_chunk_cap` parameters }`. Fold the compacted static set **in
order** — not each light's `AlphaLightsNs` global index: the bake reads only the compacted `!is_dynamic` slot
(`enumerate()` over the filtered set) and seeds nothing off a global index, so a dynamic-light
add/remove/reorder shifts static lights' global indices without changing the bake output, and folding the
global index would force a false miss on exactly the dynamic-light-only edit this stage is meant to hit. The
tree fingerprint is required because the bake's solid/exterior bypass (`find_leaf_for_point` + `leaf.is_solid`
+ `exterior_leaves`) reads leaf solidity/exterior, which `geometry_content_hash` does **not** cover (same
rationale as `direct_cache_key`'s validity fold — render geometry alone is not enough). Because only static
slots feed the bake, a dynamic-light-only edit reuses the cached section. Preserve the existing error path: a genuine
`ChunkLightListError::PayloadTooLarge` from a miss-path bake still surfaces as a build error; a cache
decode/validation failure is a soft miss (rebake). On hit and miss, emit the `[cache] chunk_light_list
hit`/`miss` line matching the existing whole-section wrapper convention (`sh_group.rs`, `shadowmask_bake.rs`) —
the per-stage signal AC #1 checks by inspection. Thread `stage_cache.as_ref()` into the `ChunkLightList`
call site in `pipeline.rs`. Define a new `stage_id`/`stage_version`. AC: byte-identity, dynamic-light-only
hit, `--no-cache` path, version bump, corruption.

### Task 6: Cross-bake verification suite

Add the tests that verify the invariants the five caching tasks share, so no single task owns the whole
determinism story. Cover, for each of the five sections: warm-vs-`--no-cache` byte-identity (bake through a
real temp-dir `StageCache`, then with `None`, compare serialized bytes — for the three delta sections, after
the full downstream post-bake pipeline, not merely post-drop); the stage-version-bump miss-then-hit contract;
and corruption-is-a-miss. For the three delta bakes, add the single-light-edit localization check: bake, edit
one light, rebake through the same cache, and assert only that light's CSR entries missed while the rest hit
(per-entry hit/miss counts). Edit the kind of light each bake consumes: an animated light for `DeltaShBake`
and `AnimatedDirectShBake`, a selected entity-shadow *static* light for `DirectShDeltaBake` (an animated-light
edit produces zero `DirectShDeltaBake` misses, so it cannot localize that bake). For `CellVisibility`, assert a
light-only edit hits; for
`ChunkLightList`, assert a dynamic-light-only edit hits. Write one test per row of the **Ordering pins** table
below (P1–P15) — those rows enumerate the collision, boundary-crossing, zero/empty, and radiance-seam orderings
this suite must nail down rather than restate here. P15 in particular (the seed-0 cross-bake collision) is
realized by baking at least two (ideally all three) delta stages through a **single** shared temp-dir
`StageCache` in one run and asserting cross-stage non-interference — this is what catches a duplicated
`stage_id`, which per-section byte-identity (each stage baked in isolation) cannot. Two harness notes: the full-downstream byte-identity check (P12) needs a base
`OctahedralShVolumeSection` whose grid matches the delta affinity dims plus a
`ScriptMutableDescriptorSlots::empty(slot_count)` to drive `PostBakeDeltaSections`' drop → coarsening →
valid-probe compaction → payload cap (both reachable in-crate); and the version-bump contract is a
key-difference test — a compile-time const cannot be bumped mid-run, so each stage's key builder must be
test-visible and version N vs N+1 must yield different `CacheKey`s. These consume all five stages' cache paths,
so this task runs last.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies the per-entry-cache → reassemble → drop boundary and
delivers the shared helper the delta bakes consume.
**Phase 2 (concurrent):** Task 2, Task 3, Task 4, Task 5 — Tasks 2/3 consume the Task 1 helper but only add
call sites in their own bake files (they do not modify the helper); Tasks 4/5 are independent whole-section
wrappers in unrelated files. All four touch distinct files.
**Phase 3 (sequential):** Task 6 — verification suite; consumes every stage's cache path.

## Rough sketch

- New module `delta_sh_cache.rs`: `fn cache_or_bake_subblocks(...) -> Vec<SubBlockBytes>` (or writes directly
  into the flat payload buffer in CSR-entry order), parameterized by stage id/version, the CSR
  (`affinity_lights`, `csr_entry_cells`, `affinity_dims`), `geometry_content_hash`, `probe_spacing`, a
  per-cell validity accessor, `Option<&StageCache>`, `&BakeControl`, and a
  `FnSync(cell, &MapLight, seed_index: u64) -> SubBlockBytes` closure. Key per entry:
  `CacheKey::new(stage_id, stage_version, blake3(geom_hash || affinity_dims || cell_coord || probe_spacing ||
  cell_validity_bytes || u64(seed_index) || len-prefixed postcard(light)))`, where `seed_index` is the bake's
  soft-visibility seed axis and the fold must equal it: `static_index` for the direct bake, `source_index` for
  the animated-direct bake (two identical-postcard lights with different seeds must key differently, or one
  stale sub-block serves both); the indirect bake's seed is `cell * PROBES_PER_CELL + local` (affinity cell
  index + in-cell local slot, both captured by `cell_coord` + `affinity_dims`), so it folds `0`. The two
  unitizing bakes (indirect, animated-direct) fold the *unitized* light (color/intensity normalized to unit),
  not the raw `postcard(light)`, since they force unit color/intensity before tracing; the direct entity-shadow
  bake reads color/intensity and folds the full postcard. Mirror `direct_sh_bake::direct_cache_key`'s fold
  discipline (`to_le_bytes`, length-prefixed postcard, no HashMap/paths/timestamps).
- Delta call sites in `pipeline.rs`: `DeltaShBake` (~line 835), `AnimatedDirectShBake` (~line 949),
  `DirectShDeltaBake` (~line 1012). Graph call sites: `CellVisibility` (~line 435), `ChunkLightList`
  (~line 1215). `stage_cache: Option<StageCache>` is a pipeline param; pass `stage_cache.as_ref()`.
- Geometry fingerprinting differs by stage, because `sh_group::geometry_content_hash` hashes `GeometryResult`
  (render geometry only) and never covers BSP leaf solidity/exterior or portal polygons — see
  `direct_cache_key`. The three **delta** keys fold `geometry_content_hash` **plus** the per-cell probe
  validity mask (which carries the tree/exterior dependence). `CellVisibility` folds the `tree`+`portal` reads
  directly (leaf bounds/solidity, portal adjacency + polygon centroid/aperture), not `geometry_content_hash`.
  `ChunkLightList` folds `geometry_content_hash` plus a tree solidity/exterior fingerprint.
- `BakeControl` accounting is **per entry** for the delta bakes: `publish_total(N)` once, `advance(1)` per
  CSR entry regardless of hit/miss (lift the advance out of the bake closure), governor permit taken only on a
  miss. This is not the whole-section skip-and-re-emit of
  `direct_sh_bake::bake_direct_sh_volume_cached_controlled`. `CellVisibility` keeps its `cell_count * 2`
  whole-section accounting on hit and miss; `ChunkLightList` has no `BakeControl`.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Warm (cached) output byte-identical to `--no-cache`, all five sections, including the full downstream delta pipeline (drop → coarsening → valid-probe compaction → payload cap) | Tasks 1–5 | Any real bake input not folded into a key → stale warm output; the downstream drop/compaction passes must stay downstream of the cache | AC "byte-identical"; Task 6 |
| Per-entry delta cache is exact (single-light entries, per-bake seed axis: `static_index`/`source_index`/position) | Existing bake code; preserved by Tasks 1–3 | A refactor that changes the soft-visibility seed axis, folds an index other than the seed axis, or sums >1 light per sub-block, breaks byte-identity | AC "byte-identical"; Task 6 |
| Light-fold matches what each bake reads: the two unitizing bakes fold the unitized light (color/intensity normalized), the direct entity-shadow bake folds the full postcard | Task 1; Tasks 2–3 | Folding raw postcard for a unitizing bake forfeits the recolor hit (perf); folding the unitized light for the direct bake serves stale output (correctness — it reads color/intensity) | P14 |
| `ScriptMutableDescriptorSlots` never enters a cache key; the drop is a downstream `pipeline.rs` pass over the reassembled pre-drop section, not run by the bakes | Tasks 1–3 | Folding mutable-slot state into a delta key would desync warm/cold and miss script-curve changes | Task 6 delta byte-identity |
| Single-light edit re-bakes only that light's CSR entries (an animated light for the two animated-driven bakes; a selected entity-shadow static light for `DirectShDeltaBake`) | Task 1 helper; Tasks 2–3 | A key that folds the whole light set (not the per-entry light) would coarsen invalidation | AC "single light"; Task 6 (P7, P14) |
| `CellVisibility` key excludes lights but folds the `tree`+`portal` reads (leaf bounds/solidity, portal adjacency + polygon centroid/aperture) — not `geometry_content_hash`, which omits BSP/leaf solidity; `ChunkLightList` folds a tree solidity/exterior fingerprint + the static set by compacted order (not global index) | Task 4, Task 5 | Keying cell-vis on `geometry_content_hash` alone (render geometry) misses a solidity/portal-polygon change → stale coupling; folding lights into the cell-vis key, or dynamic lights / global indices into the chunk key, would miss on benign edits | AC "light-only / dynamic-only hit"; Task 6 |
| `--no-cache`/`--release` neither read nor write the five entries; exact uncached path | Tasks 1–5 | A wrapper that consults the cache before checking `Option<&StageCache>` is `None` | AC "--no-cache"; Task 6 |
| Corrupt entry is a miss, not a crash | Tasks 1–5 | Fallible `to_bytes`/`from_bytes` (CellVisibility) and panic-on-invalid `to_bytes` (animated-direct) mishandled | AC "corruption"; Task 6 |

## Ordering pins

Each row is a concrete `(scenario, ordering, expected outcome)` the suite must nail down; Task 6 writes one
test per row. These pin the collision, boundary-crossing, and zero/empty orderings the invariants imply but
the prose does not otherwise make checkable.

| # | Scenario | Ordering under test | Expected outcome |
|---|---|---|---|
| P1 | Structural edit that shifts a portal polygon / leaf bounds / leaf solidity, **leaf count and every front/back leaf index pair unchanged** (include a clip- or non-rendered-brush edit that does not change render geometry) | build A caches `CellVisibility` → build B, changed tree/portal geometry | Build B **misses** and rebakes; `coupled_pairs` differ from A (the folded tree+portal fingerprint changed even though `geometry_content_hash` may not have). |
| P2 | Light-only edit (no geometry change) | build A caches `CellVisibility` → build B edits a light | Build B **hits**; holds simultaneously with P1. |
| P3 | Direct-delta warm hit, stats reconstruction | reassemble pre-drop section from cache → recompute stats from pre-drop section + `selected` | Recomputed `DirectDeltaBakeStats` equal the `--no-cache` build's and today's pre-drop values (`csr_entry_count`, `byte_total`, `total_bytes`); computed **before** `apply_exact_zero_drop_policy`. |
| P4 | Dynamic-light **add**: insert an `is_dynamic` light ahead of a static light in `AlphaLightsNs` | build A `[S,D]` → build B `[D2,S,D]` | `ChunkLightList` **hits** in B (compacted static slot of S unchanged, output identical). |
| P5 | Dynamic-light **param** edit only | build A → build B edits D's brightness | `ChunkLightList` **hits** (pin explicitly — this case hits even under the flawed global-index fold, so it must not stand in for P4). |
| P6 | Warm all-hit delta rebuild (no edit) | every CSR entry served from cache | `progress.total() == affinity_lights.len()` **and** `progress.completed() == affinity_lights.len()` (advance fires per entry on hits). |
| P7 | Warm partial-hit delta rebuild (edit one light of the kind the bake consumes: an animated light for `DeltaShBake`/`AnimatedDirectShBake`, a selected entity-shadow static light for `DirectShDeltaBake`) | that light's entries miss, others hit | Only that light's CSR entries rebake (hit/miss tally); reassembled pre-drop section byte-identical to `--no-cache`; `completed() == total == N` regardless of the hit/miss split. |
| P8 | N=0 animated set / all lights cull to zero cells, cache present | `affinity_lights` empty | `publish_total(0)`, zero gets/puts, zero advances; section assembled with empty CSR (or `None` when the animated set / geometry is empty); no panic. |
| P9 | `ChunkLightList` with zero static lights (`static_slots.is_empty()`) | whole-section cache wrap | Returns the placeholder; key folds the empty static set; warm hit returns the placeholder; a genuine `PayloadTooLarge` still surfaces from a miss-path bake; a decode failure is a soft miss. |
| P10 | `CellVisibility`, portal aperture change at minimum grid (1 leaf) | key must include aperture-affecting geometry | miss → rebake (guards P1 at the smallest grid). |
| P11 | Direct-delta selection retained-at-zero by the drop | pre-drop stats vs any post-drop recompute | Stats taken **pre-drop**: a selection whose entries are all zero-but-one-retained shows the pre-drop count, and `log_direct_sh_delta_stats`'s `.rows.first().expect(...)` never panics. |
| P12 | Byte-identity through the full downstream pipeline | reassemble → exact-zero drop → coarsening → valid-probe compaction → payload cap | Serialized bytes of each of the three delta sections after the **entire** post-bake pipeline are identical warm vs `--no-cache`. |
| P13 | Cross-build seed-axis stability, animated-direct | add/remove a static (non-source) light that shifts an animated light's `source_index` | animated-direct entries for that light **miss** (seed genuinely changed → output changed); the folded key axis is `source_index`, matching the seed. |
| P14 | Authored `color`/`intensity`-only edit of one animated light | build A caches → build B edits only that light's `color` (or `intensity`) | `DeltaShBake` + `AnimatedDirectShBake` entries for that light **hit** (both unitize before tracing → transport unchanged; keys fold the unitized light); `DirectShDeltaBake` entries for an edited *static* light with a color/intensity change **miss** (it reads color/intensity). Pins the transport-vs-authored-radiance seam. |
| P15 | Seed-0 cross-bake collision, one shared `StageCache` | an animated light with `source_index == 0` and a same-cell, same-postcard indirect entry — animated-direct folds seed `0`, indirect folds seed `0`, identical `{geom_hash, affinity_dims, cell_coord, spacing, validity, unitized postcard}` | **No cross-serve**: the two payloads stay distinct solely because their `stage_id`s differ. A duplicated `stage_id` cross-serves silently — this is the collision the Task 6 shared-cache test guards. |

## Open questions

- **Resolved — one shared `delta_sh_cache` helper.** Source review confirms the three delta bakes share the
  per-entry CSR + single-isolated-light sub-block shape, and their three points of divergence all sit outside
  the helper's grain: section assembly (each bake returns its own `*Section` type) and the direct bake's
  `DirectDeltaBakeStats` recompute live in each bake's own glue (Tasks 2–3), and the per-bake seed axis is
  carried through the closure's `seed_index` parameter. The helper owns only the key fold, get/put,
  CSR-order reassembly, the hit/miss tally, and per-entry `BakeControl` advance — no near-duplication remains
  to justify the inline-per-bake fallback. Task 1 builds the shared helper.
