# Animated Lightmap Array Atlas

## Goal

Promote the two animated-lightmap compose targets from `texture_2d` to `texture_2d_array` so a
script-animated baked light lights every face it reaches, not only faces whose chart landed on
atlas layer 0. Allocate and compose one array slot per static layer that actually holds animated
receivers, so cost tracks animated coverage rather than static atlas depth. Replace the silent
layer-0 skip with a loud diagnostic.

`lightmap-array-atlas` promoted the static atlases and named this follow-up as out of scope:
"sections 24/25 stay single-layer; animated overflow is a separate plan." This is that plan.

## Background

`StaticBakedLights` and `AnimatedBakedLights` are complementary on `animation.is_some()`, so the
static lightmap bake never sees an animated light. Its direct term exists solely in the animated
weight maps, which cover layer 0. A face on layer ≥ 1 loses the entire animated **direct** term
with no fallback, keeping only animated indirect (delta-SH, world-space) and the ambient floor.

Two failures, not one. **Missing light:** off-layer-0 chunks get a degenerate 1×1 zero-count rect
and no diagnostic at any level, folded into the stage's `info` line so a spilled map logs healthy
stats. **Possible corruption:** that rect keeps the spilled chart's per-layer coordinates, the
section carries no layer, and only zero-*area* rects are skipped during tile expansion — so it gets
a workgroup and stores black at those coordinates on layer 0. The overlap assert buckets per face
and cannot see the resulting cross-face collision. Structurally possible, unverified.

Reaching layer ≥ 1 does not take a large map — `switch-demo`, one sealed room, measures
`512×512×2`. Coarsening `_lightmap_density` is not a workaround: the per-layer dimension is sized
to fit the largest single BSP leaf alone, so it shrinks along with the texel count. Anchors,
measurements, and corrections to earlier readings are in `research.md`.

## Scope

### In scope

- `AnimatedLightWeightMaps` (section 25) v3: per-chunk atlas layer, an animated-slot count, and
  the slot→static-layer table. Graceful v2 decode.
- Compiler bakes weight maps for every layer. Dense slot assignment over the static layers that
  hold animated receivers. Remove the layer-0 gate and its degenerate-rect path.
- A bake-time cap on animated slots with a named error, mirroring `LightmapBakeError::LayerOverflow`.
- Both compose targets become `texture_2d_array` sized to the animated slot count: texture
  creation, storage views, compose BGL entries, and `textureStore` array index.
- Forward pass samples both animated atlases by layer through a static-layer→slot lookup,
  replacing the `in.lightmap_layer == 0u` guard.
- The 1×1 dummy path gains array-compatible views so the no-animated-lights and empty-map paths
  keep a valid group-4 bind group.
- Load-time graceful degradation when the animated atlas would exceed a VRAM budget, mirroring
  `filter_usable_section`'s log-and-drop posture.
- Layer-aware cross-section validation and a layer-aware overlap assert.
- Loud diagnostics: aggregate counts of animated layers and slots at bake, a hard bake error when
  the slot cap is exceeded, and a load-time `[Renderer]` error when the VRAM budget drops all
  animated contribution.

### Out of scope

- Static atlas packing changes. Layer assignment, leaf cohesion, and `choose_layer_dim` are
  untouched; this plan consumes `ChartPlacement.layer` as given.
- Backfilling small leaves into earlier layers. The packer's layer counter stays monotonic.
- The `AnimatedLightChunks` section (24). Its records need no layer — the layer lives on the rect.
- A 2D-dispatch fallback for the 65535 workgroup ceiling. The guard stays a hard error; this plan
  only ensures it counts the newly-included tiles.
- Animated **indirect** lighting. It rides world-space delta-SH — the animated-light SH
  compose (section 27, `DeltaShVolumes`) — not the weight-map atlas, so it is already
  layer-independent. The direct SH-delta volumes (`DirectShDeltaVolumes` 41,
  `AnimatedDirectShDeltaVolumes` 45) are likewise world-space and untouched. The delta-SH probe
  coarsening that landed since this draft was written operates only on 27/41/45 and never enters
  section 25.
