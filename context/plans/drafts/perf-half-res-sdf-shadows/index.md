# Half-Res SDF Shadow Pass

> **Status:** draft
> **Depends on:** sub-plan 8 (SDF atlas + sphere-traced shadows) — completed in-tree per master plan index; file remains under `in-progress/lighting-foundation/` until the full Lighting Foundation plan ships. No further bookkeeping required in this plan.
> **Related:** `context/plans/in-progress/lighting-foundation/8-sdf-shadows.md` (target of this optimization) · `context/plans/in-progress/lighting-foundation/5-shadow-maps.md` (CSM — shares the pre-forward shadow-pass concept and per-light slot pool) · `context/plans/in-progress/lighting-foundation/10-sh-leak-fix.md` (SH corner visibility — explicitly out of scope; different trace shape).

---

## Context

Sub-plan 8 ships SDF point/spot shadows as a fragment-shader sphere trace invoked from the forward-pass light loop at `postretro/src/shaders/forward.wgsl:1074` (`sample_sdf_shadow`, defined at line 364). The trace runs **per visible shadow-casting light, per fragment, at full viewport resolution**. Sub-plan 8 explicitly defers the low-res-plus-upscale optimization (see 8-sdf-shadows.md §"Low-res + upscale (deferred optimization)") and names the expected fix:

> Render the shadow contribution into a half-resolution render target, one channel per visible shadow-casting light (or a packed format). Bilaterally upsample (depth + normal-aware) back to full resolution before the main light loop uses it.

This plan executes that deferral. It adds a pre-forward half-resolution SDF shadow pass, produces one shadow-factor texture per visible shadow-casting light, and replaces the in-loop `sample_sdf_shadow` call with a sample from the upscaled result.

**Two distinct SDF-consuming call sites exist in the forward shader; this plan targets only one:**

1. **`sample_sdf_shadow` at `forward.wgsl:364`** — direct-light shadow trace. One trace per visible shadow-casting point/spot per fragment. **Target of this plan.**
2. **`sh_corner_visibility` at `forward.wgsl:484`** — SH indirect-visibility weighting from sub-plan 10. Eight traces per fragment (one per probe corner). **Out of scope.** It already has its own tuning knobs (`SH_VIS_MAX_STEPS = 12u` at line 465, fast-path skip at line 478) and its eight-corners-share-a-fragment shape rules out the per-light slot-pool approach this plan uses.

Cost comparison belongs in sub-plan 8's §"Low-res + upscale" and sub-plan 10's §"Fix A" — this plan does not invent new numbers.

---

## Goal

Cut the steady-state cost of `sample_sdf_shadow` by a factor of ~4 (quarter pixel count) without visibly degrading shadow quality on the test maps. Keep the existing `shadow_kind == 2` discriminant and `cast_shadows` FGD flag semantics intact; this is a pipeline change, not an authoring change.

`cargo test -p postretro` passes unchanged throughout.

---

## Approach

Four tasks, sequenced.

```
A (PRL-independent: pass plumbing + depth downsample) ── B (trace kernel) ── C (upscale + forward integration) ── D (fallback + tuning)
```

### Before / after pass order

Before (sub-plan 8 shipped state):

```
depth prepass (full-res)
  → CSM shadow passes (per cascade)
    → forward pass (full-res):
        for each visible light:
          sample_sdf_shadow()   ← per fragment, per light, full-res
```

After (this plan):

```
depth prepass (full-res)
  → half-res depth downsample
    → CSM shadow passes (unchanged)
      → SDF shadow pass (half-res):
          for each visible shadow-casting light:
            dispatch half-res kernel → R8 shadow texture (one per slot)
        → forward pass (full-res):
            for each visible light:
              sample shadow texture (joint-bilateral upscaled)
```

---

### Task A — Half-res depth + pass plumbing

**Crate:** `postretro` · **Files:** `postretro/src/render/mod.rs` (new pass wiring); new `postretro/src/shaders/depth_downsample.wgsl`; new `postretro/src/render/sdf_shadow_pass.rs` (module scaffold, kernel lands in Task B).

