# SDF Shadow Fine-Atlas Trace

> **SUBSUMED — do not resume.** Superseded by
> `context/plans/drafts/sdf-per-light-shadows/`. The fine-field sampler and coarse
> unit-bug fix this plan produced **landed and are carried forward** (committed as a
> checkpoint); only the dominant-direction trace structure they fed is replaced by
> per-light tracing. The fine field's role in the new design is documented in that
> plan's `architecture.md`. Retained in git history only; removed from the tree so
> agents don't resume a superseded trace.

## Goal

Make the half-res SDF static-occluder shadow trace sample the **fine brick atlas** (0.5 m voxels) near surfaces instead of marching the coarse per-brick field alone. The coarse field is per-brick (≈4 m granularity on `occlusion-test.prl`) and cannot resolve real occluders — pillars, doorways, anything smaller than a brick — so the ray marches straight through and the pass produces almost no shadows. The fine atlas is already baked, uploaded, and bound to the pass; the trace just doesn't read it. This is the named v1 follow-up (`sdf_shadow.wgsl` trace comment: "Refining to the fine atlas is the named follow-up").

**This spec is what makes the parent feature's A/B finally evaluable.** `sdf-static-occluder-shadows` is an experimental branch whose promotion to `main` is gated on a perf/quality A/B against `main`'s free baked shadows. Its single biggest open question was the static-occluder SDF *cost* gate — the two aggregate traces must hold framerate on the 2020 MBP. Until now the trace read only the coarse field, so it produced almost no shadows **and** skipped the expensive dependent fine-atlas fetches — meaning the A/B was never actually evaluable: neither side of the trade (quality vs cost) was real. Turning on fine fetches lights up exactly the per-fragment dependent-fetch cost class that got SDF removed originally. So the `≤16.6 ms` criterion below is **the parent's deferred make-or-break perf gate firing for the first time** — not a routine perf check. If it fails past the quality-degrading rungs of the retreat ladder, the parent feature loses the A/B (see *Open questions*).

**Hard prerequisite — v2 lightmap-UV pre-pass.** `sdf-shadow-lightmap-uv-prepass` (in-progress) feeds this trace a surface-locked, per-texel dominant direction — the correct ray to march. Without it the trace has no correct origin/direction to evaluate, so this spec **cannot be meaningfully implemented or evaluated until v2 merges.** This is a blocking dependency, not a soft ordering preference.

## Scope

### In scope

- A fine-atlas distance sampler in `sdf_shadow.wgsl` that resolves a world point to its brick via the top-level indirection, and:
  - **Surface brick** (slot is a real index) → sample the fine atlas, decode to metric signed distance.
  - **Empty brick** (`SDF_TOP_LEVEL_EMPTY`) → fall back to the coarse field for a large empty-space step.
  - **Interior brick** (`SDF_TOP_LEVEL_INTERIOR`) → inside solid; return a negative/zero distance so the march registers a hit.
  - **Out-of-bounds** → large positive ("far open"), same as the current coarse sampler.
- Rewiring `trace_shadow` to step on this combined fine+coarse distance instead of coarse-only. Keep the existing sphere-trace loop shape, the closest-passing-distance penumbra estimate, the self-shadow start bias, the bounded march length, and the open-space skip early-out.
- Retuning the runtime defaults (`max_march_steps`, `penumbra_k`, `open_space_skip_threshold`) for the fine field, since per-voxel distances change step sizes and penumbra width. Knobs stay runtime-settable and exposed in the debug UI.
- A perf-retreat ladder (see *Open questions*) if the fine sampling breaks the 60 fps budget on the 2020 MBP target.
- Direction sanity-check (lightweight, see *Acceptance criteria*): confirm the baked dominant direction at shadowed surfaces points toward the occluded light, not the `Vec3::Y` up-default — so the fine trace has a correct ray to march.

### Out of scope (non-goals)

- **New GPU bindings or resources.** The fine atlas (`sdf_atlas`), coarse field (`sdf_coarse`), top-level indirection (`sdf_top_level`), and meta are already bound to the pass (group 0). No Rust resource, bind-group, or pipeline-layout changes. (Editing the default-constant values and adding the source-string test in `render/sdf_shadow.rs` are expected Rust edits, not bind-group/layout changes — they are in scope.)
- **Wire format / bake changes.** No PRL section change, no re-bake required. The fine atlas is already in every `--bake-sdf` PRL.
- **Reverting or altering the v2 lightmap-UV gbuffer** (`sdf-shadow-lightmap-uv-prepass`). The per-texel direction sampling stays exactly as v2 left it. Note v2 is a **hard prerequisite** for this spec (see *Goal*), not just an untouched neighbor: its surface-locked per-texel direction is the ray this trace marches.
- **Changing the dominant-direction technique** (single baked luminance-weighted direction per texel). Multi-light or per-light direct tracing is the removed `sdf-shadows` approach — out of scope by design and by cost.
- **Full-res shadow pass.** Stays half-res; only the per-step distance source changes.
- **The animated-baked direction path quality** beyond what the same trace change incidentally improves (animated trace uses the same `trace_shadow`).