- Retiring the duplicated stride constants in the compiler's byte-size log. Updated in lockstep
  here; unifying them is separate.

## Acceptance criteria

- [ ] `cargo test -p postretro-level-format` passes, including: a v3 round-trip covering
      single-slot and multi-slot sections; a test asserting a v2 payload with chunks decodes with
      every chunk on layer 0 and one animated slot, and a v2 payload with no chunks decodes to an
      empty slot table and slot count 0; a test asserting an unsupported version is still rejected
      by a message naming the version; a test asserting `is_consistent` rejects a rect whose layer
      is absent from the slot table and rejects an unsorted or duplicated slot table.
- [ ] `cargo test -p postretro-level-compiler --bin prl-build` passes, including a test that bakes
      weight maps from placements spanning two layers and asserts both layers produce covered
      texels with distinct slots. No test may construct this by mutating a layer field on an
      otherwise layer-0 fixture — the placements must come from a real multi-layer pack.
- [ ] A test asserts no two chunk rects overlap within one `(slot, layer)`, across faces — not only
      within a face.
- [ ] Compiling a map whose animated receivers span two atlas layers logs, at `info`, the static
      atlas layer count and the animated slot count. Exceeding the animated slot cap fails the bake with an error
      naming the cap and the count found — testable as a unit test that drives the slot-assignment
      step past a low injected cap, not via a production-sized fixture.
- [ ] The `placement.layer != 0` skip branch that emitted a 1×1 zero-count sentinel rect is gone
      (a review/grep gate). A test on a real multi-layer pack asserts every chunk with covered texels
      — including layer-≥1 chunks — produces a real covered rect, not a 1×1 zero-count placeholder.
      Legitimate 1×1 rects from zero-extent charts are unaffected; the metric is the zero-count skip
      sentinel, not the 1×1 shape.
- [ ] `cargo test -p postretro-renderer` passes, including a test asserting the group-4 BGL entries
      for the animated atlas and animated direction bindings declare `D2Array`, and that the group-4
      entry count is 8 (up from 7, for the new slot-table uniform at binding 7). This must assert
      `view_dimension` explicitly — the existing entry test pins `sample_type` only, so a partial
      promotion currently passes.
- [ ] A test asserts the compose shader parses under `naga` and stores through an array-indexed
      write on both compose targets.
- [ ] A test asserts the two compose BGL storage entries declare `D2Array` `view_dimension`, not
      only that the WGSL parses — so a Rust-side storage view or BGL entry left at `D2` while the
      shader is arrayed is caught in a CPU test rather than at device pipeline creation.
- [ ] A test asserts the forward shader samples both animated atlases with a layer argument and
      contains no `lightmap_layer == 0u` animated guard.
- [ ] A static layer with no animated receivers resolves to zero animated contribution, not to
      slot 0's contents. Verified by a test on the pure layer→slot resolution, independent of a
      GPU: a lookup table built from a slot table that omits a layer returns the no-contribution
      sentinel for it.
- [ ] A test asserts that for every occupied static layer, the slot resolved during compose tile
      expansion equals the slot the forward lookup returns — both derived from the one section-25
      slot table through a single shared inversion.
- [ ] A test on the pure `animated_atlas_extent(width, height, slot_count)` seam asserts it returns
      the resolver's width and height (`usable_atlas_dimensions`) and a depth equal to the section-25
      slot count — never the static layer count — so a future change can neither desynchronise
      animated width/height from the static atlas the compose pass shares coordinates with, nor size
      animated depth from the static layer count.
- [ ] A unit-testable helper decides whether an animated atlas of a given width, height, and slot
      count fits the VRAM budget; a test exercises an over-budget case and asserts the byte estimate
      scales with the slot count (the old `width × height × 12` info log gains the slot factor).
      Over budget logs a `[Renderer]` error and the level renders with no animated contribution
      rather than failing.
