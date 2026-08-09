# Animated Weight-Map Packer Collapse

## Goal

Stop `prl-build` aborting in `assert_no_overlapping_rects_per_face` when a face
whose chart sits on lightmap atlas layer ≥ 1 carries multiple animated-light
chunks. Every chunk gets either its real, distinct atlas rect (layer-0 charts,
unchanged) or an explicit zero-area degradation that composes nothing — instead
of today's colliding 1×1 placeholder that panics the compile and, when it
doesn't panic, races a black write over another chart's compose texel. Lifts
the stress-Warren content-side animated-light cap that exists only to dodge the
abort.

## Scope

### In scope

- Compiler (`crates/level-compiler/src/animated_light_weight_maps.rs`):
  zero-area degraded-rect representation for off-layer-0 and degenerate-chart
  chunks; zero-area exemption in the per-face overlap assert; one aggregate
  degradation warning; `STAGE_VERSION` bump.
- Format/consumer contract tests pinning zero-area rects as legal: level-format
  consistency, render-cpu cross-section validation, renderer dispatch-tile
  expansion.
- Content: remove `ANIMATED_LIGHT_CAP` and its budget plumbing from
  `tools/gen_stress_map.py`; correct the stress-Warren docs that prescribe the
  cap and the `--lightmap-density 0.25` workaround.

### Out of scope

- Real animated-direct coverage for faces off layer 0. That is the
  `animated-lightmap-array-atlas` draft (section v3, per-chunk layer, array
  compose targets); it removes the degraded path this plan tightens. This plan
  changes nothing that plan consumes — placements, packing, and section version
  stay as they are.
- `chunk_atlas_rect` rounding, the subdivider's pitch floor, and
  `BOUNDARY_SNAP_EPS`. Same-layer arithmetic is sound: 0/4000 randomized
  float32-faithful simulations produce a same-face overlap for in-bounds
  layer-0 charts, and the two historical collapse modes have regression tests
  (`research.md`).
- Chunk-count/section-size scaling under heavily overlapped animated influence
  sets (measured: 64 animated lights on a 4×4×2 warren → 112 106 over-cap
  chunks, ~1.09 M retained light indices). Subdivider capacity, separate
  concern.
- Compose/forward runtime code changes. Verified none are needed: zero-area
  rects are already skipped by `expand_dispatch_tiles` and legal in every
  validator (`research.md` consumer inventory).

## Direction

**Problem.** `bake_one_chunk` returns the identical placeholder rect
`(placement.x, placement.y, 1, 1)` for every chunk of a face whose chart the
multi-bin packer spilled to atlas layer ≥ 1; two such chunks on one face
deterministically trip the per-face overlap assert and kill the whole compile.
Reproduced: a 4×4×2 warren with 64 animated lights at 0.5 m/texel panics with
two `1x1+63+239` rects for chunks whose UV ranges are distinct and disjoint
(`research.md`). Layer spill is the norm — the packer sizes layers to the
largest BSP leaf, so even a 16-room map baked 8 layers — which is also why the
capped 6-light bake silently emits 0 covered texels.

**Prior commitments.**
- Compose writes every texel of every rect, no per-frame clear
  (`animated_lightmap_compose.wgsl` header; `rendering_pipeline.md` §7.1 step
  4). Rect disjointness is therefore a real runtime invariant, not compiler
  pedantry — the assert stays, narrowed to texel-bearing rects.
