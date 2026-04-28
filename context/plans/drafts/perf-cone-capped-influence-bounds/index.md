# Cone-Capped Influence Bounds — Spot-Light Optimization

> **Status:** draft
> **Depends on:** `context/plans/in-progress/lighting-foundation/4-light-influence-volumes.md` (the `LightInfluence` sphere bound and PRL section 21 must already exist).
> **Related siblings:** `context/plans/drafts/perf-per-chunk-light-lists/` · `context/plans/drafts/perf-light-buffer-packing/` · parent `context/plans/in-progress/lighting-foundation/index.md`.

---

## Context

Sub-plan 4 uses a **sphere** as the influence volume for spot lights — radius equals the point-light falloff range (see `4-light-influence-volumes.md` §Per-type bounds, and the rationale at lines 32–38). The sphere is a correct conservative bound: cone attenuation zeros fragments outside the angular cutoff, so no fragment rejected by a tight cone bound would have been rejected by the sphere. But a spot-light cone is often a narrow slice of that sphere, and every fragment inside the sphere but outside the cone still enters the per-light switch body at `postretro/src/shaders/forward.wgsl:1036–1049` (spot case) to compute `dist`, `L`, `dist_falloff`, and `cone_attenuation` — before the cone attenuation term zeros the result.

The sphere early-out today lives at `forward.wgsl:1014–1019` (the `if inf_radius <= 1.0e30` block). The cone test will be inserted immediately after that block, at line ~1020, before the per-type switch at ~1028.

Sub-plan 4 explicitly deferred tightening this:

> A tight cone-capped bound would save a few fragments near the cone's far edge but costs a more complex CPU-side intersection test (frustum vs. cone) for the shadow-slot gate. The sphere is a provably correct superset of the cone, and the fragment shader's existing `cone_attenuation` already zeros out fragments outside the angular cutoff. Revisit if profiling shows spot lights dominate light-loop cost.
> — `4-light-influence-volumes.md` line 38

For cyberpunk-interior scenes with many tight spot lights (wall sconces, aimed security floods, billboard up-lighters), the typical spot lights' actual influence cone is a small fraction of its bounding sphere. Adding a cone test after the sphere test rejects those fragments before the per-type switch body runs.

This plan does **not** re-argue sphere-vs-cone correctness. The bound is a superset filter; both sphere and cone must be strict outer bounds of the light's effective footprint.

---

## Goal

Extend `InfluenceRecord` (or add a parallel section) with cone axis + half-angle cosine so the fragment shader can reject cone-exterior fragments before the per-type switch body. Pixel output bit-identical.

---

## Approach

Three tasks, sequenced. A precedes B precedes C.

---

### Task A — Format: extend `LightInfluence` record (or add a sibling section)

**Crate:** `postretro-level-format` · **File:** `src/light_influence.rs`

**Extend section 21** — use the existing `record_stride` forward-compat hook (designed for exactly this in `4-light-influence-volumes.md` line 71). The section 21 header already carries `record_stride` (see `light_influence.rs:32–35`):

```
u32  version         (= 1)
u32  record_count
u32  record_stride   (= 16)
u32  reserved        (= 0)
[record_count × record_stride bytes]
```

"The stride field makes forward-compatible record extension trivial: add fields at the tail; a reader that encounters `record_stride > 16` must read the first 16 bytes of each record and skip the remaining `record_stride - 16` bytes" (`4-light-influence-volumes.md` line 71).

Extend `InfluenceRecord` to 32 bytes under `LIGHT_INFLUENCE_VERSION = 2`:

```
center:           [f32; 3]   // bytes 0..12
radius:           f32        // bytes 12..16
cone_axis:        [f32; 3]   // bytes 16..28, zero for non-spot
cos_half_angle:   f32        // bytes 28..32, NON_SPOT_COS_SENTINEL for non-spot
```

`cos_half_angle` is the cosine of the outer cone half-angle — the same value the runtime already computes from `cone_angle_outer`. Non-spot lights (point, directional) write `pub const NON_SPOT_COS_SENTINEL: f32 = -2.0;` (to be defined in `influence.rs`) — any value ≤ -1.0 means "skip cone test."

Byte order: field serialization uses `to_le_bytes` consistent with the rest of `light_influence.rs` (existing records at lines 56–59 already use `to_le_bytes`; follow that convention).