## Acceptance criteria

This feature's correctness is verified by human visual inspection **by design** — it is a renderer/visual change, and the project has no agentic game-control harness and no shadow-factor pixel-readback harness. The automated ACs are therefore limited to cheap, infra-free, deterministic checks (source-string regression guards, shader-parse, decode-constant relationships, GPU timing). The visual ACs are the real correctness proof, not a fallback. Do not add automated tests that would need game-control or pixel-readback infrastructure.

Automated (cheap, infra-free, deterministic):

- [ ] **Fine-path wiring guard (regression guard, not a correctness proof).** `sdf_shadow.wgsl` no longer reaches `trace_shadow` step distances solely from `sample_coarse_distance`; a fine-atlas sampler that reads `sdf_atlas` + `sdf_top_level` is present and consumed by the march. Asserted by source-string check in the style of the existing `render/sdf_shadow.rs` tests (sampler function present; `trace_shadow` calls it; `sdf_atlas` is read). It passes even if the index math is wrong and the trace returns garbage — it only confirms the fine path is wired in and stays wired. No mirrored-arithmetic unit test is added: it would re-encode the same index math and prove nothing. Feature correctness is proven by the visual ACs below.
- [ ] **Coarse-unit fix guard.** The baker's existing structural test (`sdf_bake.rs::wall_straddling_brick_has_zero_distance_at_surface`) already asserts the coarse value's metric relationship — `coarse_at.abs() < voxel_size_m * brick_size_voxels` — confirming `coarse_signed` is in meters. That host-side relationship is the cheap deterministic check behind the "drop the `* brick_world_size`" coarse-unit fix (see *Rough sketch*); it stays green. No new test harness is added.
- [ ] **Shader parses.** The shader compiles (naga parse) in the existing shader-parse test.
- [ ] **Perf budget holds.** Measured shadow-pass + depth-pre-pass time via `POSTRETRO_GPU_TIMING=1` on `occlusion-test.prl` and `campaign-test.prl` does not push frame time over 16.6 ms on the 2020 MBP target adapter. (Measured on an adapter with `TIMESTAMP_QUERY`; if it exceeds, the perf-retreat ladder applies.)
- [ ] **No regression.** Non-SDF passes and v2 lightmap-UV sampling unaffected: existing test suite green, `cargo fmt`/`clippy` clean.

Manual / visual (observed by a human running the engine — not machine-verified):

- [ ] On `content/dev/maps/occlusion-test.prl` in `SdfShadowMode::On` with `Lighting Isolation = Normal`, pillars cast visible shadows onto the floor and onto each other where geometry occludes the dominant light — the central failing case today.
- [ ] In `SdfShadowMode::Visualize`, the static-aggregate factor shows graded shadow regions attached to occluder geometry (not near-uniform white), and the shadows stay surface-locked under camera motion (the v2 win is preserved).
- [ ] Shadows resolve at occluder scale (a ~1–2 m pillar produces a shadow, not nothing) — confirming the fine field is resolving sub-brick geometry.
- [ ] No new self-shadow acne or banding on lit surfaces from the fine sampling (the start bias still suppresses surface self-intersection).
- [ ] Direction sanity: shadowed surfaces darken on the side away from the dominant light (confirms baked directions point at occluders, not uniformly up).

## Tasks

### Task 1: Fine-atlas distance sampler + trace rewire

Add a `sample_fine_distance(world) -> f32` (metric signed distance, meters) to `sdf_shadow.wgsl`, mirroring the brick-resolution structure of the old `sdf-shadows`-tag `sample_sdf` (see *Rough sketch*) but adapted for the integer atlas. Resolve the world point to a brick cell, read `sdf_top_level`, branch on empty/interior/surface, and for surface bricks read the fine voxel(s) from `sdf_atlas` and decode. Rewire `trace_shadow` to step on this distance. Add the required bounds guards before indexing `sdf_top_level` and the atlas. Apply the mandatory coarse-unit fix — drop the `* brick_world_size` multiply in `sample_coarse_distance` (see *Rough sketch*); the empty-brick fallback this sampler reuses depends on it. Retune the three runtime knob defaults for the fine field.

