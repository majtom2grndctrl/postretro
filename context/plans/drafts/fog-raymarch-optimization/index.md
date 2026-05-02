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
- Bake `center`, `inv_half_ext`, `half_diag` into `FogVolumeRecord` at compile time in the level compiler — derived once from the AABB and written to the PRL.
- Extend the wire format (`fog_volumes.rs::FogVolumesSection::{to_bytes, from_bytes}`) to carry the new fields.
- Extend the GPU `FogVolume` struct with the same fields and copy them through `FogVolumeBridge` from the loaded record into the per-frame GPU record.
- Refactor `sample_fog_volumes` to read the precomputed fields instead of recomputing.
- Add a slab-clip prologue in `cs_main` that produces a per-ray union of `[t_enter, t_exit]` intervals over active fog volumes. Skip the march loop entirely when the union is empty; otherwise march only inside the union.

**Out of scope (future work — revisit only if profiling shows the slab-clip and field cache fall short).**
- Logarithmic / adaptive step size.
- Per-tile (workgroup-shared) volume masking or volume BVHs.
- Reordering/sorting the volumes buffer for spatial locality.

## Acceptance criteria

- On a scene that previously spiked the fog pass to 18 ms+ when looking into a fog volume, the fog raymarch GPU pass time stays at or below 3 ms steady-state with the same volume count and pixel scale. Measured via `POSTRETRO_GPU_TIMING=1` once the pass is added to the timed set, or via existing pass-timing instrumentation if the fog pass is already covered.
- Rays whose union interval is empty perform zero march iterations. Verified by an inserted debug counter (compile-time toggle, not shipped) or by inspection of the disassembly / iteration profile during bring-up; not a runtime invariant.
- Visual output of the fog pass is unchanged at default settings on existing test maps. No new banding, no halos at volume boundaries, no flicker on camera motion. Compared against a reference capture taken before the change.
- `FogVolume` GPU struct stays at a 16-byte-aligned size that satisfies the WGSL `vec3` alignment rules and the existing CPU/GPU layout assert. Exact stride is an implementation detail; the assert in the format crate is updated to whatever the new size is and remains the contract.
- All existing fog tests pass, including the byte-layout round-trip in `crates/postretro/src/fx/fog_volume.rs` and the on-disk wire-format round-trip in `crates/level-format/src/fog_volumes.rs` (both updated to cover the new fields). A freshly compiled PRL containing fog volumes loads and renders correctly end-to-end on at least one test map with `env_fog_volume` brushes.

## Tasks

### Task 1 — Precomputed fog volume fields baked into the PRL

Bake `center`, `inv_half_ext`, `half_diag` at compile time in the level compiler, carry them through the PRL, and populate the GPU record from the loaded fields. No shader behavior change yet; the new fields are written but unread at the end of this task.

**Touches.**
- `crates/level-format/src/fog_volumes.rs` — add `center: [f32; 3]`, `inv_half_ext: [f32; 3]`, `half_diag: f32` to `FogVolumeRecord`. Extend `FogVolumesSection::{to_bytes, from_bytes}` to read/write the new fields and update the `MIN_RECORD_SIZE` sanity-check (currently 60 bytes for 14 × f32 + u32; new fixed payload is 88 bytes for 21 × f32 + u32). Update the `// On-disk layout` doc comment on `FogVolumesSection`. Extend the `round_trip_two_volumes_one_with_tags_one_without` test (and add coverage for the new fields) so wire-format drift is caught.
- `crates/level-compiler/src/pack.rs` — in `encode_fog_volumes`, compute `center`, `inv_half_ext`, `half_diag` from each `MapFogVolume`'s `min`/`max` and write them into the `FogVolumeRecord`. This is the single write site for the precomputed fields. Clamp `half_ext` away from zero (`max(half_ext, splat(1e-6))`) before inverting so a degenerate (zero-thickness) brush can't bake inf/NaN into the PRL.
- `crates/postretro/src/fx/fog_volume.rs` — add matching `center: [f32; 3]`, `inv_half_ext: [f32; 3]`, `half_diag: f32` fields to the GPU `FogVolume` struct. The two existing `_pad0`/`_pad1` slots get absorbed; the struct grows from 64 bytes to 96 bytes (21 × f32 + 3 f32 of trailing pad to keep 16-byte alignment — confirm exact stride during implementation). Update the `FOG_VOLUME_SIZE` assert to the new size and update `pack_fog_volumes_round_trips_density_and_falloff` to cover the new fields.
- `crates/postretro/src/shaders/fog_volume.wgsl` — mirror the new fields on the WGSL `FogVolume` struct so the layouts agree. The shader does not yet read them.
- `crates/postretro/src/scripting/systems/fog_volume_bridge.rs` — `populate_from_level` reads the new precomputed fields straight off the `FogVolumeRecord` and stashes them on `FogVolumeAabb` (no recomputation); `update_volumes` copies them into the per-frame `FogVolume` GPU record.
- `crates/postretro/src/render/fog_pass.rs` — no logic changes; rebuilds automatically because `FOG_VOLUME_SIZE` drives the buffer size.

