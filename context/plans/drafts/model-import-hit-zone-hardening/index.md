# Model Import and Hit-Zone Hardening

## Goal

Turn the E19 review findings into a deliberate hardening pass. Audit model
import, skeletal hit-zone authority, and mesh-pose cache lifecycle so malformed
asset data and unavailable precise poses fail safe instead of producing poisoned
render data or false no-hit gameplay results.

This is not a feature plan. It is an investigation with required repairs. The
main deliverable is an evidence matrix: each audited edge case is either already
covered, fixed with a regression test, or explicitly deferred with rationale.

## Scope

### In scope

- glTF boundary parsing in `crates/model/src/gltf_loader.rs`, including present
  but unreadable accessors, non-finite values, degenerate direction streams,
  partial triangle lists, empty skins, malformed animation channels, and
  malformed hit-zone extras.
- Runtime consumers of `LoadedModel`, `Submesh`, `JointZone`, `ModelLoadError`,
  skeletons, clips, submesh ranges, and mesh bounds.
- Game-side skeletal hit-zone authority in
  `crates/postretro/src/scripting/systems/hit_zones.rs`: precise capsule hit,
  authoritative miss, unavailable pose, static-identity degradation, authored
  AABB fallback, and derived reach-bound fallback.
- Pose parity between game-side hit zones and rendered mesh sampling, including
  same-tick switches, pending animation stamps, per-instance phase, transform
  placement, and frozen animation time.
- Mesh pose cache lifecycle in `crates/postretro/src/render/mesh_pass.rs` and
  collection flags in `crates/postretro/src/scripting/systems/mesh_render.rs`.
- Durable context drift for the contracts above, especially
  `context/lib/rendering_pipeline.md` and `context/lib/entity_model.md`.

### Out of scope

- New renderer crate extraction work. That belongs to the remaining E19 render
  stack plans.
- GPU validation or visual correctness tests. Tests stay GPU-free and exercise
  data and gameplay contracts.
- General glTF feature expansion. Unsupported topology, missing required data,
  and malformed present data should be rejected or degraded; this plan does not
  add support for new asset shapes.
- Rebalancing hit-zone damage multipliers or weapon behavior.
- A broad fuzzing framework. Property tests are allowed when they are small and
  targeted, but this plan is an audit with concrete regression coverage.

## Audit Method

For each row in the matrix, trace the producer, consumer, expected authority, and
observable failure mode. Then choose one outcome.

| Outcome | Meaning |
|---|---|
| Covered | Existing code and tests already pin the invariant. Name the test. |
| Fixed | Code changed and a regression test now pins the behavior. |
| Deferred | The issue is real but outside this plan. Name the follow-up. |
| Rejected | The suspected issue is invalid after code-grounding. Explain why. |

Every `Fixed` row needs a test unless the behavior is compile-time only. Every
`Rejected` row needs a source reference. The matrix may live in
`research.md` beside this spec during implementation; only durable conclusions
belong in `context/lib/`.

## Failure Taxonomy

| Class | Safe behavior |
|---|---|
| Required mesh data missing or unreadable | `load_model` returns `ModelLoadError`. |
| Present optional vertex data malformed | Reject or intentionally degrade before packing renderer-visible bytes. |
| Authored animation channel malformed | Warn and skip the channel; do not extend clip duration from skipped data. |
| Static or empty-skin model with hit-zone tags | Preserve shootability through authored AABB or derived reach fallback when precise capsules are not authoritative. |
| Precise pose unavailable game-side | Degrade to coarse fallback; never report a false no-hit. |
| Precise pose available and trustworthy | Capsule result is authoritative; do not widen to coarse AABB after a real capsule miss. |
| Render pose cache skipped or empty frame | Reuse only frame-valid palette data; evict stale entries even when no instances are planned. |
| Context contract drift | Update durable context so future agents do not reintroduce old assumptions. |

## Acceptance Criteria

- [ ] An audit matrix exists for the in-scope surfaces and every row is marked
      `Covered`, `Fixed`, `Deferred`, or `Rejected`.
- [ ] All `Fixed` rows have focused regression tests. Test names describe the
      boundary behavior, not implementation mechanics.
- [ ] Malformed present glTF data cannot panic or produce non-finite/degenerate
      renderer-visible packed vertex data.
- [ ] Skinned model topology cannot produce submesh index ranges that are not
      whole triangle lists.
- [ ] Empty or malformed skins cannot shadow later valid skinned nodes during
      model selection.
