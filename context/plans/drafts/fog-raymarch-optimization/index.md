# Fog Raymarch Optimization

## Goal

Cut fog raymarch frame time from the 18 ms+ spike observed when the camera faces a fog volume to a stable sub-3 ms cost on the same scene. Achieve this by clipping each ray to the union of fog-volume AABB intervals before the march loop and by removing per-step constant arithmetic from the inner volume sampler.

## Background

`fog_volume.wgsl::cs_main` marches up to 256 fixed-size steps from `near_clip` to the depth-buffer hit distance. Two issues compound at high volume counts and oblique camera angles:

1. **No early-exit when the ray leaves all volumes.** The break conditions are `t >= ray.max_t` and `transmittance < 0.01`. A ray that grazes a volume keeps stepping (and re-running N AABB tests per step) through empty space until `max_t`. Rays that miss every volume burn the full step budget too.
2. **Per-step constant recomputation.** `sample_fog_volumes` rebuilds `half_ext`, `center`, and `half_diag` for every volume on every step. With ~130 k rays × up to 256 steps × N volumes, the redundant `length()` and arithmetic is millions of wasted ops per frame.

Both problems read the same data — AABB extents and derived constants — so they share a single spec. The precomputed fields land first; the slab-clip prologue then consumes them.

## Scope

**In scope.**
- Extend the GPU `FogVolume` struct with precomputed `center`, `inv_half_ext`, `half_diag`.
- Populate the new fields once per volume per level in the runtime bridge that builds the GPU records.
- Refactor `sample_fog_volumes` to read the precomputed fields instead of recomputing.
- Add a slab-clip prologue in `cs_main` that produces a per-ray union of `[t_enter, t_exit]` intervals over active fog volumes. Skip the march loop entirely when the union is empty; otherwise march only inside the union.

**Out of scope (future work — revisit only if profiling shows the slab-clip and field cache fall short).**
- Logarithmic / adaptive step size.
- Per-tile (workgroup-shared) volume masking or volume BVHs.
- Moving fog-volume packing into the level compiler PRL section. Today's GPU records are built per-frame in the bridge from the durable AABB side-table; that boundary is fine for this spec.
- Reordering/sorting the volumes buffer for spatial locality.

## Acceptance criteria

- On a scene that previously spiked the fog pass to 18 ms+ when looking into a fog volume, the fog raymarch GPU pass time stays at or below 3 ms steady-state with the same volume count and pixel scale. Measured via `POSTRETRO_GPU_TIMING=1` once the pass is added to the timed set, or via existing pass-timing instrumentation if the fog pass is already covered.
- Rays whose union interval is empty perform zero march iterations. Verified by an inserted debug counter (compile-time toggle, not shipped) or by inspection of the disassembly / iteration profile during bring-up; not a runtime invariant.
- Visual output of the fog pass is unchanged at default settings on existing test maps. No new banding, no halos at volume boundaries, no flicker on camera motion. Compared against a reference capture taken before the change.
- `FogVolume` GPU struct stays at a 16-byte-aligned size that satisfies the WGSL `vec3` alignment rules and the existing CPU/GPU layout assert. Exact stride is an implementation detail; the assert in the format crate is updated to whatever the new size is and remains the contract.
- All existing fog tests pass, including the byte-layout round-trip in `crates/postretro/src/fx/fog_volume.rs` (updated to cover any new fields).

## Tasks

### Task 1 — Precomputed fog volume fields

Extend the runtime GPU record and populate the new fields once per level. No shader behavior change yet; the new fields are written but unread at the end of this task.

**Touches.**
- `crates/postretro/src/fx/fog_volume.rs` — add `center: [f32; 3]`, `inv_half_ext: [f32; 3]`, `half_diag: f32` (plus padding as needed) to `FogVolume`. Update the `FOG_VOLUME_SIZE` assert to the new size. Update `pack_fog_volumes_round_trips_density_and_falloff` (and add new field coverage) so layout drift remains caught.
- `crates/postretro/src/shaders/fog_volume.wgsl` — mirror the new fields on the WGSL `FogVolume` struct so the layouts agree. The shader does not yet read them.
- `crates/postretro/src/scripting/systems/fog_volume_bridge.rs` — extend `FogVolumeAabb` with the precomputed values; compute them once in `populate_from_level` from the level-loaded `min`/`max`. Have `update_volumes` copy them into the `FogVolume` record it builds each frame.
- `crates/postretro/src/render/fog_pass.rs` — no logic changes; rebuilds automatically because `FOG_VOLUME_SIZE` drives the buffer size.

**Sketch.**

