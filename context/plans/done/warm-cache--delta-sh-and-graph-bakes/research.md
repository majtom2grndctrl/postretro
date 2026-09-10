# Research — warm-cache delta-SH and graph bakes

Investigation notes behind the spec. Decisions live in `index.md`; this is the grounding.

## Symptom

Building `content/dev/maps/campaign-test.map` twice, the second (warm-cache) build was only ~20 s
faster on an Intel i9. The warm build should skip nearly all bake work; it does not.

`campaign-test.map` light census (grep of the `.map`): 11 `light` (static), 2 `light_spot`,
7 `light_dynamic`, 11 `light_dynamic_spot` — **18 animated/dynamic lights**, 0 carried-light links.
The animated-light count is what drives the uncached delta bakes below.

## Cache infrastructure (unchanged, correct)

`cache.rs::StageCache` — disk cache under `.build-caches/prl-cache/`, keyed
`CacheKey::new(stage_id, stage_version, input_hash)` → `blake3(stage_id || stage_version_le || input_hash)`.
On by default; `main.rs` builds `Some(StageCache)` unless `args.release || args.no_cache`, then threads
`stage_cache: Option<StageCache>` through `pipeline.rs`. LRU-pruned once at build start.

## Pipeline stage cache audit (23 stages, `pipeline.rs::run_after_parsing`)

Confirmed **CACHED-and-stable** (warm no-edit rebuild hits, keys deterministic — postcard/`to_le_bytes`,
no HashMap-iteration/paths/timestamps in any key):

- Lightmap — two-level: `"lightmap_section"` memo + per-light `"lightmap_layer"` (`lightmap_layer.rs::layer_input_hash`/`section_input_hash`).
- Base indirect SH — per-4³-group `"sh_group"` (`sh_group.rs::group_cache_key`), warm-approx (bounded light set).
- Base direct SH — whole-section `"direct_sh_volume"` (`direct_sh_bake.rs::direct_cache_key`), **exact**.
- NavMesh `"navmesh"`, ShadowmaskAtlas `"shadowmask_atlas"`, SDF `"sdf_atlas"`, AnimatedWeightMaps `"animated_lm_weight_maps"`.
- TextureMips — its own `.prm` sidecar bundle cache under `baked/materials/` (not `StageCache`).

Confirmed **UNCACHED, recomputed every warm build** (the problem):

| Stage (`StageId`) | Fn | Cost driver | Runs when |
|---|---|---|---|
| `DeltaShBake` | `delta_sh_bake::bake_delta_sh_volumes_controlled` | per-(cell×animated-light) 64-probe indirect ray bake | ≥1 animated light |
| `AnimatedDirectShBake` | `animated_direct_sh_bake::bake_animated_direct_sh_delta_volumes_controlled` | per-(cell×animated-light) direct ray bake | ≥1 animated light |
| `DirectShDeltaBake` | `direct_sh_bake::bake_direct_sh_delta_volumes_controlled` | per-(cell×selected-static-light) direct ray bake | entity-shadow selection non-empty |
| `CellVisibility` | `cell_visibility_bake::cell_visibility_bake` | portal-graph components + parallel top-K coupling over all cells | always |
| `ChunkLightList` | `chunk_light_list_bake::bake_chunk_light_list` | per-static-light portal-flood BFS + per-chunk BVH shadow rays | always |

Uncached-by-design and inherently so (fast, or mandatory inputs to every downstream key):
Parse, BSP `Partitioning`, `Visibility`/portals, `Geometry`, `BvhBuild`. These cannot be skipped — every
downstream cache key is `blake3(postcard(GeometryResult))`-derived, so geometry must be rebuilt to look up
any entry. They are a floor on warm-build time but not the dominant cost.

## Ruled out: cache-key instability

Both the lightmap and SH keys were audited for run-to-run instability (the "always-miss" failure). None
found — every key input is deterministic. The recent `carrier: String` field added to `MapLight`
(`21e8fa2`, "carried dynamic-light vertical slice") folds into the postcard-based keys and is force-cleared
for baked lights in `parse::resolve_carried_light_links`, so it caused a **one-time** invalidation at the
code change, not a persistent warm miss. The `60bb7c3` SH range-cull runs on the miss-only bake path and is
byte-identical. So the ~20 s savings *is* the lightmap + base-SH cache working; everything in the table
above is what still runs.

## Divergence from prior commitments