**Sketch — level compiler (`encode_fog_volumes`).**

```
// Proposed: bake precomputed fields into FogVolumeRecord at compile time.
let min = Vec3::from(v.min);
let max = Vec3::from(v.max);
let half_ext = (max - min) * 0.5;
let center   = (min + max) * 0.5;
// Clamp the divisor away from zero so a degenerate (zero-thickness) brush
// can't produce inf/NaN in the shader downstream.
let inv_half = Vec3::ONE / half_ext.max(Vec3::splat(1e-6));
let half_diag = half_ext.length();

FogVolumeRecord {
    min: v.min,
    max: v.max,
    center: center.to_array(),
    inv_half_ext: inv_half.to_array(),
    half_diag,
    // ...existing fields (density, falloff, color, scatter, ...)
}
```

The runtime side stays trivial: `populate_from_level` copies `entry.center`, `entry.inv_half_ext`, `entry.half_diag` into `FogVolumeAabb`; `update_volumes` copies them into the GPU `FogVolume`. No math at runtime.

**Wire format — `FogVolumesSection` (PRL section 30), updated.**

```
u32  pixel_scale
u32  volume_count
repeat volume_count:
  f32  min_x, min_y, min_z
  f32  density
  f32  max_x, max_y, max_z
  f32  falloff
  f32  color_r, color_g, color_b
  f32  scatter
  f32  height_gradient
  f32  radial_falloff
  f32  center_x, center_y, center_z          // NEW: baked from (min+max)*0.5
  f32  inv_half_ext_x, inv_half_ext_y, inv_half_ext_z  // NEW: 1 / max(half_ext, 1e-6)
  f32  half_diag                              // NEW: |half_ext|
  u32  tag_count
  repeat tag_count: u32 tag_byte_len; u8[] tag_utf8
```

Fixed payload grows from 60 to 88 bytes per volume; the tag tail is unchanged. This is a breaking PRL change — pre-release, fix all consumers (compiler, runtime, tests) in the same pass. No back-compat shim.

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

End-to-end flow for each new field: level compiler → PRL on-disk record → runtime side-table → GPU struct → shader.

| Field | Computed (write site) | PRL boundary | Runtime | GPU read site |
|-------|----------------------|--------------|---------|---------------|
| `FogVolumeRecord.center` → `FogVolume.center` | `level-compiler/src/pack.rs::encode_fog_volumes` (from baked `min`/`max`) | `level-format/src/fog_volumes.rs` `to_bytes`/`from_bytes` | `fog_volume_bridge.rs::populate_from_level` copies into `FogVolumeAabb`; `update_volumes` copies into `FogVolume` | `fog_volume.wgsl::sample_fog_volumes` and `cs_main` slab-clip prologue |
| `FogVolumeRecord.inv_half_ext` → `FogVolume.inv_half_ext` | same | same | same | `fog_volume.wgsl::sample_fog_volumes` |
| `FogVolumeRecord.half_diag` → `FogVolume.half_diag` | same | same | same | `fog_volume.wgsl::sample_fog_volumes` |

WGSL changes:

- `sample_fog_volumes` — drop the `half_ext`, `center`, `half_diag` locals; consume `v.center`, `v.inv_half_ext`, `v.half_diag`.
- `cs_main` — new prologue function (e.g. `build_active_intervals`) called once before the march loop; the loop body iterates the union slots returned by the prologue.

The runtime never recomputes these from `min`/`max`. If the on-disk record is missing them (e.g. an older PRL), parsing fails at the format boundary — pre-release, recompile maps rather than carrying a back-compat shim.

## Risks

- **Degenerate AABBs.** A zero-thickness brush would divide by zero in `inv_half_ext`. The level compiler clamps the divisor at `1e-6` before inverting (matching the existing `max(half_ext, 1.0e-6)` guard the shader already used), so the baked `inv_half_ext` is finite by construction. The format crate's `read_f32` already rejects non-finite floats at the boundary as a defense-in-depth check.
- **Ray direction with zero components.** Slab-clip's `1.0 / dir` is infinite on axis-aligned rays. Standard fix: let the IEEE-inf propagate; `min`/`max` resolve correctly because the t_min/t_max swap on the affected axis still produces a valid [t_near, t_far] when the ray origin is inside the slab, and produces an empty interval (t_far < t_near) when it's outside. Validate during implementation; if a target adapter mishandles inf, fall back to a per-axis branch.
- **Interval-union overflow.** Capped at `MAX_FOG_VOLUMES` slots. Overflow path collapses to `[min_enter, max_exit]` — looser bound, correct shading.