This is the whole feature; a single sequential task. The plan stays one task because the change is confined to one shader function plus default constants, with no plumbing across modules.

## Sequencing

Single task — no concurrency. Verification (perf measurement, visual checks, direction sanity) follows implementation in the same task.

## Rough sketch

**The trace today** (`sdf_shadow.wgsl::trace_shadow`, ~line 195) sphere-traces on `sample_coarse_distance` only, which returns `max(coarse, 0.0) * brick_world_size` — a 4 m-granular lower bound. Replace the per-step distance with a fine+coarse combined sampler.

**Fine sampler structure** (blueprint: `git show sdf-shadows:postretro/src/shaders/forward.wgsl`, `sample_sdf`):

```wgsl
// Proposed design — remove after implementation.
fn sample_fine_distance(world: vec3<f32>) -> f32 {
    // bounds → large positive sentinel ("far open") — the same `1.0e4` literal sample_coarse_distance returns today (no named const exists).
    // brick_coord = floor((world - world_min) / brick_world_size); guard 0..grid_dims.
    // flat = bz*gridX*gridY + by*gridX + bx;   // z-major; matches sdf_top_level layout
    // slot = sdf_top_level[flat];
    //   slot == SDF_TOP_LEVEL_EMPTY    → coarse-based positive step (reuse sample_coarse_distance)
    //   slot == SDF_TOP_LEVEL_INTERIOR → negative (inside solid) → march registers a hit
    //   else (surface brick)           → sample fine atlas voxel(s), decode
    // brick atlas coord: bxa=slot%ax; bya=(slot/ax)%ay; bza=slot/(ax*ay)   // ax,ay = atlas_bricks_per_axis
    //   NB: mirror the baker's canonical packing order (pack_atlas_dimensions in the SDF bake);
    //   do not trust this restated formula — de-pack must match how the baker packed.
    // voxel within brick from local position → texel = brick_atlas*brick_size + voxel
    //   HIGHEST-RISK index math: off-by-one here yields NO shadows (see prose below).
    // decode: f32(textureLoad(sdf_atlas, texel, 0).r) * (voxel_size_m / SDF_I16_QUANT_STEPS_PER_VOXEL)
}
```

**Local-position → voxel → texel is the highest-risk math.** The brick de-pack and the per-voxel index are where an off-by-one or a swapped axis silently produces **no shadows** — indistinguishable from "the feature didn't work" rather than failing loudly. Two guards: (a) De-pack the slot to its atlas brick coordinate by **mirroring the baker's canonical packing order** (`pack_atlas_dimensions` in the SDF bake) — read that order from source, do not trust the formula restated in the sketch. (b) Map the world point's brick-local position to a voxel index by scaling the local offset (`world − brick_origin`) by the voxel grid resolution and flooring, clamp the voxel index to the brick's valid range, then offset by the brick's atlas base to get the texel. Apply the half-texel clamp (next note) so the sample stays inside the brick. Describe the mapping precisely from the meta dimensions — do not invent constants.

**Integer-atlas divergence from the blueprint.** The old sampler used hardware `textureSample` (trilinear) on a float atlas. The current atlas is `texture_3d<i32>` — hardware filtering is unavailable on integer textures. Decode via `textureLoad`. **Nearest voxel** (one `textureLoad`, half-texel-clamped within the brick) is the **intended default**, not merely the cheapest option: it is also the cheapest, but the parent's hard, pixelated retro shadow aesthetic licenses voxel-scale coarseness, so nearest reads as style rather than as a defect. Manual trilinear (8 `textureLoad`s + decode, then lerp) is an **unlikely** upgrade — pursue it only if voxel banding reads as a bug rather than as the retro look, and only if the perf budget allows, since it multiplies per-step fetch cost. The correctness win (resolving pillars) does not depend on trilinear smoothing.

**Decode constants** (all already in the shader/meta): step = `voxel_size_m / 256` per i16 (`SDF_I16_QUANT_STEPS_PER_VOXEL = 256.0`); stored value is signed (negative inside solids). Brick packing has **no apron** — clamp the voxel coordinate to `[0.5, brick_size − 0.5]` within each brick to avoid bleeding into neighbors.

