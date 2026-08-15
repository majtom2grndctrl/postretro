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
- Loud diagnostics: aggregate counts of animated layers and slots, and a warning when slots are
  dropped.

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
      single-slot and multi-slot sections; a test asserting a v2 payload decodes with every chunk
      on layer 0 and one animated slot; a test asserting an unsupported version is still rejected
      by a message naming the version.
- [ ] `cargo test -p postretro-level-compiler --bin prl-build` passes, including a test that bakes
      weight maps from placements spanning two layers and asserts both layers produce covered
      texels with distinct slots. No test may construct this by mutating a layer field on an
      otherwise layer-0 fixture — the placements must come from a real multi-layer pack.
- [ ] A test asserts no two chunk rects overlap within one `(slot, layer)`, across faces — not only
      within a face.
- [ ] Compiling a map whose animated receivers span two atlas layers logs, at `info`, the animated
      layer count and the slot count. Compiling a map that exceeds the animated slot cap fails the
      bake with an error naming the cap and the count found.
- [ ] No compile path emits a chunk rect whose width and height are both 1 as a stand-in for
      "skipped". A test asserts the degenerate-rect shape is gone.
- [ ] `cargo test -p postretro-renderer` passes, including a test asserting the group-4 BGL entries
      for the animated atlas and animated direction bindings declare `D2Array`. This must assert
      `view_dimension` explicitly — the existing entry test pins `sample_type` only, so a partial
      promotion currently passes.
- [ ] A test asserts the compose shader parses under `naga` and stores through an array-indexed
      write on both compose targets.
- [ ] A test asserts the forward shader samples both animated atlases with a layer argument and
      contains no `lightmap_layer == 0u` animated guard.
- [ ] A static layer with no animated receivers resolves to zero animated contribution, not to
      slot 0's contents. Verified by a test on the pure layer→slot resolution, independent of a
      GPU: a lookup table built from a slot table that omits a layer returns the no-contribution
      sentinel for it.
- [ ] The resolver that reports usable atlas dimensions reports the layer count with them, and a
      test asserts the animated atlas is created at those dimensions — so a future change cannot
      silently desynchronise animated width/height from the static atlas the compose pass shares
      coordinates with.
- [ ] A unit-testable helper decides whether an animated atlas of a given width, height, and slot
      count fits the VRAM budget; a test exercises an over-budget case. Over budget logs a
      `[Renderer]` error and the level renders with no animated contribution rather than failing.
- [ ] Loading a level whose animated atlas construction fails leaves no previous level's atlas
      views bound. (Today the failure path logs and returns without reassigning either resource
      field, so stale views stay bound; this plan adds failure modes to that constructor.)
- [ ] A dev fixture exists whose baked animated light has receivers on atlas layer ≥ 1, confirmed
      by reading `layer_count` and the slot table out of its compiled PRL — not asserted from map
      authoring alone. Running that map shows the animated light on the layer-≥1 faces, before and
      after its curve fires. **[manual GPU]** — this is the end-to-end check that the promotion
      works; no existing fixture reaches layer ≥ 1.
- [ ] `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler -- --ignored`
      shows no regression against the pre-change baseline. The golden PRL is regenerated, and the
      regeneration is justified by a byte-delta diff confirming the change is confined to section
      25's new fields.

## Tasks

### Task 1: Section 25 v3 wire format

Add `layer: u32` to `ChunkAtlasRect` as an appended field, taking `CHUNK_RECT_SIZE` from 20 to 24.
Add an animated-slot count and a slot→static-layer table to the section, so the runtime can size
the atlas and build the inverse lookup without scanning every rect. Bump the version constant to
3. The encoder always writes v3. The decoder resolves version to a capability flag once, up front,
then branches where the appended data would be read — the `TriggerVolumes` v1→v2 decoder is the
pattern to mirror, with one difference: this decoder currently rejects every non-matching version,
so the graceful branch is new. A v2 payload decodes as every chunk on layer 0 with exactly one
animated slot mapping to static layer 0, which is what v2 meant. This section computes its total
size up front from fixed strides rather than walking a self-describing stream, so the stride used
in that computation becomes version-dependent; the appended slot table is variable-length and must
be counted in it. Extend `is_consistent` to check that every rect's layer appears in the slot table
and that the table is sorted and duplicate-free. Fix the layout doc block, which still says
`version (= 1)` two bumps later.

