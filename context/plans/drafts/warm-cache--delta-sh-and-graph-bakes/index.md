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

- [ ] A warm no-edit rebuild of `campaign-test.map` logs a cache hit for every present stage among
      `DeltaShBake`, `AnimatedDirectShBake`, `DirectShDeltaBake`, `CellVisibility`, `ChunkLightList`, and
      each of those stages' Build-Summary times drops to decode-only (no per-probe ray work).
- [ ] For each of the five sections, warm (cached) output is **byte-identical** to the `--no-cache` output
      for the same map — including after the delta drop pass. Verified by a test that bakes each section both
      ways and compares serialized bytes.
- [ ] Editing a single animated light's parameters and rebuilding re-bakes only that light's delta CSR
      entries; every unchanged animated light's entries are served from cache. Verified against the helper's
      hit/miss tally in a test, not by wall-clock.
- [ ] A light-only edit (no geometry/portal change) yields a `CellVisibility` cache hit — its key excludes
      lights. A dynamic-light-only edit yields a `ChunkLightList` cache hit — its key covers only static lights.
- [ ] `--no-cache` and `--release` runs neither read nor write any of the five new cache entries and produce
      output identical to a build with the cache directory deleted.
- [ ] Bumping any one of the five new `stage_version` constants makes the next build miss that stage and
      re-bake, then hit on the following build. Verified per stage.
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
stage id + version, an `Option<&StageCache>`, and a per-entry bake closure; for each entry it builds a
`CacheKey` folding `{ geometry hash, affinity_dims, entry cell coord, probe_spacing, that cell's probe
validity bytes, the single light's `postcard` encoding, and the light's global index }`, returns the cached
sub-block bytes on hit, else calls the closure and `put`s the result. It returns sub-blocks in the exact
CSR-entry order so the caller reassembles the pre-drop section unchanged, plus a small hit/miss tally
(entries served from cache vs baked) so tests and callers can assert localization without wall-clock timing.
With `cache == None` it bakes every entry and reports every entry as a miss (the `--no-cache`/`--release`
path). Then change `delta_sh_bake::bake_delta_sh_volumes_controlled` to
add an `Option<&StageCache>` parameter and drive its existing `par_iter` sub-block bake through the helper
(each closure invocation is the current `bake_subblock` body); assemble into `DeltaShVolumesSection` and run
the existing `delta_drop_policy` step exactly as today (the drop consumes the fully-assembled pre-drop
section and must stay downstream of the cache — `ScriptMutableDescriptorSlots` is not a bake input and must
never enter a cache key). Thread `stage_cache.as_ref()` into the `DeltaShBake` call site in `pipeline.rs`
(the block building `DeltaBakeInputs`, guarded by non-empty animated lights). Keep the `BakeControl`
accounting correct: on a full set of entry cache hits the worker still reports the same `publish_total`/
`advance` totals it does on a miss (mirror `direct_sh_bake::bake_direct_sh_volume_cached_controlled`'s
hit-path re-emit). Define a new `stage_id`/`stage_version` for this bake (none exist in the module today;
`DIRECT_SH_STAGE_ID` is taken by the base direct bake). This is the thin slice: it falsifies the
per-entry-cache → reassemble → drop boundary before the other two bakes fan out. AC: byte-identity (indirect
section), single-light localization, `--no-cache` path, version bump, corruption.

### Task 2: Animated-direct delta bake through the helper