```
// Proposed: precompute alongside the existing AABB fields.
let half_ext = (max - min) * 0.5;
let center   = (min + max) * 0.5;
// Clamp the divisor away from zero so a degenerate (zero-thickness) brush
// can't produce inf/NaN in the shader.
let inv_half = vec3::ONE / half_ext.max(vec3::splat(1e-6));
let half_diag = half_ext.length();
```

Out of scope here: changing the level-format `FogVolumeRecord`. The PRL on-disk layout stays as-is; precomputed fields are a runtime-only concern.

### Task 2 — Slab-clip prologue and constant-free inner sampler

Consumes the new fields from Task 1.

**Touches.**
- `crates/postretro/src/shaders/fog_volume.wgsl`:
  - `sample_fog_volumes` — replace `(v.max_v - v.min) * 0.5`, `(v.min + v.max_v) * 0.5`, and `length(half_ext)` with reads of `v.center`, `v.inv_half_ext`, `v.half_diag`. Replace `abs(pos - center) / max(half_ext, 1e-6)` with `abs(pos - v.center) * v.inv_half_ext`.
  - `cs_main` — before the march loop, run a slab-clip prologue that builds the ray's active interval union over `fog_volumes[0..fog.volume_count]`. Skip the loop when the union is empty; otherwise advance `t` across gaps between sub-intervals so the loop only steps through active fog regions.

**Sketch.**

```
// Proposed: per-ray active-interval union, built before the march loop.
//
// For each volume, compute slab-test enter/exit:
//   inv_d  = 1.0 / ray.direction        (component-wise; guard zeros)
//   t_min  = (v.min   - ray.origin) * inv_d
//   t_max  = (v.max_v - ray.origin) * inv_d
//   t_near = max3(min(t_min, t_max))
//   t_far  = min3(max(t_min, t_max))
//   if t_near < t_far && t_far > 0.0 { record [max(t_near, start_t), min(t_far, ray.max_t)] }
//
// Build the union. With MAX_FOG_VOLUMES = 16 and most maps using a handful,
// a fixed-size array of intervals + insertion-sort merge is fine — no heap,
// no branching mess. Cap union slots at MAX_FOG_VOLUMES; if the cap is hit,
// fall back to a single [min t_enter, max t_exit] envelope (correct, just
// less tight).
//
// March: iterate union sub-intervals; for each, set t = sub.enter and step
// while t < sub.exit && transmittance >= 0.01 && step_count < max_steps.
// max_steps remains the global cap so a pathological case can't hang.
```

The `sample_fog_volumes` AABB membership test stays — the slab-clip narrows where the loop runs but a step inside the union envelope can still fall outside an individual volume's box, and the existing per-volume fade math depends on the membership branch.

## Sequencing

Sequential. Task 2 reads fields written by Task 1, so Task 1 must merge first. Within each task, the changes are tightly coupled (struct + WGSL mirror + bridge populate; or shader sampler + prologue) and land together.

## Plumbing rule

Every new struct field has exactly one write site and one read site:

| Field | Written | Read |
|-------|---------|------|
| `FogVolume.center` | `FogVolumeBridge::update_volumes` (computed once in `populate_from_level`, cached on `FogVolumeAabb`) | `fog_volume.wgsl::sample_fog_volumes` and `cs_main` slab-clip prologue |
| `FogVolume.inv_half_ext` | same as above | `fog_volume.wgsl::sample_fog_volumes` |
| `FogVolume.half_diag` | same as above | `fog_volume.wgsl::sample_fog_volumes` |

WGSL changes:

- `sample_fog_volumes` — drop the `half_ext`, `center`, `half_diag` locals; consume `v.center`, `v.inv_half_ext`, `v.half_diag`.
- `cs_main` — new prologue function (e.g. `build_active_intervals`) called once before the march loop; the loop body iterates the union slots returned by the prologue.

No level-format changes. No PRL section changes. No level-compiler changes — the PRL `FogVolumeRecord` carries only the durable AABB; runtime caches the derivatives.

## Risks

- **Degenerate AABBs.** A zero-thickness brush would divide by zero in `inv_half_ext`. The bridge clamps the divisor at `1e-6` (matching the existing `max(half_ext, 1.0e-6)` guard the shader already used) so the fix is layout-neutral.
- **Ray direction with zero components.** Slab-clip's `1.0 / dir` is infinite on axis-aligned rays. Standard fix: let the IEEE-inf propagate; `min`/`max` resolve correctly because the t_min/t_max swap on the affected axis still produces a valid [t_near, t_far] when the ray origin is inside the slab, and produces an empty interval (t_far < t_near) when it's outside. Validate during implementation; if a target adapter mishandles inf, fall back to a per-axis branch.
- **Interval-union overflow.** Capped at `MAX_FOG_VOLUMES` slots. Overflow path collapses to `[min_enter, max_exit]` — looser bound, correct shading.