### Task 2: Compiler — bake every layer, assign slots, diagnose

Remove the `placement.layer != 0` gate and the degenerate-rect return. Collect the distinct static
layers holding animated receivers, sort them, and assign dense slots; emit the slot table and set
each rect's layer. Thread the atlas layer count out of the lightmap bake into the weight-map stage
— `pipeline.rs` currently destructures `layer_count: _` with a comment saying animated weight maps
are single-layer, which is the line this plan invalidates. Add the animated-slot cap and a named
bake error mirroring `LightmapBakeError::LayerOverflow`. Rework the stage's `info` line so spilled
chunks are no longer folded invisibly into healthy-looking stats: report animated layer and slot
counts, and warn when the cap drops slots, following the aggregate-count-plus-rate-limited-detail
shape `animated_light_chunks.rs` already uses for its per-chunk cap. Widen the per-face overlap
assert to bucket by layer and to catch cross-face collisions within a layer, since a per-face-only
assert is what let the borrowed-coordinate collision hide. Bump the stage's cache-key version. The
byte-size log carries its own copy of the encoder's stride constants; update it in lockstep.

### Task 3: GPU — array atlases, slot-indexed compose and sampling

Create both compose targets with `depth_or_array_layers` set to the section's animated slot count,
and pin `D2Array` on their storage and forward views — the static irradiance/direction/shadowmask
views already do this explicitly and are the pattern to copy. Flip the two compose BGL storage
entries and the two group-4 sampled entries to `D2Array`. Carry the target slot on `DispatchTile`,
which has a spare `_pad` word, resolving rect layer → slot during tile expansion so the shader does
no lookup; all three `textureStore` sites (debug heatmap, irradiance, direction) take an array
index. Give the forward pass a static-layer→slot lookup so it can sample by `in.lightmap_layer`,
replacing the layer-0 guard; layers with no animated receivers must resolve to no contribution.
Size that lookup for the existing 256-layer static ceiling and respect uniform array-stride rules.
The single 1×1 `Rgba16Float` dummy texture backs both fallback views and needs array-compatible
views, or the no-animated-lights path breaks on layout incompatibility. The VRAM `info` log
computes `width × height × 12` with no layer factor; fix it. Keep the compose shader parsing under
`naga` — an existing test pins that, and another pins a compose binding by literal string.

### Task 4: Runtime validation and graceful degradation

`usable_atlas_dimensions` consumes `max_texture_array_layers` only to reject, and discards
`layer_count` from what it returns; expose the layer count and update both call sites. Add a
pure, unit-testable helper deciding whether a given width, height, and slot count fits the VRAM
budget, and a load-time path that logs a `[Renderer]` error and falls back to no animated
contribution when it does not — matching how an oversize static section degrades rather than
aborting. Extend cross-section validation, which today checks prefix sums and light-index bounds
but has no layer notion, to reject rects whose layer is outside the slot table and whose
coordinates fall outside the atlas. Fix the install path so a failed animated-lightmap
construction cannot leave the previous level's views bound: today the error arm logs and returns
without reassigning, and only the success arm rebuilds the group-4 bind group.

### Task 5: Multi-layer verification fixture