Route `animated_direct_sh_bake::bake_animated_direct_sh_delta_volumes_controlled` through the Task 1 helper.
Add an `Option<&StageCache>` parameter; its per-entry bake closure is the current `bake_direct_subblock` body
(single unit-radiance light, `sh_bake::bake_probe_direct_rgb` seeded off the light's global source index).
Reassemble into `AnimatedDirectShDeltaVolumesSection` and run the existing `delta_drop_policy` animated-direct
drop unchanged. Use `try_to_bytes()` (not `to_bytes()`, which panics on an invalid wire payload) when writing
cache entries for this section type; on a hit decode with `from_bytes`. Thread `stage_cache.as_ref()` into
the `AnimatedDirectShBake` call site in `pipeline.rs` (guarded by non-empty animated lights). Define a
distinct `stage_id`/`stage_version` for this bake. Preserve the `BakeControl` hit-path accounting as in
Task 1. This task only adds a call site plus a small amount of section-assembly glue in
`animated_direct_sh_bake.rs`; it does not modify the shared helper. AC: byte-identity (animated-direct
section), single-light localization, `--no-cache` path, version bump, corruption.

### Task 3: Direct (entity-shadow) delta bake through the helper

Route `direct_sh_bake::bake_direct_sh_delta_volumes_controlled` through the Task 1 helper. Add an
`Option<&StageCache>` parameter; its per-entry closure is the current `bake_direct_delta_subblock` body
(single selected static light, seeded off the light's global static index). This bake returns
`Option<(DirectShDeltaVolumesSection, DirectDeltaBakeStats)>`: cache only the section bytes (the helper's
grain), reassemble the section from cached/baked sub-blocks, run the existing `delta_drop_policy` direct drop
(which retains each selection's highest canonical cell record even when zero), and **recompute**
`DirectDeltaBakeStats` from the reassembled post-drop section — its rows are per-selection CSR entry counts
and byte totals derivable from the section, so it need not be cached. The selection set comes from the
`EntityShadowLightsSection` and `AlphaLightsNs` the function already takes; fold the selected lights (postcard
+ global static index) into the per-entry keys exactly as the other two bakes fold their lights. Thread
`stage_cache.as_ref()` into the `DirectShDeltaBake` call site in `pipeline.rs` (inside the
`raw_entity_shadow_lights_section.as_ref().and_then(...)` block). Define a distinct `stage_id`/`stage_version`
— it must differ from `DIRECT_SH_STAGE_ID` (the base direct bake in the same module). Preserve `BakeControl`
hit-path accounting. AC: byte-identity (direct-delta section, including recomputed stats consistency),
single-light localization, `--no-cache` path, version bump, corruption.

### Task 4: Whole-section CellVisibility cache

Wrap `cell_visibility_bake::cell_visibility_bake(tree, portals, control)` in a whole-section `StageCache`
memo. The section depends only on BSP leaf topology and the portal graph — **no lights** — so its cache key
folds `{ tree leaf count, the portal front/back leaf adjacency pairs (the only portal data the components +
coupling passes read), and the fixed `CELL_VISIBILITY_DISTANCE_CAP` / `CELL_VISIBILITY_FANOUT_K` constants }`.
Mind the payload API: `to_bytes()` is **fallible** (`-> Result`) and `from_bytes(data, expected_cell_count)`
takes an extra `expected_cell_count: u32` — pass `tree.leaves.len() as u32` on the hit-decode path. On a hit,
return the decoded section's `to_bytes()` bytes as today (the call site currently does
`cell_visibility_bake(...)?.to_bytes()?`); a decode/validation failure is a miss (rebake), matching the
`StageCache` corruption contract. Thread `stage_cache.as_ref()` into the `CellVisibility` call site in
`pipeline.rs`, and preserve the existing `BakeControl` progress accounting on both hit and miss (the bake
publishes `cell_count * 2` units). Define a new `stage_id`/`stage_version`. Because lights are absent from the
key, this stage hits on every light-only edit — the common iteration case. AC: byte-identity, light-only-edit
hit, `--no-cache` path, version bump, corruption.

### Task 5: Whole-section ChunkLightList cache

Wrap `chunk_light_list_bake::bake_chunk_light_list(inputs, cell_size, cap)` in a whole-section `StageCache`
memo. It returns `Result<ChunkLightListSection, ChunkLightListError>` and takes **no `BakeControl`** (serial
bake), so its wrapper has no worker to re-`publish_total` against. The cache key folds
`{ geometry content hash (`sh_group::geometry_content_hash`), the portal front/back leaf adjacency pairs, the
static (`!is_dynamic`) light set from `AlphaLightsNs` (postcard + each light's global index), and the
`cell_size_meters` / `per_chunk_cap` parameters }`. Because only static slots feed the bake, a
dynamic-light-only edit reuses the cached section. Preserve the existing error path: a genuine
`ChunkLightListError::PayloadTooLarge` from a miss-path bake still surfaces as a build error; a cache
decode/validation failure is a soft miss (rebake). Thread `stage_cache.as_ref()` into the `ChunkLightList`
call site in `pipeline.rs`. Define a new `stage_id`/`stage_version`. AC: byte-identity, dynamic-light-only
hit, `--no-cache` path, version bump, corruption.

### Task 6: Cross-bake verification suite

Add the tests that verify the invariants the five caching tasks share, so no single task owns the whole
determinism story. Cover, for each of the five sections: warm-vs-`--no-cache` byte-identity (bake through a
real temp-dir `StageCache`, then with `None`, compare serialized bytes — including post-drop for the three
delta sections); the stage-version-bump miss-then-hit contract; and corruption-is-a-miss. For the three delta
bakes, add the single-animated-light-edit localization check: bake, edit one light, rebake through the same
cache, and assert only that light's CSR entries missed while the rest hit (per-entry hit/miss counts). For
`CellVisibility`, assert a light-only edit hits; for `ChunkLightList`, assert a dynamic-light-only edit hits.
These consume all five stages' cache paths, so this task runs last.

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
  per-cell validity accessor, `Option<&StageCache>`, and a `FnSync(cell, &MapLight, global_index) ->
  SubBlockBytes` closure. Key per entry: `CacheKey::new(stage_id, stage_version, blake3(geom_hash ||
  affinity_dims || cell_coord || probe_spacing || cell_validity_bytes || u32(global_index) ||
  len-prefixed postcard(light)))`. Mirror `direct_sh_bake::direct_cache_key`'s fold discipline
  (`to_le_bytes`, length-prefixed postcard, no HashMap/paths/timestamps).
- Delta call sites in `pipeline.rs`: `DeltaShBake` (~line 835), `AnimatedDirectShBake` (~line 949),
  `DirectShDeltaBake` (~line 1012). Graph call sites: `CellVisibility` (~line 435), `ChunkLightList`
  (~line 1215). `stage_cache: Option<StageCache>` is a pipeline param; pass `stage_cache.as_ref()`.
- Reuse `sh_group::geometry_content_hash` for the geometry fingerprint in all new keys (lockstep with the
  existing SH/lightmap invalidation on any geometry edit).
- Hit-path `BakeControl` accounting mirrors `direct_sh_bake::bake_direct_sh_volume_cached_controlled`
  (re-emit `publish_total` + `advance` when the worker is skipped). `ChunkLightList` has no `BakeControl`.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Warm (cached) output byte-identical to `--no-cache`, all five sections, including post-drop | Tasks 1–5 | Any new bake input not folded into a key → stale warm output; drop pass must stay downstream of the cache | AC "byte-identical"; Task 6 |
| Per-entry delta cache is exact (single-light entries, global-index / position seed) | Existing bake code; preserved by Tasks 1–3 | A refactor that changes the soft-visibility seed axis, or sums >1 light per sub-block, breaks byte-identity | AC "byte-identical"; Task 6 |
| `ScriptMutableDescriptorSlots` never enters a cache key; drop runs each build over reassembled sub-blocks | Tasks 1–3 | Folding mutable-slot state into a delta key would desync warm/cold and miss script-curve changes | Task 6 delta byte-identity |
| Single-animated-light edit re-bakes only that light's CSR entries | Task 1 helper; Tasks 2–3 | A key that folds the whole light set (not the per-entry light) would coarsen invalidation | AC "single light"; Task 6 |
| `CellVisibility` key excludes lights; `ChunkLightList` key covers only static lights | Task 4, Task 5 | Folding lights into the cell-vis key, or dynamic lights into the chunk key, would miss on benign edits | AC "light-only / dynamic-only hit" |
| `--no-cache`/`--release` neither read nor write the five entries; exact uncached path | Tasks 1–5 | A wrapper that consults the cache before checking `Option<&StageCache>` is `None` | AC "--no-cache"; Task 6 |
| Corrupt entry is a miss, not a crash | Tasks 1–5 | Fallible `to_bytes`/`from_bytes` (CellVisibility) and panic-on-invalid `to_bytes` (animated-direct) mishandled | AC "corruption"; Task 6 |

## Open questions

- **Shared helper vs three near-duplicates.** The spec assumes one `delta_sh_cache` helper parameterized by a
  per-entry closure. If the three sub-block bake bodies prove too divergent to share cleanly (different
  section assembly, the direct-delta stats path), Task 1 may instead establish a thin common key-builder +
  get/put pair that each bake calls inline, still keeping the key-fold contract in one place. Either shape
  satisfies the ACs; the implementer picks based on how cleanly the closures factor. Not a scope change.