- [ ] Animation clips do not derive duration from channels whose tracks were not
      installed.
- [ ] Game-side hit-zone queries distinguish authoritative misses from
      unavailable or untrustworthy precise poses.
- [ ] Same-tick animation switches sample with the same effective clock and
      pending-stamp behavior as the visible render frame.
- [ ] Mesh pose palette cache entries are not reused after an empty or skipped
      frame unless touched by the current plan.
- [ ] `context/lib/rendering_pipeline.md` and `context/lib/entity_model.md`
      describe the final hit-zone authority and transform contract.
- [ ] Focused gates pass:
      `cargo test -p postretro-model gltf_loader --lib`,
      `cargo test -p postretro hit_zones`,
      `cargo test -p postretro mesh_render`,
      `cargo test -p postretro palette_cache`, and
      `cargo check -p postretro --tests`.
- [ ] Final preflight passes: `cargo fmt --check`,
      `cargo clippy -- -D warnings`, and `cargo test`.

## Tasks

### Task 1: Build the evidence matrix

Create a local audit matrix for the failure taxonomy above. Ground every row in
source before classifying it. Start from `load_model`, `nearest_entity_hit`,
`MeshRenderCollector::collect`, and `MeshPass::plan_and_upload`; follow values
to the first consumer that would observe corruption, a false miss, or stale pose
data. Keep source notes in the plan folder if they are useful during review, but
do not copy large code snippets into the spec.

### Task 2: Split test fixtures before extending oversized files

`gltf_loader.rs` and `hit_zones.rs` are already oversized. Before adding a large
new batch of fixtures, split reusable test builders into sibling `#[cfg(test)]`
fixture modules if the new helpers would make the file harder to scan. Keep the
split behavior-preserving and run focused tests before adding new cases.

### Task 3: Harden glTF ingress

Audit required and optional glTF streams in `gltf_loader.rs`. Required geometry
or skeleton data should fail with typed `ModelLoadError` variants. Optional
present streams should either reject malformed data before packing or degrade in
a documented way. Cover index topology, non-finite floats, degenerate normals and
tangents, skinning attribute pairs, empty skins, inverse-bind matrices, joint
remaps, and animation channels.

### Task 4: Harden hit-zone authority

Audit `hit_zones.rs` for every path that can answer a hitscan query. Pin which
misses are authoritative and which must degrade to coarse fallback. Cover
authored AABB fallback, derived reach fallback, static-identity zones, no-op
clips, unavailable chained smooth-interrupt poses, full transform placement,
non-finite transforms, scale effects on capsule radius, and zone-only entities.

### Task 5: Reconcile render-pose parity

Trace animation sampling from game logic through mesh render collection and
renderer palette upload. The game-side hit-zone query must sample the same
logical pose the visible render frame uses when precise parity is possible.
Cover pending animation stamps, same-tick switches, frozen animation time,
per-instance phase, render resample flags, snapshot captures, and palette cache
eviction.

### Task 6: Update durable contracts

Update `context/lib/rendering_pipeline.md` and `context/lib/entity_model.md`
only after the audit settles the final contracts. Capture durable behavior:
renderer owns GPU, game logic owns hit decisions, precise hit zones are
authoritative only when the game side can reconstruct the visible pose, and
fallbacks preserve shootability without making every miss coarse.

## Sequencing

**Phase 1 (sequential):** Task 1 — defines the audit rows and prevents aimless hunting.

**Phase 2 (sequential):** Task 2 — only if new fixture volume would extend oversized files.

**Phase 3 (concurrent):** Task 3, Task 4 — separate model-ingress and gameplay-authority work.

**Phase 4 (sequential):** Task 5 — consumes the final hit-zone authority rules from Task 4.

**Phase 5 (sequential):** Task 6 — durable docs update after behavior is settled.

## Investigation Notes

- The E19 review rounds found real latent bugs, not just extraction mistakes:
  malformed present streams, no-op clips changing authority, stale palette cache
  lifecycle, and docs that lagged runtime behavior.
- The review signal is now diminishing for E19 itself. This plan gives that
  bug-hunting energy a bounded home with explicit stop conditions.
- Prefer small regression fixtures over large asset files. A fixture should show
  the boundary shape that failed.
- When a row is already covered, record the test and move on. The goal is
  confidence, not endless novelty.

## Open Questions

- Should this plan produce a committed `research.md` evidence matrix, or should
  the matrix stay as implementation notes unless it contains decisions worth
  preserving?
- Should promotion require a review panel, or is one code-review pass enough
  after the matrix closes?