No existing fixture reaches atlas layer ≥ 1 at production dimensions, so the promotion has no
end-to-end check. Author a dev map with a baked animated light whose receivers land on a layer
above 0, and confirm the spill by reading `layer_count` and the slot table out of the compiled PRL
rather than inferring it from the map. Spilling is driven by the *sum* of BSP-leaf footprints
exceeding one layer while each leaf still fits alone — not by total texel count and not by `.map`
file size — so shape the geometry for leaf count, and expect to iterate against the compiled
output. Do not raise `_lightmap_density` expecting more layers; the per-layer dimension grows with
the charts, so density alone does not spill. Leave `switch-demo`'s indicator on the dynamic tier:
dynamic is the better design for a small press indicator and reverting it would weaken that fixture
to serve this one. Record the measured layer count in the map's header comment, since nothing in the
pipeline reports it and the next reader will otherwise re-derive it. Do not add this fixture to
`GATE_FIXTURES` — see Open questions.

## Sequencing

**Phase 1 (sequential):** Task 1 — every other task reads the v3 shape.
**Phase 2 (concurrent):** Task 2, Task 3 — disjoint crates, both consume Task 1's format.
**Phase 3 (concurrent):** Task 4, Task 5 — Task 4 consumes Task 3's atlas construction and Task 1's
slot table; Task 5 needs Tasks 2 and 3 to compile and render a multi-layer map.

## Wire format

Section 25, version 3. Little-endian throughout, mirroring the existing v2 layout: a fixed header
carrying the version and every array count, then flat arrays in header order. No per-array length
prefix and no per-entry counts. Appended fields go last within `ChunkAtlasRect`, and the new slot
table goes after the existing three arrays, so a v2 reader's field offsets are unchanged up to
where it stops.

- `ChunkAtlasRect`: existing `atlas_x, atlas_y, width, height, texel_offset` then appended
  `layer: u32`. Stride 20 → 24. Coordinates stay per-layer, as the packer emits them.
- Header gains the slot count. The slot table is `u32` static-layer indices, ascending, no
  duplicates; index into it *is* the slot.
- Empty section: header with zero counts and an empty slot table, matching v2's
  header-only encoding for the empty case.
- v2 decode: every rect layer 0, slot table `[0]`, slot count 1. A section with no chunks decodes
  to an empty slot table and slot count 0.
- Unsupported versions stay a hard `InvalidData` error naming the version found.

The TOC blob's per-section `version` field is hardcoded to 1 for every optional section and is not
the payload version. Do not change it.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Every chunk with animated receivers gets a real rect and a slot — no rect is a skip sentinel | Task 2 (gate removal, slot assignment) | Exceeding the slot cap must fail the bake, never fall back to a placeholder rect | AC: cap fails the bake · AC: degenerate shape gone |
| Slot table is a sorted, duplicate-free bijection onto occupied static layers; a layer absent from it yields no animated contribution | Task 1 (`is_consistent`), Task 2 (assignment) | Task 3's forward lookup must map an absent layer to no contribution, not to slot 0's contents | AC: v3 round-trip · AC: absent layer resolves to sentinel |
| Animated atlas width and height equal the static atlas's; only layer depth differs | Task 3, Task 4 (resolver) | Compose stores at absolute per-layer coordinates and forward samples all atlases with one normalized UV — enforced today by doc comment and one resolver test, no assert | AC: resolver reports dimensions with layer count |
| Any texel the forward pass samples was composed this frame or is zero-initialized | Task 3 | Allocating slots the compose does not write relies on wgpu zero-init; a per-frame clear is deliberately absent | AC: absent layer resolves to sentinel · AC: multi-layer fixture renders |
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
  load-time byte budget, but neither number is chosen. Pick the cap from measured layer counts on
  real content rather than from the static 256.
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
  pre-requisite so that Task 1's own regen shows a delta **confined to section 25's new `layer`
  field** — the check the acceptance criterion demands. Regenerating without this first would let
  Task 1's diff bury the section-25 change under section 45 + SH coarsening.
- **Whether Task 5's fixture joins `GATE_FIXTURES`.** Recommendation: no. Those gates bake every
  fixture twice, and dropping one oversized fixture cut them 7× recently. Profile first if revisited.
- **Animated weight maps are in no `GATE_FIXTURES` byte comparison**, despite four gate fixtures
  being named for them — the determinism gates compare the static lightmap and SH sections only.
  Decide whether this plan adds that coverage or leaves the gap standing.
