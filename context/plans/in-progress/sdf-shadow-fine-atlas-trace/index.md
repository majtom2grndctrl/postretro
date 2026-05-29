# SDF Shadow Fine-Atlas Trace

## Goal

Make the half-res SDF static-occluder shadow trace sample the **fine brick atlas** (0.5 m voxels) near surfaces instead of marching the coarse per-brick field alone. The coarse field is per-brick (≈4 m granularity on `occlusion-test.prl`) and cannot resolve real occluders — pillars, doorways, anything smaller than a brick — so the ray marches straight through and the pass produces almost no shadows. The fine atlas is already baked, uploaded, and bound to the pass; the trace just doesn't read it. This is the named v1 follow-up (`sdf_shadow.wgsl` trace comment: "Refining to the fine atlas is the named follow-up").

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

- **New GPU bindings or resources.** The fine atlas (`sdf_atlas`), coarse field (`sdf_coarse`), top-level indirection (`sdf_top_level`), and meta are already bound to the pass (group 0). No Rust resource, bind-group, or pipeline-layout changes.
- **Wire format / bake changes.** No PRL section change, no re-bake required. The fine atlas is already in every `--bake-sdf` PRL.
- **Reverting or altering the v2 lightmap-UV gbuffer** (`sdf-shadow-lightmap-uv-prepass`). The per-texel direction sampling stays exactly as v2 left it.
- **Changing the dominant-direction technique** (single baked luminance-weighted direction per texel). Multi-light or per-light direct tracing is the removed `sdf-shadows` approach — out of scope by design and by cost.
- **Full-res shadow pass.** Stays half-res; only the per-step distance source changes.
- **The animated-baked direction path quality** beyond what the same trace change incidentally improves (animated trace uses the same `trace_shadow`).

## Acceptance criteria

Automated (test- or tooling-gated):

- [ ] `sdf_shadow.wgsl` no longer reaches `trace_shadow` step distances solely from `sample_coarse_distance`; a fine-atlas sampler that reads `sdf_atlas` + `sdf_top_level` is present and consumed by the march. Asserted by source-string check in the style of the existing `render/sdf_shadow.rs` tests (sampler function present; `trace_shadow` calls it; `sdf_atlas` is read).
- [ ] The shader compiles (naga parse) in the existing shader-parse test.
- [ ] Shadow-pass GPU time stays within the 2020 MBP 60 fps vsync budget: measured shadow-pass + depth-pre-pass time via `POSTRETRO_GPU_TIMING=1` on `occlusion-test.prl` and `campaign-test.prl` does not push frame time over 16.6 ms on the target adapter. (Measured on an adapter with `TIMESTAMP_QUERY`; if it exceeds, the perf-retreat ladder applies.)
- [ ] No regression in the non-SDF passes or the v2 lightmap-UV sampling (existing test suite green; `cargo fmt`/`clippy` clean).

Manual / visual (observed by a human running the engine — not machine-verified):

- [ ] On `content/dev/maps/occlusion-test.prl` in `SdfShadowMode::On` with `Lighting Isolation = Normal`, pillars cast visible shadows onto the floor and onto each other where geometry occludes the dominant light — the central failing case today.
- [ ] In `SdfShadowMode::Visualize`, the static-aggregate factor shows graded shadow regions attached to occluder geometry (not near-uniform white), and the shadows stay surface-locked under camera motion (the v2 win is preserved).
- [ ] Shadows resolve at occluder scale (a ~1–2 m pillar produces a shadow, not nothing) — confirming the fine field is resolving sub-brick geometry.
- [ ] No new self-shadow acne or banding on lit surfaces from the fine sampling (the start bias still suppresses surface self-intersection).
- [ ] Direction sanity: shadowed surfaces darken on the side away from the dominant light (confirms baked directions point at occluders, not uniformly up).

## Tasks

### Task 1: Fine-atlas distance sampler + trace rewire

Add a `sample_fine_distance(world) -> f32` (metric signed distance, meters) to `sdf_shadow.wgsl`, mirroring the brick-resolution structure of the old `sdf-shadows`-tag `sample_sdf` (see *Rough sketch*) but adapted for the integer atlas. Resolve the world point to a brick cell, read `sdf_top_level`, branch on empty/interior/surface, and for surface bricks read the fine voxel(s) from `sdf_atlas` and decode. Rewire `trace_shadow` to step on this distance. Add the required bounds guards before indexing `sdf_top_level` and the atlas. Retune the three runtime knob defaults for the fine field.