**Version gate:** Relax the `from_bytes` check at `light_influence.rs:73–81` from the current strict `version != LIGHT_INFLUENCE_VERSION` reject to **accept both 1 and 2**. `v1` sections (16-byte records, stride=16) still load; the runtime fills cone fields with sentinel values. `v2` sections (32-byte records, stride=32) carry real cone data. New binaries rejecting `v1` maps after this change would be a regression; old binaries rejecting `v2` maps is an acceptable pre-release break (project "moves fast, breaks APIs"). The existing `record_stride < 16` error stays.

Update `LightInfluenceSection::to_bytes`/`from_bytes` (`postretro-level-format/src/light_influence.rs:42+`) and the existing round-trip tests.

---

### Task B — Compiler: populate cone fields

**Crate:** `postretro-level-compiler` · **File:** `postretro-level-compiler/src/pack.rs`, function `encode_light_influence` at line 69. This function already iterates `lights`, filters `bake_only`, and maps each `MapLight` to an `InfluenceRecord`; the cone fields are added here.

For each `MapLight`:

- `LightType::Spot`: `cone_axis = normalize(cone_direction)`, `cos_half_angle = cos(cone_angle_outer)`. Both `cone_direction: Option<[f32; 3]>` and `cone_angle_outer: Option<f32>` exist on `MapLight` (`postretro-level-compiler/src/map_data.rs:212–215`). The compiler-side source for `cone_angle_outer` is `MapLight.cone_angle_outer`, which at encode time feeds `encode_alpha_lights` as `cone_angle_outer: l.cone_angle_outer.unwrap_or(0.0)` (`pack.rs:54`).
- `LightType::Point`: `cone_axis = [0,0,0]`, `cos_half_angle = -2.0` sentinel.
- `LightType::Directional`: ditto — sentinel. Directional lights already bypass the sphere test via `f32::MAX` radius; the cone test must also no-op.

Compiler bumps the section `version` field to `2` and `record_stride` to `32`.

---

### Task C — Runtime: struct extension, shader test, loader compatibility

**Crate:** `postretro` · **Files:** `src/lighting/influence.rs`, `src/lighting/mod.rs`, `src/prl.rs` (loader), `src/shaders/forward.wgsl`.

**Runtime struct.** Extend `LightInfluence` in `postretro/src/lighting/influence.rs:11–17`:

```rust
pub struct LightInfluence {
    pub center: Vec3,
    pub radius: f32,
    pub cone_axis: Vec3,       // NEW: zero for non-spot
    pub cos_half_angle: f32,   // NEW: -2.0 sentinel for non-spot
}
```

`pack_influence` at `influence.rs:27` grows to 32 bytes per record. Add a new `pack_influence_v2` (or just replace; the only caller is the GPU upload path).

**GPU binding.** `light_influence` storage buffer at `forward.wgsl:58` currently binds `array<vec4<f32>>`. Use **`array<vec4<f32>>` strided by 2**: entry `i` sphere = `light_influence[2*i]`, entry `i` cone (axis + cos_ha) = `light_influence[2*i+1]`. Flat array avoids struct alignment landmines in WGSL.

**Shader test.** Insert a cone test after the sphere test in the loop body. The sphere early-out today lives at `forward.wgsl:1014–1019`; the cone test goes immediately after (before the per-type switch at ~1028):

```wgsl
if inf_radius <= 1.0e30 {
    let d = in.world_position - influence.xyz;
    if dot(d, d) > inf_radius * inf_radius {
        continue;
    }
    // NEW: cone test. cos_half_angle = NON_SPOT_COS_SENTINEL (-2.0) for non-spots → always passes.
    // Binding: light_influence[2*i] = sphere (xyz=center, w=radius),
    //          light_influence[2*i+1] = cone  (xyz=axis,   w=cos_half_angle)
    let cone_slot = light_influence[2u * i + 1u];
    let cos_ha = cone_slot.w;
    if cos_ha > -1.5 {
        // d = frag - center, so normalize(d) points light→fragment. Correct direction.
        let to_frag = normalize(d);
        if dot(cone_slot.xyz, to_frag) < cos_ha {
            continue;
        }
    }
}
```

Sign note: `d = in.world_position - influence.xyz` (frag minus center), so `normalize(d)` points **center → fragment** (light outward). `dot(cone_axis, normalize(d)) >= cos_ha` is the correct "is fragment inside the cone?" test. The draft had `normalize(-d)` which pointed frag→center — wrong direction; corrected above.

The `cos_ha > -1.5` gate avoids the `normalize` on the non-spot path. `-1.5` is any value below `-1.0` (minimum valid cosine) and above the `-2.0` sentinel.