- The 1×1-degenerate skip for spilled faces was an explicit decision ("NOT an
  assert: faces on layers >= 1 legitimately exist", `bake_one_chunk`). This
  plan keeps that decision and fixes its representation; the two defects it
  causes (assert collision, cross-chart black write) are already documented in
  `animated-lightmap-array-atlas` § Background, whose fix — real per-layer
  coverage — subsumes this degradation later and deletes the path either way.
  No unstated divergence: this is the interim tightening, not a rival design.
- Zero-area rects are within the shipped section-25 v2 invariants — every
  validator does Σ width×height arithmetic and the renderer already skips
  zero-area during tile expansion (`research.md` consumer inventory) — so no
  format version bump.
- Cached-stage contract: output bytes change for spilled/degenerate maps, so
  `STAGE_VERSION` bumps (5 → 6), per the version-constant pattern documented on
  that constant.
- Byte-determinism gate (`build_pipeline.md`): degraded rects are a constant,
  preserving byte-identical rebuilds.

**Alternatives rejected.**
- *Land `animated-lightmap-array-atlas` instead.* Right end state, but a
  format-v3 + GPU-array + forward-shader feature; the compile abort and the
  compose race ship in any spilled map today. This fix is small, and everything
  it adds sits on the path that plan deletes wholesale — nothing to unwind.
- *Exempt degenerate rects in the assert but keep the 1×1 placeholder.* Leaves
  the placeholder's dispatch tile storing black at another chart's layer-0
  coordinates each frame (write race), and keeps misleading cross-layer
  coordinates in the section.
- *Give spilled chunks real texels in layer-0 free space.* The forward pass
  attributes those coordinates to whichever chart owns them; making them
  sampleable requires the slot/layer remap that is exactly the array-atlas
  design — don't half-build it here.

## Packing outcomes

Configuration → packed result. The test tasks cite these rows rather than
restating them.

| # | Face configuration | Today | After this plan |
|---|---|---|---|
| 1 | Chart on layer 0, ≤ cap lights (1 chunk) | real rect | unchanged, byte-identical |
| 2 | Chart on layer 0, > cap lights (N chunks) | N disjoint rects | unchanged, byte-identical |
| 3 | Chart on layer ≥ 1, 1 chunk | 1×1 placeholder at foreign coords; dispatch tile stores black | zero-area rect; no entries, no tile, no write |
| 4 | Chart on layer ≥ 1, ≥ 2 chunks | **panic**, compile lost | zero-area rects; compiles; aggregate warning |
| 5 | Degenerate chart (`uv_extent ≤ 0`; subdivider emits exactly 1 chunk) | 1×1 rect, all texels zero-count | zero-area rect |
| 6 | Every chunk in the map degraded | n/a (panic first, rows 3–4) | section with rects but empty `texel_lights` → renderer's existing dummy-atlas early-out |
| 7 | Two texel-bearing rects overlap on one face (packer regression) | panic | still panics — invariant kept |
| 8 | Zero-area rect at coordinates inside a texel-bearing rect | n/a | no panic — assert exempts zero-area explicitly (strict-inequality test would false-positive; `research.md`) |

## Acceptance criteria

- [ ] AC1 — A bake where a multi-chunk animated face sits on atlas layer ≥ 1
      completes without panicking; its section passes `is_consistent` and
      render-cpu cross-section validation (row 4).
- [ ] AC2 — Degraded chunks contribute nothing: zero offset-count entries, zero
      weight entries, no dispatch tile — verified at the baker, the format
      validators, and the renderer's tile expansion (rows 3, 5, 8).
- [ ] AC3 — Two overlapping texel-bearing rects on one face still abort the
      bake (row 7).
- [ ] AC4 — A bake with ≥ 1 degraded chunk logs one aggregate warning naming
      degraded chunk and face counts; a bake with none logs no such warning.
- [ ] AC5 — Layer-0-only fixtures bake byte-identically to before the change:
      the existing weight-map module tests, `animated_weight_maps_fixtures.rs`
      integration tests (including the golden-PRL comparison), and the
      byte-determinism tests pass unmodified (rows 1–2).
- [ ] AC6 — `STAGE_VERSION` is bumped and the existing stage-version cache
      tests (miss-then-hit, key-change) pass against the new constant.
- [ ] AC7 — `tools/gen_stress_map.py` has no animated-light cap; a 4×4×2
      `--lights static --lights-per-room 3 --animated-frac 1.0` map compiles at
      `--lightmap-density 0.5` with exit 0 and the AC4 warning (manual command;
      ~minutes of bake, not CI).
- [ ] AC8 — `content/dev/maps/stress-warren.README.md` and the generator
      docstring no longer instruct the 6-light cap or claim the packer aborts
      at 0.5 m/texel; both state that faces off atlas layer 0 bake no animated
      direct light until the array-atlas plan lands.

## Tasks

### Task 1: Zero-area degraded rects in the weight-map baker

In `crates/level-compiler/src/animated_light_weight_maps.rs`:

- `bake_one_chunk`: the `placement.layer != 0` early-return currently emits
  `ChunkAtlasRect { atlas_x: placement.x, atlas_y: placement.y, width: 1,
  height: 1, texel_offset: 0 }` plus one zero-count `TexelLightEntry`. Replace
  with `ChunkAtlasRect { atlas_x: 0, atlas_y: 0, width: 0, height: 0,
  texel_offset: 0 }`, empty `offset_counts`, empty `texel_lights`. Hoist the
  degenerate-chart check (`chart.uv_extent[0] <= 0.0 || chart.uv_extent[1] <=
  0.0`, today the `chart_usable` flag) to the same early-return so degenerate
  charts take the identical degraded path; `chunk_atlas_rect`'s own degenerate
  branch becomes defensive-only and stays untouched.
- `assert_no_overlapping_rects_per_face`: skip any rect with `width == 0 ||
  height == 0` before the pair test, with a comment noting the
  strict-inequality test would otherwise flag a zero-area rect nested inside a
  real one (row 8). The panic path for texel-bearing overlaps is unchanged.
- `bake_animated_light_weight_maps_controlled`: after the per-chunk collect,
  count degraded chunks and the distinct faces they belong to; if nonzero, emit
  one `log::warn!` naming both counts and the cause ("chart off atlas layer 0
  or degenerate — these chunks bake no animated direct light"). The
  concatenation loop, byte-size formula, and stats already tolerate zero-area
  rects — no change.
- Bump `STAGE_VERSION` 5 → 6, extending the constant's changelog doc comment
  (degraded chunks now emit zero-area rects; prior placeholder bytes must not
  be served from cache).
- In `crates/level-format/src/animated_light_weight_maps.rs`, extend the
  `ChunkAtlasRect` doc comment: zero-area (width = height = 0) marks a degraded
  chunk — no texels, no offset_counts entries, coordinates meaningless and
  pinned to 0.
- Module tests: (a) repro — geometry fixture with a hand-built
  `ChartPlacement { layer: 1, .. }` and a two-chunk face; panics pre-fix,
  post-fix yields two zero-area rects, empty entries, `is_consistent`, and the
  aggregate warning (AC1, AC2, AC4; assert log via `test-log-capture` per
  `testing_guide.md` §3); (b) `#[should_panic]` — call
  `assert_no_overlapping_rects_per_face` directly with two hand-built
  texel-bearing overlapping `ChunkBakeResult`s (AC3); (c) zero-area rect at
  coordinates inside a texel-bearing rect on the same face does not panic
  (row 8); (d) degenerate-chart fixture emits a zero-area rect (row 5);
  (e) existing determinism and stage-version tests pass unmodified (AC5, AC6).

### Task 2: Consumer contract tests for zero-area rects

No production code changes — pin the existing behavior the fix relies on.
In `crates/render-cpu/src/animated_lightmap.rs` tests: `validate_cross_section`
accepts a section mixing one real and one zero-area rect (prefix sums remain a
partition; rect count still matches the chunk section). In
`crates/renderer/src/render/animated_lightmap.rs` tests (module has a
`#[cfg(test)]` block): `expand_dispatch_tiles` emits no tile for zero-area
rects and correct tiles for the surviving real rects. In
`crates/level-format/src/animated_light_weight_maps.rs` tests: a section with a
zero-area rect round-trips `to_bytes`/`from_bytes` and passes `is_consistent`
(AC1, AC2).

### Task 3: Lift the stress-Warren animated-light cap

In `tools/gen_stress_map.py`: delete `ANIMATED_LIGHT_CAP` and the
`anim_budget` single-element-list plumbing threaded through `emit_room_lights`
and its callers; `--animated-frac`
alone selects which baked lights animate. Rewrite the module docstring's
animated-lights paragraph and the "Animated lights" section of
`content/dev/maps/stress-warren.README.md`: drop the 0.25-vs-0.5 abort guidance
and the cap rationale; state that any density compiles, that animated direct
light bakes only for faces whose chart landed on lightmap atlas layer 0 (the
compiler warns with degraded counts — AC4), and that full multi-layer coverage
is the `animated-lightmap-array-atlas` plan. Keep the existing atlas-size and
bake-time density guidance. Verify with the AC7 command (AC7, AC8).

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; carries the repro that
falsifies the diagnosis and the fix itself.
**Phase 2 (concurrent):** Task 2, Task 3 — independent crates/files; Task 3's
AC7 verification consumes Task 1's compiler.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Texel-bearing chunk rects on one face are pairwise disjoint (compose writes every rect texel — overlap is a GPU write race) | Pre-existing: `chunk_atlas_rect` half-open ownership + subdivider pitch floor | Task 1 narrows the assert to texel-bearing rects; a leak of the zero-area exemption to real rects would mask regressions | AC3 |
| Degraded chunk ⇒ zero compose writes (no entries, no tile) | Task 1 (baker), pre-existing `expand_dispatch_tiles` skip | Renderer tile expansion; any future consumer that iterates rects must honor zero-area | AC2 |
| Rebuild determinism: identical inputs → byte-identical section | Pre-existing; degraded rect is a constant | Task 1 | AC5 |
| Stale cache entries unreachable after output change | Task 1 `STAGE_VERSION` bump | — | AC6 |
| `chunk_rects` pairs 1:1 with `AnimatedLightChunks.chunks` | Pre-existing; degradation keeps the record | Task 1 must emit a record per chunk, never drop one | AC1 (cross-section validation) |

## Rough sketch

All compiler edits sit in `animated_light_weight_maps.rs` (`bake_one_chunk`,
`assert_no_overlapping_rects_per_face`,
`bake_animated_light_weight_maps_controlled`, `STAGE_VERSION`); the
degraded-rect constant is `ChunkAtlasRect { 0, 0, 0, 0, texel_offset }` with
`texel_offset` filled by the existing concatenation pass (adds 0 to the running
offset). Counting degraded chunks needs no new plumbing: `ChunkBakeResult`
already flows to the concat loop, and `width == 0` identifies degradation;
face indices come from the parallel `chunks` slice. No renderer, loader, or
wire-format code changes — zero-area is legal under section-25 v2 arithmetic
(`research.md` consumer inventory).

## Open questions

None. The one deferred behavior — real animated-direct coverage for spilled
faces — is owned by `animated-lightmap-array-atlas` (its Scope names removal of
the layer-0 gate and the degenerate-rect path).