This is the whole feature; a single sequential task. The plan stays one task because the change is confined to one shader function plus default constants, with no plumbing across modules.

## Sequencing

Single task — no concurrency. Verification (perf measurement, visual checks, direction sanity) follows implementation in the same task.

## Rough sketch

**The trace today** (`sdf_shadow.wgsl::trace_shadow`, ~line 195) sphere-traces on `sample_coarse_distance` only, which returns `max(coarse, 0.0) * brick_world_size` — a 4 m-granular lower bound. Replace the per-step distance with a fine+coarse combined sampler.

**Fine sampler structure** (blueprint: `git show sdf-shadows:postretro/src/shaders/forward.wgsl`, `sample_sdf`):

```wgsl
// Proposed design — remove after implementation.
fn sample_fine_distance(world: vec3<f32>) -> f32 {
    // bounds → SDF_LARGE_POS (far open), as sample_coarse_distance does today.
    // brick_coord = floor((world - world_min) / brick_world_size); guard 0..grid_dims.
    // flat = bz*gridX*gridY + by*gridX + bx;   // z-major; matches sdf_top_level layout
    // slot = sdf_top_level[flat];
    //   slot == SDF_TOP_LEVEL_EMPTY    → coarse-based positive step (reuse sample_coarse_distance)
    //   slot == SDF_TOP_LEVEL_INTERIOR → negative (inside solid) → march registers a hit
    //   else (surface brick)           → sample fine atlas voxel(s), decode
    // brick atlas coord: bxa=slot%ax; bya=(slot/ax)%ay; bza=slot/(ax*ay)   // ax,ay = atlas_bricks_per_axis
    // voxel within brick from local position; texel = brick_atlas*brick_size + voxel
    // decode: f32(textureLoad(sdf_atlas, texel, 0).r) * (voxel_size_m / SDF_I16_QUANT_STEPS_PER_VOXEL)
}
```

**Integer-atlas divergence from the blueprint.** The old sampler used hardware `textureSample` (trilinear) on a float atlas. The current atlas is `texture_3d<i32>` — hardware filtering is unavailable on integer textures. Decode via `textureLoad`. Baseline: **nearest voxel** (one `textureLoad`, half-texel-clamped within the brick) — cheapest, and sufficient to resolve occluders at voxel scale. If the penumbra estimate looks stepped/blocky, manual trilinear (8 `textureLoad`s + decode, then lerp) is the quality upgrade — but it multiplies per-step fetch cost, so gate it behind the perf budget. Start nearest; the correctness win (resolving pillars) does not depend on trilinear smoothing.

**Decode constants** (all already in the shader/meta): step = `voxel_size_m / 256` per i16 (`SDF_I16_QUANT_STEPS_PER_VOXEL = 256.0`); stored value is signed (negative inside solids). Brick packing has **no apron** — clamp the voxel coordinate to `[0.5, brick_size − 0.5]` within each brick to avoid bleeding into neighbors.

**Coarse-unit check.** `sample_coarse_distance` returns `max(coarse, 0.0) * brick_world_size`, treating the stored coarse value as a normalized per-brick lower bound; the baker writes `coarse_signed = mean(per-voxel signed distance)`. Confirm the coarse value's units (meters vs voxel-normalized) when reusing it for the empty-brick step distance, so empty-space steps are correctly scaled. This only affects open-space step size, not occluder resolution.

**Retuning.** With true per-voxel distances, sphere-trace steps shrink near surfaces and the `penumbra_k * d / t` estimate sharpens. Re-evaluate `max_march_steps` (more small steps may be needed to cross open space to a distant occluder), `penumbra_k`, and `open_space_skip_threshold` defaults (`render/sdf_shadow.rs`).

## Open questions

- **Perf-retreat ladder** if the `≤16.6 ms` budget fails on the 2020 MBP, in priority order: (1) nearest-voxel fine sampling only (no trilinear); (2) cap fine sampling to the first N march steps near the origin and fall back to coarse beyond — most occlusion is near the receiver; (3) reduce `max_march_steps`; (4) restrict the fine path to surface bricks within a bounded world radius of the march origin. None designed yet; (1) is already the proposed baseline.
- **Nearest vs trilinear** as the shipped default — decide during implementation against the perf budget and the visual banding check. Nearest is the baseline; trilinear is the named upgrade if banding is objectionable and budget allows.
- **Direction adequacy.** If the visual direction-sanity check fails (shadowed texels point up via the `Vec3::Y` default rather than at the occluded light), the fine trace alone won't fully fix coverage and a follow-up on the dominant-direction bake is needed — out of scope here, filed separately if observed.