**Problem.** The trace kernel needs a world position per half-res pixel. The existing depth prepass (`DEPTH_PREPASS_SHADER_SOURCE` at `render/mod.rs:41`, pipeline at line 315, dispatched at line 1617) produces full-res depth only.

**Fix.** Add a half-res depth texture (half viewport width/height, rounded up) and a downsample compute pass that runs after the depth prepass. Picker is min-depth (keeps geometry edges conservative for shadow reconstruction) or checkerboard pick — the exact choice is an Open Question; min-depth is the default because an upscale that occasionally trusts the wrong surface is worse than one that trusts the nearer surface. No full-res texture changes; the downsample reads the existing depth target and writes a new half-res R32Float.

**Pass module.** Create `render/sdf_shadow_pass.rs` alongside `render/shadow_pass.rs` (the CSM module at `render/mod.rs:6`). Mirror its shape: a `SdfShadowResources` struct owning the per-slot half-res R8 targets, a bind group layout for the trace kernel (filled in Task B), and a `render_sdf_shadow_passes` entry point called from the main frame function between CSM and forward.

**Slot pool.** Allocate N half-res R8 shadow targets at startup, matching the CSM slot-sizing concept from sub-plan 5. Default N = 16 (representative upper bound on visible shadow-casting point/spot lights per frame; Open Question). At frame start, visible shadow-casting lights are assigned to slots; beyond N slots, spill policy is an Open Question (likely: prioritize by screen-space influence, unassigned lights fall back to unshadowed or to the full-res trace — see Task D).

---

### Task B — Half-res sphere-trace kernel

**Crate:** `postretro` · **Files:** new `postretro/src/shaders/sdf_shadow.wgsl`; `postretro/src/render/sdf_shadow_pass.rs` (pipeline + dispatch).

**Problem.** Move the body of `sample_sdf_shadow` (`forward.wgsl:364–403`) into a standalone compute shader that runs per half-res pixel.

**Fix.** Port the trace verbatim. Inputs per dispatch:
- Half-res depth texture (from Task A) — reconstruct world position via inverse view-projection.
- Light index (push constant or per-dispatch uniform) into the `GpuLight` buffer already uploaded by sub-plan 3.
- The SDF atlas bind group — the existing group assembled in `postretro/src/render/sdf.rs` (`sdf_bind_group_layout_entries` at line 513, bindings 5–9).

Output: R8Unorm shadow factor written to the slot's half-res target.

**Bind-group plan.** Group 2 is fully occupied (CSM at sub-plan 5 bindings 0–4; SDF at `render/sdf.rs:10–14` bindings 5–9). Sub-plan 10 already consumes these; there is no room to add the depth + half-res-output bindings there. This pass runs in its own pipeline with its own layout — the SDF atlas group is rebound as group 0 or 1 for the shadow kernel, and the new bindings (half-res depth in, R8Unorm shadow target out, per-light uniform) occupy a distinct group. No forward-pass binding shuffle is required.

**Output format.** Each per-slot shadow target is `R8Unorm` (one shadow factor in [0, 1]). In the forward shader the slot array is bound as a texture array in **group 2 binding 10** (or the next free slot if 10 is taken). Slot count can change without rebuilding the bind group layout — the array length is the only thing that changes.

**Dispatch.** One dispatch per visible shadow-casting light per frame, N dispatches total (bounded by the slot pool in Task A).

---

### Task C — Joint bilateral upscale + forward integration

**Crate:** `postretro` · **Files:** `postretro/src/shaders/forward.wgsl` (replace inline trace with texture sample); `postretro/src/shaders/sdf_shadow_upscale.wgsl` (new — inline helper module `include`d into `forward.wgsl`, or a compute pass if profiling shows the inline helper is slower).

**Problem.** The forward pass currently calls `sample_sdf_shadow(in.world_position, light.position_and_type.xyz, 0.02617994)` at `forward.wgsl:1074`. Replace with a joint-bilateral sample of the light's slot texture.