**Coarse-unit fix (mandatory — the shader is the bug, a missed-shadow correctness risk).** Resolved in source: the **shader** over-scales, not the baker. The baked `coarse_signed` (`crates/level-compiler/src/sdf_bake.rs:200`) is already a **metric signed distance in meters** — the mean of per-voxel `±nearest_triangle_distance`. The baker's own structural test confirms this (`sdf_bake.rs:723`): it asserts the coarse value against `voxel_size_m * brick_size_voxels` — the brick edge length, in meters. The shader's `sample_coarse_distance` (`crates/postretro/src/shaders/sdf_shadow.wgsl:164`) returns `max(coarse, 0.0) * brick_world_size`, re-scaling an already-metric value by the ~4 m brick edge — over-stepping the empty-brick fallback by that factor. Its own comment (`sdf_shadow.wgsl:162–163`) calls the value "a metric distance lower bound," contradicting the multiply.

An over-stepping empty-brick step is **not** a step-efficiency nit: the sphere-trace can step from before a thin pillar's surface brick to past it, **tunneling through the exact sub-brick occluders this feature exists to catch** — a missed shadow that looks identical to "the feature didn't work."

**The fix:** drop the `* brick_world_size` multiply in `sample_coarse_distance`; return `max(coarse, 0.0)` directly (already meters).

**One nuance to record:** the baked value is the brick *mean* signed distance, not a conservative minimum. That is acceptable for the empty-brick fallback — those bricks are classified outside the surface band, so a mean-based step does not risk under-counting distance into a surface brick. Do not mistake the mean for a tight lower bound.

**Retuning — seed and stop-bar.** With true per-voxel distances, sphere-trace steps shrink near surfaces and the `penumbra_k * d / t` estimate sharpens. Re-evaluate `max_march_steps` (more small steps may be needed to cross open space to a distant occluder), `penumbra_k`, and `open_space_skip_threshold`. **Seed** from the current `render/sdf_shadow.rs` defaults — `DEFAULT_MAX_MARCH_STEPS = 48`, `DEFAULT_OPEN_SPACE_SKIP_THRESHOLD = 2.5`, `DEFAULT_PENUMBRA_K = 16.0` — do not start from scratch. **Stop-bar:** tune only until the visual acceptance criteria pass *and* the perf AC holds, then hand off. This is bounded retuning, not open-ended tuning — once both bars are met, stop.

## Open questions

- **Perf-retreat ladder — terminates in a fail-floor.** If the `≤16.6 ms` budget fails on the 2020 MBP, descend in priority order. The first rung is free of quality cost; past it the rungs trade shadow quality away, back toward coarse. (1) nearest-voxel fine sampling only (no trilinear) — already the proposed baseline, no quality loss. (2) cap fine sampling to the first N march steps near the origin and fall back to coarse beyond — most occlusion is near the receiver; this **starts degrading quality** (distant occluders revert to coarse, i.e. no shadow). (3) reduce `max_march_steps` — degrades further (shorter reach, more missed occluders). (4) restrict the fine path to surface bricks within a bounded world radius of the march origin — degrades further still. **The fail-floor:** if holding budget requires climbing past roughly rung (2) — into the quality-degrading rungs — the honest outcome is that the feature **fails the A/B**: the shadows have degraded back toward coarse, and at that point `main`'s free baked shadows win on both cost and quality. The named retreat is then the parent's documented fallback — **keep static lights baked-shadowed on `main`** — not indefinite optimization of a losing trade. None of rungs (2)–(4) are designed; they exist to bound how far it is worth pushing before declaring the A/B lost.
- **Nearest vs trilinear** as the shipped default. **Nearest is the intended aesthetic default**, not merely the cheap fallback: the parent leans on a hard, pixelated retro shadow aesthetic that licenses coarseness, and nearest-voxel sampling reads as that aesthetic. Trilinear is **unlikely** — pursue it only if voxel banding reads as a bug rather than as style, and only if budget allows. Confirm during the visual checks.
- **Direction adequacy — two distinct failure modes, do not conflate.** (1) A shadowed texel lit by overlapping static lights points toward their luminance-weighted **mean** direction and casts one shadow that way. This is the parent's *documented, accepted approximation* (`sdf-static-occluder-shadows`: single mean-direction shadow for overlapping static lights), now visible for the first time because shadows finally resolve. It is **expected by design — not a regression** to fix here. (2) A shadowed texel points to the `Vec3::Y` up-default instead of at any occluded light. This is a **different failure** — a degenerate or missing baked direction, an actual bug in the bake. Fix (2) in the dominant-direction bake; (2) is out of scope here, filed separately if the visual check observes it. Keep the two cases separate: (1) is the approximation working as specified, (2) is a bug.