- [ ] A test asserts the pure slot-count guard maps slot count 0 to the dummy-atlas decision (never
      an `Extent3d` of depth 0); that the guard precedes allocation on the normal, VRAM-fallback, and
      failure paths is a review gate, not a single unit test.
- [ ] A test asserts cross-section validation rejects a chunk rect whose layer is absent from the
      slot table, and one whose coordinates fall outside the atlas bounds.
- [ ] Loading a level whose animated atlas construction fails binds the 1×1 dummy animated views
      (no animated contribution) and leaves no previous level's atlas views or dispatch state bound
      with the new geometry; a test asserts the failure arm rebinds to a dummy
      `AnimatedLightmapResources` (nulling `dispatch_state`) rather than logging and falling through.
      Both install entry points (`renderer_full_init`, `renderer_resources`) apply this one policy.
      (Today the `renderer_resources` failure arm logs and falls through without reassigning either
      resource field while the new geometry swaps in unconditionally; `renderer_full_init` treats the
      same error as fatal.)
- [ ] `content/dev/maps/animated-layer-spill.map` has baked animated receivers on atlas layer ≥ 1,
      confirmed by reading `layer_count` (= 2) and — post-Task-1 — the v3 slot table out of its
      compiled PRL, not asserted from map authoring alone. Running it shows the pulsing animated
      light on the layer-≥1 rooms after the promotion. **[manual GPU]** — the end-to-end check; on
      current `main` those rooms are dark (12 degenerate 1×1 rects), the pre-fix baseline this
      criterion flips.
- [ ] `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler -- --ignored`
      shows no regression against the pre-change baseline. The golden PRL is regenerated, and the
      regeneration is justified by a byte-delta diff confirming the only changes are section 25's
      payload (every rect grows 4 bytes for `layer`; the header gains the slot count; the slot table
      is appended) and the downstream shift of every later section's TOC offset — no other section's
      content changes.

## Tasks

### Task 1: Section 25 v3 wire format

Add `layer: u32` to `ChunkAtlasRect` as an appended field, taking `CHUNK_RECT_SIZE` from 20 to 24.
Add an animated-slot count (growing the fixed header stride 16 → 20) and a slot→static-layer table
to the section, so the runtime can size the atlas and build the inverse lookup without scanning
every rect. Bump the version constant to
3. The encoder always writes v3. The decoder resolves version to a capability flag once, up front,
then branches where the appended data would be read — the `TriggerVolumes` v1→v2 decoder is the
pattern to mirror, with one difference: this decoder currently rejects every non-matching version,
so the graceful branch is new. A v2 payload decodes as every chunk on layer 0 with exactly one
animated slot mapping to static layer 0, which is what v2 meant — except a v2 section with no chunks,
which decodes to an empty slot table and slot count 0, not to `[0]`/1. This section computes its total
size up front from fixed strides rather than walking a self-describing stream, so the stride used
in that computation becomes version-dependent; the appended slot table is variable-length and must
be counted in it. Extend `is_consistent` to check that every rect's layer appears in the slot table
and that the table is sorted and duplicate-free. Fix the layout doc block, which still says
`version (= 1)` two bumps later. Appending `layer` breaks every `ChunkAtlasRect` struct literal in
downstream crates (the compiler bake and the renderer/render-cpu fixtures); those compile fixes
belong to their Phase-2/3 owners, not to this task.

### Task 2: Compiler — bake every layer, assign slots, diagnose