**Fix.** In the forward fragment shader's light loop, replace the `shadow_kind == 2u` branch body (`forward.wgsl:1067–1079`) with a joint-bilateral upscale helper that reads the per-light shadow texture, guided by full-res depth (already available in the fragment) and optionally the fragment normal. Reference formulation: "joint/cross bilateral upsample" — see Kopf et al. 2007 ("Joint Bilateral Upsampling") for the canonical formulation; À-Trous wavelet variants are a cheaper alternative if the 4-tap bilateral is insufficient. Exact tap count and weight formulation is an Open Question — default to a 4-tap bilinear-footprint joint-bilateral that rejects samples whose half-res depth deviates from the full-res depth by more than a per-frame threshold.

Light-to-slot mapping. The forward shader needs to know which slot holds each light's shadow. Options: (a) extend `GpuLight` with a `shadow_slot: u32` field (already has a `shadow_info` pair visible at `forward.wgsl:1063,1066` used for CSM index); (b) add a parallel `array<u32>` lookup keyed by light index. Preferred: reuse `shadow_info.y` since CSM reads it at line 1066 — the field is already typed as "shadow map index" for `shadow_kind == 1` and can serve symmetrically as "shadow slot index" for `shadow_kind == 2`.

**Full-res fallback encoding.** Encode the fallback path as `slot_index = 0xFFFF` (sentinel) in `shadow_info.y`. When the forward shader sees `shadow_info.y == 0xFFFFu`, it falls through to the inline `sample_sdf_shadow` trace. This avoids a separate flag bit in `.w` and prevents any `.y`/`.w` collision with future uses.

---

### Task D — Full-res fallback + tuning

**Crate:** `postretro` · **Files:** `postretro/src/shaders/forward.wgsl` (keep the original `sample_sdf_shadow` function callable as a fallback path).

**Problem.** Half-res + bilateral upsample loses thin features (handrails, grates, thin pillars one voxel thick). Sub-plan 8 explicitly names this risk; the retro aesthetic hides it more than a modern target would, but not all the time.

**Fix.** Per-light "trace full-res" override. **Trigger: automatic by influence radius.** Lights with `radius < 2 m` fall through to the inline `sample_sdf_shadow` call at full resolution. They cover too few pixels to justify a half-res slot, and the tight geometry around small lights is where thin-feature artifacts are most visible. No FGD-authored flag — keeps the FGD surface clean and avoids a modder-facing tuning knob. (2 m is the starting threshold; confirm before implementation — see User Decisions below.)

Keep `sample_sdf_shadow` in `forward.wgsl` as the fallback implementation; route `shadow_kind == 2` to the half-res texture sample *or* the inline trace based on `shadow_info.y == 0xFFFFu` (sentinel; see Task C).

Tuning pass on `assets/maps/occlusion-test.map` and the standard test map: walk slot count, bilateral threshold, downsample picker (min vs. checkerboard) with the `POSTRETRO_GPU_TIMING=1` pass breakdown (already supported per `CLAUDE.md`).

---

## Files to modify

| File | Task | Change |
|------|------|--------|
| `postretro/src/render/mod.rs` | A, B, C | Wire half-res depth downsample after depth prepass; instantiate `SdfShadowResources`; call `render_sdf_shadow_passes` between CSM and forward; no change to `SdfResources` at `render/sdf.rs`. |
| `postretro/src/shaders/depth_downsample.wgsl` | A | New compute shader: sample full-res depth, write half-res min-depth (or checkerboard). |
| `postretro/src/render/sdf_shadow_pass.rs` | A, B | New module. Owns half-res shadow target pool, bind group layouts, and dispatch entry point. Mirrors `render/shadow_pass.rs` shape. |
| `postretro/src/shaders/sdf_shadow.wgsl` | B | New compute shader. Body is `sample_sdf_shadow` ported verbatim from `forward.wgsl:364–403`, reading world-pos from reconstructed half-res depth. |
| `postretro/src/shaders/forward.wgsl` | C, D | Replace `shadow_kind == 2u` inline trace at lines 1067–1079 with a joint-bilateral upscale sample, indexed by `shadow_info.y`. Keep the original `sample_sdf_shadow` function body as the fallback for the full-res override. |
| `postretro/src/shaders/sdf_shadow_upscale.wgsl` | C | New helper (inlined into `forward.wgsl` via `include` or build-time concat). Joint bilateral upscale keyed on full-res depth. |
| `postretro/src/render/sdf.rs` | — | Unchanged. The trace kernel moves; the atlas access pattern does not. |

