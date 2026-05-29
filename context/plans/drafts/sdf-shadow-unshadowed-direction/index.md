# SDF Shadow — Unshadowed-Mode Direction Fix

## Goal

Make the animated-light dominant-direction bake honor the lightmap's `Unshadowed`
bake mode, so animated lights produce runtime SDF shadows the same way static
lights do. Today the animated bake culls occluded (texel, light) pairs
unconditionally; in `Unshadowed` mode that drops the direction at exactly the
texels that should be shadowed, and the trace marches the world-up default into
open space — no shadow. The static path already gates this cull on bake mode;
this aligns the animated path with it, and captures the shared contract as an
executable test plus a context-library doc.

## Background

Confirmed by end-to-end trace this session. The lightmap supports two bake modes:
`Shadowed` (visibility baked into the lightmap) and `Unshadowed` (full irradiance
baked; runtime SDF supplies visibility). For the SDF path the lightmap is baked
`Unshadowed` and a per-texel dominant direction tells the trace which way to march.

- The **static** lightmap bake gates its visibility cull on `mode == Shadowed`,
  so in `Unshadowed` mode it retains both irradiance and the dominant direction
  for occluded texels. The static (r-channel) direction is correct.
- The **animated** weight-map bake culls occluded pairs **unconditionally** — it
  has no bake-mode parameter. In `Unshadowed` mode the animated (g-channel)
  direction atlas is empty at occluded texels; the compose pass then writes the
  world-up default, and the trace finds no occluder.

This fix targets **animated lights**. The static path is already correct, so a
plain static light's missing shadow (the current `occlusion-test.prl` symptom) is
a build/bake-state issue, tracked separately — not this bug.

## Scope

### In scope

- Thread the lightmap bake mode into the animated weight-map bake and gate its
  visibility cull on `Shadowed`, mirroring the static bake. In `Unshadowed` mode,
  retain occluded (texel, light) pairs — both the contribution weight and the
  direction — so the animated irradiance is unshadowed and the direction points
  at the occluded light, symmetric with the static path.
- Invalidate stale cached animated bakes (stage-version bump).
- A behavioral contract test on the animated bake mirroring the existing static
  test: in `Unshadowed` mode an occluded texel keeps a direction toward its light;
  in `Shadowed` mode it is still culled.
- A context-library doc capturing the sub-system's data flow and the
  visibility-matches-bake-mode invariant.

### Out of scope

- The static lightmap / static direction bake. Untouched.
- New GPU bindings, pipeline, or bind-group changes.
- Wire-format / PRL section changes. The section layout is unchanged; only which
  entries the bake writes changes. (If runtime gating turns out to need a
  per-section mode flag — see Open questions — that is a wire change and must be
  surfaced before proceeding, not folded in silently.)
- The fine-atlas trace, the lightmap-UV pre-pass, and the dominant-direction
  technique. Unchanged.
- The user's current static-spot screenshot (build/bake state, handled separately).
- Any SDF role in indirect visibility. DDGI owns indirect GI; SDF stays on direct
  static-occluder shadows.

## Acceptance criteria

Automated (host-side, deterministic — the bake is pure CPU data logic):

- [ ] In `Unshadowed` bake mode, a texel fully occluded from an animated light
      still emits a dominant direction pointing toward that light (not the
      world-up default). Verified by a bake-level test asserting the direction
      outcome, modeled on the existing static unshadowed test.
- [ ] In `Shadowed` bake mode, the animated bake still omits occluded
      (texel, light) pairs — existing behavior preserved.
- [ ] Static path behavior is unchanged: the existing static lightmap and
      direction tests stay green.
- [ ] Re-bakes are not served stale: the animated-bake stage version is bumped so
      prior cache entries invalidate.
- [ ] `cargo fmt` / `clippy` clean; full suite green. No new GPU/pixel-readback
      tests added.

Manual / visual (human running the engine — not machine-verified):

- [ ] In an `Unshadowed`-baked PRL with an animated light and a static occluder,
      the animated light casts a visible SDF shadow that tracks the occluder —
      the g-channel aggregate is no longer uniformly unshadowed where geometry
      occludes the light.

Documentation:

- [ ] `context/lib/sdf_shadows.md` exists, is routed in `index.md`, is marked
      EXPERIMENTAL / A/B-gated, and states (a) the direction-visibility ≡
      bake-mode invariant, (b) the DDGI boundary (SDF never does indirect
      visibility), and (c) the two direction failure modes.

## Tasks

### Task 1: Mode-gate the animated direction cull + contract test

Add the lightmap bake mode to the animated weight-map bake's inputs and gate its
visibility cull on `Shadowed`, mirroring the static bake. Bump the animated bake's
stage version so cached entries invalidate. Add the contract test asserting the
`Unshadowed`-mode direction outcome (and that `Shadowed` mode still culls).

### Task 2: Author the SDF shadows contract doc

Promote the sibling `sdf_shadows.md` (drafted alongside this plan) into
`context/lib/`, add its Agent Router entry in `context/lib/index.md`, and confirm
the data-flow diagram and invariants match the post-fix code. Decide placement
(see Open questions).

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — independent. Task 1 is code + test;
Task 2 is the context doc. The doc describes durable intent, not Task 1's specific
edits, so it does not depend on Task 1 landing first.

## Rough sketch

**The fix (Task 1).** Mirror the static gate. In the static bake the cull reads
`if mode == BakeMode::Shadowed && !shadow_visible(...) { continue }`; the animated
bake's loop currently reads `if !shadow_visible(...) { continue }` with no mode in
scope.

- `WeightMapInputs` gains a bake-mode field (reuse `lightmap_bake::BakeMode` — do
  not introduce a parallel enum). The caller at `main.rs` (where `WeightMapInputs`
  is built) already chose the mode it passed to the lightmap bake; pass the same
  value. Thread it through to the per-chunk loop that runs the cull.
- Gate the cull on `Shadowed`. In `Unshadowed` mode, retain the pair: this makes
  the animated irradiance (`lm_anim`) unshadowed and the direction point at the
  occluded light — symmetric with the static `lm_irr` path, where the runtime
  applies the animated SDF factor as the shadow.
- Bump the animated bake's `STAGE_VERSION` (the cache key folds it in alongside
  the input hash). Without this, a PRL re-baked after the fix can be served the
  old culled bake from cache.

**The test (Task 1).** Model on the existing static test
`unshadowed_bake_lights_occluded_texels`: a floor with a blocker between it and a
single light. Bake the animated weight maps in `Unshadowed` mode; assert the
occluded texel's light list is non-empty and its decoded direction points toward
the light (dot with the surface→light vector above a clear threshold), **not** the
world-up axis. Assert the same texel is culled (count 0) under `Shadowed` mode.
Assert the **outcome** (the emitted direction), never the code path — no
"`shadow_visible` was/wasn't called" assertions; an implementation-coupled test
proves nothing and breaks on harmless refactors.

**Test reach.** The bake is pure host logic — fully testable. The producer↔consumer
seam (bake direction format ↔ what the trace/compose expect, including the
world-up sentinel) is partially testable via the testing-guide seam-crossing
pattern. The GPU tail (atlas upload, trace, composite) and the end visual
("animated light casts a shadow") are not unit-testable and stay human-verified
per `testing_guide.md` §3/§5.

## Open questions

- **Runtime gating of the animated SDF factor.** The forward shader applies the
  animated factor only when its enable bit is set, and the static factor only when
  the static lightmap was baked unshadowed (else baking + factor would
  double-shadow). Confirm the animated enable bit is driven by the same global
  lightmap bake mode. If the animated atlas can be baked in a different mode than
  the static lightmap, the runtime needs to learn the animated mode — which would
  be a per-section flag and therefore a wire change. Current design appears to use
  one bake mode per PRL; verify before implementing, and surface if a section flag
  is actually required (do not add one silently).
- **Contract-doc placement.** Recommend standalone `context/lib/sdf_shadows.md`,
  loudly marked EXPERIMENTAL and promotable to canonical if the A/B is won. The
  alternative is folding it into `rendering_pipeline.md` §7, where the SDF shader
  header already points. Decision needed at promotion.
- **Experimental status.** SDF is A/B-gated against main's free baked shadows and
  may be reverted to the last pre-SDF commit. This fix is cheap and bake-side, so
  it is worth landing regardless — it removes a confound (animated lights silently
  unshadowed) that would otherwise corrupt the A/B's quality side.