Remove the `placement.layer != 0` gate and the degenerate-rect return. Collect the distinct static
layers holding animated receivers, sort that distinct-layer list (not the rects or placements — leaf
cohesion must be preserved), and assign dense slots; emit the slot table and set each rect's layer. Thread the atlas layer count out of the lightmap bake into the weight-map stage
— `pipeline.rs` currently destructures `layer_count: _` with a comment saying animated weight maps
are single-layer, which is the line this plan invalidates. Add the animated-slot cap as a hard bake
error — a named error mirroring `LightmapBakeError::LayerOverflow` that aborts the bake, never
dropping a slot or emitting a placeholder rect (the cap is a deterministic authoring guard; silently
dropping a slot would reintroduce the silent-darkness failure this plan exists to remove). Adding the
error makes the weight-map stage fallible — `bake_animated_light_weight_maps_controlled` returns a
`Result` — which ripples through its infallible wrapper and the `pipeline.rs` cache hit/miss arms;
propagate the error there. Rework the stage's `info` line so spilled chunks are no longer folded
invisibly into healthy-looking stats: report the static atlas layer count (the threaded `layer_count`
— its consumer) and the animated slot count, which differ when animated receivers occupy only some of
the static layers. Widen the per-face overlap
assert to bucket by layer and to catch cross-face collisions within a layer, since a per-face-only
assert is what let the borrowed-coordinate collision hide. Bump the stage's cache-key version. The
byte-size log carries its own copy of the encoder's stride constants; update it in lockstep.

### Task 3: GPU — array atlases, slot-indexed compose and sampling