---

## Acceptance Criteria

1. `cargo test -p postretro` passes. No new `unsafe`.
2. `sample_sdf_shadow` no longer runs in the forward fragment shader's steady-state `shadow_kind == 2u` path; it is reached only via the explicit full-res fallback (Task D).
3. Per-frame GPU cost of SDF shadows, measured via `POSTRETRO_GPU_TIMING=1`, drops on the standard test map. **The PR description must include `[gpu-timing]` before/after numbers for at least one dense-shadow-caster test map.** No abstract "drops" claim — attach actual numbers.
4. Visual A/B on `assets/maps/occlusion-test.map` shows no new leaks through thin geometry that the full-res trace did not already exhibit (thin-geometry loss is the documented trade-off, addressed by Task D's fallback).
5. The full-res override fallback (Task D) is reachable by at least one trigger and produces identical output to the pre-optimization shader for lights that opt in.
6. Shadow slot pool is sized via a single constant in `sdf_shadow_pass.rs`; changing it does not require other edits.

---

## Out of scope

- **SH corner visibility trace (`sh_corner_visibility` at `forward.wgsl:484`, 8 per fragment).** Different optimization shape — the 8 corners share a fragment's depth/normal so a per-light slot pool doesn't apply. Sub-plan 10 already exposes `SH_VIS_MAX_STEPS` (line 465) and the fast-path skip at line 478 as its tuning surface.
- **SDF atlas representation changes.** Voxel size, brick size, quantization, and coarse-distance texture stay exactly as sub-plan 8 defines them. `postretro/src/render/sdf.rs` is not modified by this plan.
- **Dynamic SDF rebake.** Sub-plan 8's "moving geometry casts no SDF shadow until later milestones" note applies unchanged.
- **CSM changes.** Directional shadows continue through `sample_csm_shadow` (`forward.wgsl:408`). The half-res path is SDF-only.
- **New light types or FGD schema changes.** Task D uses an automatic radius threshold; no new FGD property is introduced.
- **Replacing the forward pass with deferred/clustered.** Pre-forward shadow pass is a targeted intervention, not a pipeline rewrite. Clustered forward+ remains the deferred follow-up called out in the parent plan §"In scope".

---

## Decided Questions

These were open questions; decisions are recorded here so the implementer does not re-open them.

1. **Slot pool size — 16.** Matches CSM pool conventions from sub-plan 5. Single constant in `sdf_shadow_pass.rs`; tune on measurement. (Confirm with user — see User Decisions below.)
2. **Spill policy — priority by influence radius × screen-coverage-proxy; skip beyond slot 16.** Lights beyond the pool go unshadowed. Same policy CSM uses; revisit if profiling shows the proxy is expensive.
3. **Upscale filter — cross-bilateral, depth rejection only (no normals first pass).** Reference: Kopf et al. 2007 "Joint Bilateral Upsampling." Add normal-aware rejection only if edge artifacts appear on test maps.
4. **Depth downsample picker — min of 4 neighbors** (farthest from camera, pessimistic for shadow). Conservative over-shadowing is visually quieter than under-shadowing.
5. **Full-res fallback trigger — automatic by influence radius, 2 m threshold.** No authored flag; no FGD surface change. (Confirm threshold with user — see User Decisions below.)
6. **Upscale location — inline in the forward fragment shader.** One bilateral per shadowed light in the light loop; no separate render-graph node. Add a dedicated pass only if profiling shows the inline path is the bottleneck.

---

## User Decisions

The following thresholds are the starting proposal; confirm before Task D lands:

- **Full-res fallback radius threshold:** 2 m. Lights with influence radius below this fall through to inline `sample_sdf_shadow`.
- **Slot pool cap:** 16 slots.

---

## Notes

- **CSM does not consume the half-res depth.** The downsample from Task A is SDF-only; CSM shadow passes are unchanged.

---