`build_pipeline.md` states (lines ~173, ~277): "The SH indirect-bounce delta path is cache-less but
low-frequency" and "Delta bakes are invoked directly from the compiler rather than through the build cache."
That "low-frequency" premise is false for animated-light-heavy maps (campaign-test: 18). `CellVisibility`
and `ChunkLightList` postdate the "Participating stages" list (line ~320: "Parse, BSP, portals, geometry,
and BVH run uncached — they are fast enough") and are not in the "fast enough" set. The spec updates these
statements at promotion.

## Delta-bake structure → cache grain

All three delta bakes share one shape (`affinity_grid.rs`):

1. `decompose_affinity_for_lights(AffinityReachInputs, lights)` → `per_light_cells: Vec<Vec<u32>>`, `affinity_dims`.
   Each light's cell set = **AABB-clip (falloff sphere + 0.5 m pad) ∩ portal-reachability flood** — already
   bounded per affinity cell. `AFFINITY_FACTOR = 4` (locked; = compose `@workgroup_size(4,4,4)`); an affinity
   cell = 4³ = 64 base probes.
2. `build_csr(per_light_cells, cell_count)` → `(affinity_offsets, affinity_lights)`, cell-keyed CSR;
   `csr_entry_cells(affinity_offsets)` → per-entry cell index, index-parallel to `affinity_lights`.
3. `affinity_lights.par_iter().zip(csr_entry_cells)` → **one dense 64-probe sub-block per CSR entry**, each
   baking a **single light in isolation** (single-element light slice), flat-mapped into the section payload.

**Exactness of per-entry caching.** Because each CSR entry is single-light, bounding the light set never
changes a *kept* cell's radiance — it only changes *which* cells a light occupies (culled cells are provably
zero: falloff-AABB exact, portal-flood by the shared disjoint-reach assumption). Determinism holds across
processes: direct-delta and animated-direct seed soft-visibility off the light's **global** static/source
index (`sh_bake::bake_probe_direct_rgb`, `soft_visibility_seed`); indirect-delta seeds off a stable
position-derived probe index `cell * PROBES_PER_CELL + local` (never a subset-relative slot). So a per-entry
cache is **byte-identical** to the uncached bake — the delta channel needs no warm/cold approximation, unlike
the base indirect SH volume.

**Drop policy is downstream.** `delta_drop_policy.rs` omits only CSR records whose **decoded RGB is exactly
zero** (`rgb_payload_is_zero`) — not an error budget — and additionally retains script-mutable slots
(`ScriptMutableDescriptorSlots`, from the light-membership manifest, *not* a bake input) and, for direct
delta, each selection's highest canonical cell. It consumes a **fully-assembled pre-drop section** and
rebuilds it. So per-entry caching sits at the pre-drop grain: reassemble cached/baked sub-blocks into the
pre-drop section, then run the existing drop pass unchanged each build. `ScriptMutableDescriptorSlots` never
enters a cache key.

## Payload / signature gotchas (per bake)

- `DeltaShVolumesSection`: `to_bytes()->Vec<u8>` / `from_bytes()->Result`. `DeltaBakeInputs` is a flat context
  (bvh, primitives, geometry, tree, exterior_leaves, portals, animated_lights) — not `ShBakeCtx`.
- `AnimatedDirectShDeltaVolumesSection`: `to_bytes()` panics on invalid; prefer `try_to_bytes()->Result` for
  puts. `AnimatedDirectShBakeInputs { sh_ctx, portals, animated_lights }`.
- `direct_sh_bake::bake_direct_sh_delta_volumes_controlled(inputs, config, alpha_lights, entity_shadow_lights, control)`
  returns `Option<(DirectShDeltaVolumesSection, DirectDeltaBakeStats)>`. Stats are **not** in the wire payload
  but are recomputable from the section (per-selection CSR entry counts). `DIRECT_SH_STAGE_ID`/`_VERSION` are
  taken by the whole-section base bake — the delta bake needs a **distinct** stage id.
- `chunk_light_list_bake::bake_chunk_light_list(inputs, cell_size, cap) -> Result<Section, ChunkLightListError>`.
  **No `BakeControl`** (serial). `ChunkLightListInputs { bvh, primitives, geometry, lights: AlphaLightsNs,
  tree, portals, exterior_leaves }`. Uses only `!is_dynamic` (static) slots — dynamic-light edits do not
  affect it. `to_bytes()->Vec<u8>` / `from_bytes()->Result`.
- `cell_visibility_bake::cell_visibility_bake(tree, portals, control) -> anyhow::Result<CellVisibilitySection>`.
  Depends on **tree leaf topology + portal graph only — no lights**. Odd payload API: `to_bytes()->Result`
  (fallible) and `from_bytes(data, expected_cell_count: u32)->Result` (extra arg — pass `tree.leaves.len()`).

## Reuse

`sh_group::geometry_content_hash(geometry) -> [u8;32]` (`pub(crate)`, postcard+blake3) is the whole-map
geometry fingerprint every existing SH key folds; the new keys reuse it for lockstep invalidation. The
per-entry cache-hit control accounting mirrors `direct_sh_bake::bake_direct_sh_volume_cached_controlled`:
the hit path re-emits `publish_total` + `advance` because the worker is skipped.

## Measurement note for the user

Every stage already prints its own line in the Build Summary (`reporter.rs::finalize` → `{label} {secs}s`)
and each cache decision logs `[cache] <stage> hit|miss`. A warm build of campaign-test will show the five
stages above dominating wall time with no hit lines — the direct empirical confirmation of this diagnosis.