Ordering: sphere test first, cone test second. Sphere rejects most fragments with a cheaper `dot` + compare (no normalize); cone is the second-pass tightener for the subset that passes the sphere.

**PRL load-time compatibility.** `v1` maps (stride=16) parse with cone fields filled to sentinel. The runtime sphere-only fast path is preserved on older maps. Confirm by loading a pre-Task-A `.prl` and verifying no load error.

**CPU frustum test.** `visible_lights` at `influence.rs:46–54` currently does sphere-vs-frustum. **Leave CPU sphere-only.** Conservative over-allocation of shadow slots is preferable to a more complex frustum-vs-cone test. The fragment shader does the tight per-fragment rejection; the CPU path tolerates some over-allocation with no visual impact. Sub-plan 5 can tolerate it.

---

## Files to modify

| File | Task | Change |
|------|------|--------|
| `postretro-level-format/src/light_influence.rs` | A | Extend record to 32 bytes; bump `LIGHT_INFLUENCE_VERSION = 2`; keep `v1` read path for old maps |
| `postretro-level-format/src/light_influence.rs` tests | A | New round-trip tests for v2 records with spot/non-spot mix |
| `postretro-level-compiler/src/...` (section writer) | B | Populate `cone_axis` + `cos_half_angle` from `MapLight.cone_direction` / `cone_angle_outer`; write stride=32, version=2 |
| `postretro/src/lighting/influence.rs` | C | Extend `LightInfluence` struct; expand `pack_influence` to 32 B/record; update existing tests |
| `postretro/src/lighting/mod.rs` | C | Hook through the extended runtime struct |
| `postretro/src/prl.rs` | C | Fill sentinel cone fields when loading a `v1` section |
| `postretro/src/render/mod.rs` | C | Rebind the influence storage buffer at 32 B stride |
| `postretro/src/shaders/forward.wgsl` | C | Cone test after sphere test (around line 1019); new or restructured binding for cone data |

---

## Acceptance Criteria

1. `cargo test -p postretro-level-format` and `cargo test -p postretro` pass. New format tests cover v1→v2 forward compatibility and spot/non-spot mixing.
2. **Pixel output bit-identical** on every test map, spot-light-heavy or not. The cone test is a strict superset of the cone-attenuation zero region; any pixel change means the test is too tight.
3. Loading a pre-Task-A `.prl` (stride=16, version=1) succeeds without warning — the runtime fills sentinels and the cone test no-ops.
4. On a spot-light-heavy test map (`content/base/maps/occlusion-test.map` or equivalent with ≥8 spot lights), `POSTRETRO_GPU_TIMING=1` shows **≥10% drop in forward-pass time** vs. the sphere-only baseline. On a spot-light-free map, no regression.
5. No new `unsafe`.
6. `cargo clippy` clean across all three crates at `-D warnings`.

---

## Out of scope

- **General AABB bounds for any light type.** Sub-plan 4 argued spheres over AABBs (line 40); that judgment stands. Cone is a second bound layered on the sphere, not a replacement.
- **Tightening point-light bounds.** Spheres are already the natural tight bound for radial falloff.
- **Per-chunk light lists.** Sibling `perf-per-chunk-light-lists` covers the general loop-count reduction; this plan tightens the per-light reject.
- **`GpuLight` struct changes.** Sibling `perf-light-buffer-packing` covers that axis.
- **Dynamic lights** (Milestone 6+).
- **Changing `cone_attenuation` math in the shader.** Preserved verbatim; the cone test is a pre-filter, not a replacement.

---

## Open Questions

All design questions are resolved:

1. **Extend section 21 vs. new sibling section.** → **Extend section 21** using the `record_stride` forward-compat hook.
2. **Tighten CPU frustum gate too, or shader-only.** → **Shader-only.** CPU stays sphere-vs-frustum.
3. **Sphere-then-cone vs. combined test.** → **Sphere then cone**, two separate early-continues.
4. **Sentinel value for non-spot `cos_half_angle`.** → **`pub const NON_SPOT_COS_SENTINEL: f32 = -2.0;`** in `influence.rs`. Gate in shader: `cos_ha > -1.5`.
5. **Section-version negotiation logging.** → **Always log at `debug`** when loading a v1 section: `"LightInfluence v=1, cone data not present — treating all lights as non-spot-bounded"`.

**Remaining acceptance question:** target map for the ≥10% GPU timing threshold is a spot-light-heavy scene. If `content/base/maps/occlusion-test.map` does not contain enough spots (≥8), build a minimal test variant before running the timing gate.