Create both compose targets with `depth_or_array_layers` set to the section's animated slot count,
and pin `D2Array` on their storage and forward views — the static irradiance/direction/shadowmask
views already do this explicitly and are the pattern to copy. The array atlas is allocated only when
the slot count is ≥ 1; a slot count of 0 (empty section / no animated receivers) takes the 1×1 dummy
path via an explicit slot-count guard, never a `depth_or_array_layers = 0` allocation (a wgpu
validation error). Derive both atlases' extent through a pure `animated_atlas_extent(width, height,
slot_count)` seam so the depth-equals-slot-count rule and the slot-count-0 guard are unit-testable
off-device. Flip the two compose BGL storage entries and the two group-4 sampled entries to
`D2Array`. Carry the target slot on `DispatchTile`,
which has a spare `_pad` word, resolving rect layer → slot during tile expansion so the shader does
no lookup; all three `textureStore` sites (debug heatmap, irradiance, direction) take an array
index. Give the forward pass a static-layer→slot lookup so it can sample by `in.lightmap_layer`,
replacing the layer-0 guard; layers with no animated receivers must resolve to the no-contribution
sentinel `INVALID_SLOT = 0xFFFF_FFFF`, not slot 0's contents. Realize the lookup as a new group-4
uniform at binding 7 — the forward/lightmap BGL (`lightmap.rs`, `LightmapResources`) grows from 7 to
8 entries, and the group-4 entry-count assertion moves 7 → 8 with it — built by inverting the
section-25 slot table in `LightmapResources::new`, which today receives the animated forward views
and must additionally receive the decoded slot table (thread it through both call sites,
`renderer_resources.rs` and `renderer_full_init.rs`; the geometry-`None` init path passes an empty
table). Both this table and the compose-side tile-expansion resolution must invert the *same* slot
table through one shared helper, so a given static layer maps to the identical slot on both sides.
Size the table for the existing 256-layer static ceiling and pack it against WGSL uniform stride: a
bare `array<u32, 256>` in uniform space strides each element to 16 bytes, so pack four static layers
per `vec4<u32>` (64 `vec4`s).
The single 1×1 `Rgba16Float` dummy texture backs both fallback views and needs array-compatible
views, or the no-animated-lights path breaks on layout incompatibility. The VRAM `info` log
computes `width × height × 12` with no layer factor; fix it. Keep the compose shader parsing under
`naga` — an existing test pins that, and another pins a compose binding by literal string.

### Task 4: Runtime validation and graceful degradation

The animated atlas already takes its width and height from `usable_atlas_dimensions` at both call
sites (`renderer_full_init.rs`, `renderer_resources.rs`), which is what keeps it synced to the static
atlas; ensure both sites feed those resolved dimensions to the animated constructor. The animated
depth is the section-25 slot count, not the resolver's static layer count, so the resolver needs no
new return value. Add a pure, unit-testable helper deciding whether a given width, height, and slot
count fits the VRAM budget — a named constant `ANIMATED_ATLAS_VRAM_BUDGET_BYTES`, provisional and
owner-tuned (see Open questions), so the task builds against a concrete number — and a load-time path
that logs a `[Renderer]` error and falls back to no animated contribution when it does not — matching
how an oversize static section degrades rather than aborting. Extend cross-section validation (`validate_cross_section`, `render-cpu`), which today
checks prefix sums and light-index bounds but has no layer notion, to reject rects whose layer is
absent from the slot table and whose coordinates fall outside the atlas; thread the decoded slot
table and the static-atlas width/height into it; the caller `AnimatedLightmapResources::new` already
holds these as its `atlas_dimensions` argument and supplies them (its `render-cpu` test `mk_rect`
literals update alongside). This repeats the layer-in-slot-table check `is_consistent` runs at decode
time — deliberate defense-in-depth across the decode-time and load-time boundaries, not a
duplication to collapse. Fix the install path so a failed animated-lightmap construction never
leaves stale views or dispatch state bound. Today the `Err` arm in `renderer_resources.rs` only logs
and falls through (no `return`): it reassigns neither `full.animated_lightmap` nor
`full.lightmap_resources`, while the new level's geometry (`bvh_leaves`, `cell_draw_index`,
`compute_cull`) swaps in unconditionally just after the match, so the new level renders lit by the
previous level's atlas and culled against stale `dispatch_state`, every frame. On any
animated-lightmap failure at load (extended `validate_cross_section` rejection, the VRAM budget, or
an existing construction error), fall back the way the pre-existing `weight_maps: None` early-out in
`AnimatedLightmapResources::new` already does: construct a dummy `AnimatedLightmapResources` via that
non-failing path (yielding dummy `forward_view`/`direction_forward_view` and `dispatch_state: None`,
so `is_active()` is false and the compose dispatch is skipped), assign it to `full.animated_lightmap`,
then build `LightmapResources` from its views — the same three-step order the `Ok` arm uses.
Reconcile the two entry points: `renderer_full_init.rs` currently treats the same error as fatal via
`map_err(...)?`; replace that with the same dummy construction so both apply one dummy-fallback
policy.

### Task 5: Multi-layer verification fixture

The fixture `content/dev/maps/animated-layer-spill.map` provides the end-to-end check: four disjoint
sealed rooms cloned from `test_animated_weight_maps_mixed`'s room, each carrying a `_bake_only`
`style 11` pulse light. At default density it bakes to `layer_count = 2` (per-layer 512²) and already
reproduces the bug on current `main` — section 25 carries 24 chunk rects, 12 of them the degenerate
1×1 skip sentinels for the two rooms that pack onto layer 1, so those rooms' pulse light is silently
dropped. Verified deterministic across bakes, ~4 s to bake, with the measured layer count recorded in
the map header. Confirm the spill by reading `layer_count` — and, once Task 1 lands, the v3 slot
table — out of the compiled PRL, never by inferring from the `.map`.

The spill comes from total leaf *area* exceeding one layer, not from density. `choose_layer_dim`
sizes the per-layer dimension to the densest single BSP leaf (leaf-cohesion keeps a leaf's charts on
one layer), and raising `_lightmap_density` grows that dimension in lockstep with the charts — so
density alone does not add layers. More layers come from more leaf area at a bounded per-leaf size:
either one geometrically complex room with several leaves (`switch-demo`'s single room already sits at
512×512×2 this way) or, as here, several disjoint simple boxes — each box is ~one leaf that fits well
inside 512², so four of them are needed to overflow the first layer.

Leave `switch-demo`'s indicator on the dynamic tier: it already reaches 512×512×2, but its indicator
is a dynamic-tier light that lands no baked-animated receiver on layer ≥ 1 — which is why a dedicated
fixture was needed — and dynamic is the better design for a small press indicator, so reverting it
would weaken that fixture to serve this one. Do not add `animated-layer-spill` to `GATE_FIXTURES`:
its determinism is useful but the gates bake every fixture twice; see Open questions.

## Sequencing

**Phase 1 (sequential):** Task 1 — every other task reads the v3 shape.
**Phase 2 (concurrent):** Task 2, Task 3 — disjoint crates, both consume Task 1's format.
**Phase 3 (concurrent):** Task 4, Task 5 — Task 4 consumes Task 3's atlas construction and Task 1's
slot table; Task 5 needs Tasks 2 and 3 to compile and render a multi-layer map.

## Wire format

Section 25, version 3. Little-endian throughout, mirroring the existing v2 layout: a fixed header
carrying the version and every array count, then flat arrays in header order. No per-array length
prefix and no per-entry counts. Appended fields go last within `ChunkAtlasRect` and the new slot
table goes after the existing three arrays, keeping the encoder append-only. Backward safety does
not rest on offset preservation — growing the fixed rect stride 20 → 24 shifts every rect after the
first — but on the version check in `AnimatedLightWeightMapsSection::from_bytes`, which rejects a
mismatched version at the header before reading any rect.

- `ChunkAtlasRect`: existing `atlas_x, atlas_y, width, height, texel_offset` then appended
  `layer: u32`. Stride 20 → 24. Coordinates stay per-layer, as the packer emits them.
- Header gains the slot count, growing the fixed header stride from 16 to 20. The slot table is
  `u32` static-layer indices, ascending, no duplicates; index into it *is* the slot.
- Empty section: header with zero counts and an empty slot table, matching v2's
  header-only encoding for the empty case.
- v2 decode: every rect layer 0, slot table `[0]`, slot count 1. A section with no chunks decodes
  to an empty slot table and slot count 0. This reproduces v2 runtime semantics exactly, including
  its off-layer-0 loss: an already-baked v2 map still carries 1×1 skip sentinels, which the runtime
  `expand_dispatch_tiles` (skips zero *area* only) still dispatches, so a v2 map must be rebaked to
  v3 to gain off-layer-0 animated light. Graceful decode buys forward compatibility, not a fix for
  old bakes.
- Unsupported versions stay a hard `InvalidData` error naming the version found.

The TOC blob's per-section `version` field is hardcoded to 1 for every optional section and is not
the payload version. Do not change it.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Every chunk with animated receivers gets a real rect and a slot — no rect is a skip sentinel | Task 2 (gate removal, slot assignment) | Exceeding the slot cap must fail the bake, never fall back to a placeholder rect | AC: cap fails the bake · AC: degenerate shape gone |
| Slot table is a sorted, duplicate-free bijection onto occupied static layers; a layer absent from it yields no animated contribution | Task 1 (`is_consistent`), Task 2 (assignment) | Task 3's forward lookup must map an absent layer to no contribution, not to slot 0's contents | AC: v3 round-trip · AC: absent layer resolves to sentinel |
| Animated atlas width and height equal the static atlas's; animated depth is the section-25 slot count, not the static layer count | Task 3, Task 4 (resolver) | Compose stores at absolute per-layer coordinates and forward samples all atlases with one normalized UV — enforced today by doc comment and one resolver test, no assert | AC: animated atlas created at resolver width/height, slot-count depth |
| The forward pass samples an animated texel only for a cell in this frame's `VisibleCells`, which the compose pass wrote from the same `VisibleCells` | Task 3 | A texel never in any visible set reads its once-only zero-init; a texel written then culled holds stale contents, never sampled because one `VisibleCells` gates both passes. A second consumer (reflection probe, alternate camera) must share this frame's `VisibleCells` or skip animated chunks. No per-frame clear | AC: absent layer resolves to sentinel · AC: multi-layer fixture renders |
| A face's animated direct term is zero only when no animated light reaches it | Task 2, Task 3 | Regressing to a layer-gated sample restores the silent-darkness bug | AC: multi-layer fixture renders |
| Leaf cohesion: all charts of one BSP leaf share a layer | pre-existing (`pack_layers`) | Untouched here; slot assignment must not reorder or regroup placements | AC: multi-layer bake from a real pack |

## Rough sketch

Slot assignment belongs in the weight-map stage, not the packer: the packer knows nothing about
which lights are animated, and layer assignment must stay identical so the static atlas is
byte-unchanged. Collect `placement.layer` for chunks that produced at least one covered texel,
dedupe, sort, and index.

Resolving layer → slot on the CPU during tile expansion keeps the compose shader free of a lookup
and costs nothing — the tile record already has a spare word. The forward pass genuinely needs the
lookup, since it samples per fragment from a per-vertex layer.

For the forward lookup, prefer a small uniform table sized to the static layer ceiling over a new
storage buffer: group 4 is a fragment-visible group and the vertex-stage storage budget is already
documented as tight elsewhere in the renderer. Mind WGSL uniform array stride — a `u32` array in
uniform space strides to 16 bytes, so pack it.

The compose tile count grows to include previously-skipped chunks. Tiles are already culled against
the visible-cell bitmask each frame, so per-frame cost still tracks visible animated coverage; the
65535 ceiling is checked against the pre-cull master list, which is the number that grows.

## Open questions

- **Animated slot cap value.** A layer costs 12 bytes per texel across both targets, so cost scales
  with atlas dimension as hard as with slot count: 8 slots is ~24 MiB at 512² but ~6 GiB at 8192².
  A layer cap alone cannot bound VRAM. The plan therefore carries both a bake-time slot cap and a
  load-time byte budget (`ANIMATED_ATLAS_VRAM_BUDGET_BYTES`), neither number yet settled. Both ship
  as tunable constants so the tasks build: pick the slot cap from measured layer counts on real
  content rather than from the static 256, and the byte budget from measured animated-atlas sizes.
  A conservative provisional start for the budget is 256 MiB (both animated targets together, 12
  bytes/texel). Both values are owner-held; the postures — hard bake error for the cap, load-time
  graceful drop for the budget — are settled.
- **The golden PRL is stale from engine evolution, not a regression — regenerate on `main` before
  Task 1.** `mixed_fixture_without_script_membership_matches_pre_feature_golden_prl` is one of two
  pre-existing `--ignored` failures on `main` (see
  `context/plans/done/switch-entity/out-of-scope-findings.md` §1). Characterized 2026-08-15 with a
  section-level byte diff of a cold `--no-cache` bake against the checked-in golden (bake confirmed
  deterministic across two runs). The delta is confined to lighting/nav evolution that landed since
  the golden was baked at `33e3a152`: a newly-emitted `AnimatedDirectShDeltaVolumes` section (45),
  the delta-SH probe coarsening shrinking `OctahedralShVolume` (34: 96 200 → 9 972 B) and shifting
  `DeltaShVolumes` (27: +72 B), plus deterministic changes to `TextureCacheKeys` (32), `Bvh` (19),
  and `NavMesh` (36). The sections this plan touches — `Lightmap` (22), `AnimatedLightChunks` (24),
  and `AnimatedLightWeightMaps` (25) — are **byte-identical**, so the golden's actual guarantee
  (script-membership plumbing must not change an un-targeted static light's output) still holds; the
  emissive-slot hypothesis in the findings doc is superseded (`Geometry`/`TextureNames` unchanged).
  Because the delta is unrelated legitimate change, regenerate the golden on `main` as a
  pre-requisite so that Task 1's own regen shows a delta scoped to section 25's payload and the
  downstream TOC-offset shift it forces (the check the `--ignored` acceptance criterion demands),
  with no other section's content changing. Regenerating without this first would let Task 1's diff
  bury the section-25 change under section 45 + SH coarsening.
- **Whether Task 5's fixture joins `GATE_FIXTURES`.** Recommendation: no. Those gates bake every
  fixture twice, and dropping one oversized fixture cut them 7× recently. Profile first if revisited.
- **Animated weight maps are in no `GATE_FIXTURES` byte comparison**, despite four gate fixtures
  being named for them — the determinism gates compare the static lightmap and SH sections only.
  Decide whether this plan adds that coverage or leaves the gap standing.
