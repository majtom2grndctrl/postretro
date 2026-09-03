# Capture harness: dynamic receivers + authored animation state

## Goal

The static frame-capture harness (`crates/postretro/src/capture`, E20) renders a
world-only, script-free, single-instant frame: the scene vocabulary is
`{map, camera, resolution, output}` (`scene.rs`, `deny_unknown_fields`), the driver runs
no script VM (`now_seconds = 0`), and it installs **static world lights only**
(`capture_static_lights` filters `!is_dynamic`; `entity_shadow_lights: &[]`; no
mover/skinned/billboard entity draws — `driver.rs` builds `LevelGeometry` straight from
`level_world_to_geometry` with empty influence/shadow inputs). Because of this, several
visual acceptance surfaces cannot be automated as capture goldens today — most recently
the "animated direct SH on dynamic receivers" feature, whose AC1/AC2/AC4/AC5/AC10 visual
halves had to fall back to `[manual GPU]` eyeball checks.

Extend the capture harness two ways — (1) render dynamic receivers (kinematic movers,
`prop_mesh` skinned meshes, `billboard_emitter` billboards), and (2) accept an **authored
animation state** (a frozen time / forced-active input) so a static capture can reach an
animated light's "fired" appearance **without** running the script VM or an event loop.
This converts the deferred manual-GPU checks into deterministic, threshold-based capture
goldens, restoring CI visual-regression coverage for baked-animated-light features.

## Scope

### In scope

- **Entity-draw population in capture.** Thread the dynamic-receiver draws the runtime
  frame builds — kinematic movers, `prop_mesh` skinned meshes, `billboard_emitter`
  billboards — into the capture `LevelGeometry`, so a captured frame shows the same
  receivers the windowed engine does.
- **Authored animation-state input.** Add an optional field to `CaptureScene` (e.g.
  `anim_time_s` and/or a per-tag `force_active` map) that seeds the shared compose
  animation descriptors to a chosen keyframe, so the capture renders the animated
  light at (say) its red alarm peak deterministically. No script VM, no trigger firing —
  the state is authored directly in the scene JSON.
- **Golden coverage** for baked-animated-light dynamic-receiver features: convert the
  `[manual GPU]` visual checks in `animated-direct-sh-dynamic-receivers` (AC1/AC2/AC4/
  AC5/AC10) into capture goldens against an authored red frame.
- Keep the harness deterministic and GPU-adapter-gated (self-skip without an adapter),
  matching the existing E20 golden discipline.

### Out of scope

- **Running the script VM / firing triggers in capture.** The animation state is authored
  in the scene JSON, not produced by executing `spawner-test.ts`. Full end-to-end script
  execution in a headless frame loop is a separate, larger effort (the "full integration
  harness" option), not this spec.
- **Event loop / multi-frame animation.** A single authored instant per capture, not a
  time series.
- **New engine lighting behavior.** This is a test-harness capability; it changes no
  runtime rendering path, only what the capture driver feeds and freezes.

## Dependencies

Depends on `animated-direct-sh-dynamic-receivers` shipping first (it provides the
compose animation descriptors and the dynamic-receiver direct term this harness would
freeze and photograph). This spec is the deferred follow-up that plan's Task 5 names.

## Acceptance criteria

- [ ] **AC1** `[unit]` `CaptureScene` accepts the new authored-animation-state field(s)
      and rejects malformed input via the existing `deny_unknown_fields` + validation path
      (mirror the `scene.rs` parser tests).
- [ ] **AC2** `[golden]` A capture scene with an authored red alarm state on
      `spawner-test.prl` renders the closet door mover, the `prop_mesh`, and the
      `billboard_emitter` sprite reddened with the wall inside the cone — the automated
      replacement for the manual-GPU checks deferred from `animated-direct-sh-dynamic-receivers`.
- [ ] **AC3** `[golden]` A capture scene with no authored animation state (or all
      descriptors inactive) renders the same receivers unlit-by-the-animated-term,
      byte-comparable to the pre-fire baseline (no-pop / start-dark coverage).
- [ ] **AC4** `[review]` The capture driver populates dynamic-receiver draws from the
      same seam the runtime frame uses (no capture-only divergent draw path).
- [ ] **AC5** `[manual GPU]` + `[review]` The harness stays adapter-gated and
      deterministic (self-skips without an adapter; identical bytes across runs on one
      adapter).
- [ ] **AC6** `[review]` For each `animated-direct-sh-dynamic-receivers` AC whose deferred
      `[manual GPU]` visual half is now reproduced by a capture golden here, that half is
      re-tagged `[golden]` in the dependency spec with a reference to the covering scene
      (AC2 or AC3). A half asserting behavior outside this spec's single-authored-instant
      scope — despawn/reload drop, brightness-pop transition, animation over time — stays
      `[manual GPU]`, so the conversion is per-half, not a whole-AC flip. Verified by reading
      the updated dependency spec.
- [ ] **AC7** `[review]` `context/lib/rendering_pipeline.md` §7.8 (frame capture) records
      the dependency: the capture harness reaches an animated light's fired appearance by
      freezing an authored animation state, with no script VM or event loop. Verified by
      grep/read.

## Tasks

### Task 1: Authored animation-state scene input

Extend `crates/postretro/src/capture/scene.rs`: add optional `anim_time_s: Option<f32>`
and/or `force_active: Option<Vec<...>>` (per-light-tag or per-descriptor) to
`CaptureScene`, with validation consistent with the existing finite/range checks. In
`driver.rs`, seed the shared compose animation descriptors to the authored state before
`capture_frame_indirect` (compute the same descriptor values the runtime `light_bridge`
would, at the frozen time). Parser + validation unit tests mirror the existing `scene.rs`
suite.

### Task 2: Dynamic-receiver draws in capture

Thread kinematic-mover, `prop_mesh`, and `billboard_emitter` draws into the capture
`LevelGeometry` from the same construction the runtime frame uses
(`level_world_to_geometry` / the entity-draw seam), rather than the current world-only
build in `driver.rs`. Confirm the three consumers sample the composed direct atlas
(binding 15) in the captured frame exactly as in the windowed engine.

### Task 3: Convert deferred manual-GPU checks to goldens

Add capture scenes + GPU-golden regressions asserting the reddened dynamic receivers
(AC2) and the pre-fire baseline (AC3). Once these land, re-tag the deferred `[manual GPU]`
visual halves in `animated-direct-sh-dynamic-receivers` that a golden here now covers —
the "receiver reddens with the wall" halves and the start-dark baseline — to `[golden]`,
each referencing the covering scene, and leave the out-of-scope halves as `[manual GPU]`
per AC6. Note the harness dependency in `context/lib/rendering_pipeline.md` §7.8 (frame
capture) (AC7).

## Notes

Grounding for the harness extension:
- `crates/postretro/src/capture/scene.rs` — scene vocabulary + validation.
- `crates/postretro/src/capture/driver.rs` — `capture_static_lights`,
  `LevelGeometry { entity_shadow_lights: &[] }`, `capture_frame_indirect`.
- `crates/renderer/src/render/renderer_geometry.rs::level_world_to_geometry` — the
  geometry-build seam Task 2 extends.
- See also `context/plans/*/E20--frame-capture` for the original capture design intent.
