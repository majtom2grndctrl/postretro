# Lighting — SH Baker Static Filter + Normal-Offset Re-Add

> **Status:** ready.
> **Depends on:** nothing — all predecessors shipped (`lighting-foundation/`, `lighting-dynamic-flag/`, `lighting-old-stack-retirement/`).
> **Related:** `context/plans/done/lighting-foundation/2-sh-baker.md` (baker being amended) · `context/plans/done/lighting-foundation/6-sh-volume.md` (runtime SH sampling being amended) · `context/plans/done/lighting-foundation/10-sh-leak-fix.md` (Fix D shipped and still live; Fix B shipped then reverted with SDF retirement; Fix A dropped — this plan reinstates Fix B only).

---

## Context

Two small SH corrections remain after the SDF retirement and the `is_dynamic` flag landing:

1. **Static-only filter in the SH baker.** `sh_bake.rs` currently partitions lights by `animation.is_none()` (`sh_bake.rs:94`). A dynamic light with no animation still folds into `static_lights` and bakes — which double-counts it, because the runtime direct loop also evaluates it. The lightmap baker already has the right pattern (`lightmap_bake.rs:119`: `inputs.lights.iter().filter(|l| !l.is_dynamic)`); the SH baker needs the same filter.

2. **Normal-offset SH sampling at runtime.** Sub-plan 10's Fix B was shipped in commit `4b32565`, then removed in `e8294ec` (SDF retirement) because its companion Fix A (SDF-weighted visibility) was also removed. The normal offset is independently useful — it alone reduces bleed across thin walls — and should return without Fix A. The current `sample_sh_indirect` in `forward.wgsl` carries a placeholder comment ("Wall-bleed mitigation is deferred…") that this plan replaces.

Sub-plan 10's Fix D (exterior-leaf probe invalidation) is already shipped and still live — this plan does not touch it.

---

## Goal

- **Compiler:** SH baker filters out `is_dynamic` lights before accumulation.
- **Runtime:** SH sample position is offset along the (normal-mapped) surface normal before the trilinear probe lookup.

No new PRL sections. No new bind groups. No format bump.

---

## Concurrent workstreams

Task A (compiler) and Task B (runtime) are independent.

```
Task A (compiler): is_dynamic filter ──── independent
Task B (runtime):  normal-offset sample ─ independent
```

---

## Task A — SH baker: static-only filter

**Crate:** `postretro-level-compiler` · **File:** `src/sh_bake.rs`.

Change the light-partition at `sh_bake.rs:94` so `static_lights` excludes `is_dynamic` lights in addition to the existing animation split. The lightmap baker's filter is the reference implementation:

```rust
// lightmap_bake.rs:119 (reference)
let static_lights: Vec<&MapLight> = inputs.lights.iter().filter(|l| !l.is_dynamic).collect();
```

For SH, preserve the existing static/animated split — a light must be both non-dynamic **and** non-animated to fold into `static_lights`; animated non-dynamic lights still go to `animated_lights`; dynamic lights are excluded from both.

`bake_only` lights continue to bake (they have no runtime direct contribution — the SH bake is their only contribution). Only `is_dynamic` is newly filtered.

### Task A acceptance gates

- New unit test in `sh_bake.rs` mirroring `lightmap_bake.rs`'s `is_dynamic_lights_skipped_by_bake`: baking a scene with one `is_dynamic` light produces an SH section whose probe coefficients match the no-light baseline within numerical noise.
- Existing SH baker tests continue to pass (the default `MapLight` has `is_dynamic: false`, so existing fixtures are unaffected).

---

## Task B — Runtime: normal-offset SH sampling

**Crate:** `postretro` · **File:** `src/shaders/forward.wgsl`.

In `sample_sh_indirect` (currently at `forward.wgsl:511`), offset the sample world-space position along the normal-mapped normal by a fraction of the probe grid spacing before computing the grid UV:

```wgsl
const SH_NORMAL_OFFSET_M: f32 = 0.1; // fraction of cell_size, tuned empirically
// ...
let offset_world = world_pos + normal * SH_NORMAL_OFFSET_M * sh_grid.cell_size;
let cell_coord = (offset_world - sh_grid.grid_origin) / max(sh_grid.cell_size, vec3<f32>(1.0e-6));
// ... existing clamp/floor/fract/sample
```

The offset should use the same normal the direct term uses (the normal-mapped `N` at the call site in `fs_main`, not the interpolated mesh normal), so surface detail participates.

Delete the placeholder "Wall-bleed mitigation is deferred…" comment block introduced by the SDF retirement; replace it with a short note that the offset biases the lookup toward the lit side.

The original sub-plan 10 value was 10% of cell size — reuse that as the starting constant. It was never tuned independently of the (now-removed) SDF visibility term, so verify visually before committing.

### Task B acceptance gates

- Two-room test (rooms separated by a thin wall, one lit, one dark): the dark room's interior wall face receives visibly less SH bleed than with the offset absent. Visual comparison via `LightingIsolation::IndirectOnly` (Alt+Shift+4) is sufficient.
- No visible seams or dark bands on open-room geometry compared to a pre-change screenshot.

---

## Acceptance Criteria (both tasks)

1. `cargo test --workspace` passes.
2. `cargo clippy --workspace -- -D warnings` clean.
3. No new `unsafe`.
4. Task A and Task B acceptance gates above.

---

## Out of scope

- SDF-weighted trilinear SH sampling (sub-plan 10 Fix A) — dropped with SDF retirement; not returning.
- Exterior-leaf probe invalidation (sub-plan 10 Fix D) — already shipped, already live.
- Animated SH layers — deprioritized.
- DDGI-style dynamic probe updates.
- Three-bounce indirect.
