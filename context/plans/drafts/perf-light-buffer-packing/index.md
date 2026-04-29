# Light Buffer Packing — GpuLight Cache-Footprint Optimization

> **Status:** draft
> **Depends on:** `context/plans/in-progress/lighting-foundation/3-direct-lighting.md` (established the `GpuLight` 5-vec4 layout) · `context/plans/in-progress/lighting-foundation/5-shadow-maps.md` (activated `shadow_info` slot).
> **Related siblings:** `context/plans/drafts/perf-per-chunk-light-lists/` · `context/plans/drafts/perf-cone-capped-influence-bounds/` · parent `context/plans/in-progress/lighting-foundation/index.md`.

---

## Context

`GpuLight` is five `vec4<f32>` slots, 80 bytes per light, documented at `postretro/src/lighting/mod.rs:14–27`:

```
0: position_and_type        (xyz = world position, w = bitcast<f32>(light_type))
1: color_and_falloff_model  (xyz = linear RGB × intensity, w = bitcast<f32>(falloff_model))
2: direction_and_range      (xyz = aim direction, w = falloff_range meters)
3: cone_angles_and_pad      (x = inner angle rad, y = outer angle rad, zw = pad)
4: shadow_info              (shadow-map assignment — 4 × u32 bitcast to f32)
```

At 500 authored lights per level (parent `index.md` line 16), the buffer is 500 × 80 = **40 KB**. Typical GPU L1 data-cache sizes on mid-range-to-modern parts are 16–32 KB per compute unit. The light buffer cannot live in L1 at full population; every sphere-rejected light in the flat loop (see sibling `perf-per-chunk-light-lists` Context) still incurs the `vec4` loads for the reject path, pulling the buffer line by line through L2.

**Even fragments that only need the first two slots** (position/type for the sphere test, plus the type discriminant) still cause the whole struct layout to compete for cache lines on any hardware that fetches a 64 B line at a time — each `GpuLight` straddles more than one line. Reducing the struct to 40–48 bytes means multiple lights per line and double-digit-percent more of the buffer resident in L1 at any given moment.

**Chosen approach: Scheme 1 — merged-shrink to 48 B.** Hot/cold buffer split is a future optimization if cache miss rate stays high after Scheme 1 ships (see Out of Scope).

The merged-shrink saves 40% per light and is one buffer change rather than two.

---

## Current byte layout

Verified against `postretro/src/lighting/mod.rs:55–97`:

| Offset | Bytes | Field | Precision |
|--------|-------|-------|-----------|
| 0..12  | 12 | `position.xyz` | f32×3 — three scalar `f32` fields, **not** `vec3<f32>`; avoids 16-byte alignment padding |
| 12..16 | 4  | `light_type` (u32 via bitcast<f32>) | u32 |
| 16..28 | 12 | `color × intensity` | f32×3 |
| 28..32 | 4  | `falloff_model` (u32 via bitcast<f32>) | u32 |
| 32..44 | 12 | `cone_direction` (the aim direction) | f32×3 |
| 44..48 | 4  | `falloff_range` (meters) | f32 |
| 48..52 | 4  | `cone_angle_inner` (radians) | f32 |
| 52..56 | 4  | `cone_angle_outer` (radians) | f32 |
| 56..64 | 8  | pad | — |
| 64..80 | 16 | `shadow_info` (4 × u32 via bitcast) — `[cast_shadows, shadow_map_index, shadow_kind, reserved]`; 4th u32 is always 0 (`shadow.rs:313`) | u32×4 |
| **Total** | **80** | | |

Dead weight: 8 B of pad (slot 3 zw), plus 4 B of `u32` packed into 4 B of `f32` slot in three places — not wasted in bytes, but `bitcast<u32>` is the signal that a tighter integer-packed alternative exists.

---

## Goal

Halve `GpuLight`'s GPU-facing byte footprint (target 40–48 bytes). Preserve bit-identical pixel output. Measured drop in forward-pass GPU time on a dense-light map. **Do not change PRL on-disk layout** — pack only at GPU upload, to avoid a format break across sibling plans.

---

## Approach

Task order: **A (layout decisions) → B (implement) → C (validate + measure).**

---

### Task A — Layout is decided: Scheme 1 merged-shrink to 48 B

#### New byte layout

| Offset | Bytes | Field | Precision |
|--------|-------|-------|-----------|
| 0..12  | 12 | `position.xyz` — three scalar `f32` fields, **not** `vec3<f32>`; no 16-byte alignment forced | f32×3 |
| 12..16 | 4  | `light_type` (2 b) + `falloff_model` (2 b) + `shadow_kind` (2 b) + `shadow_slot_index` (10 b) + `cast_shadows` (1 b) + pad (15 b) | u32 bitfield |
| 16..22 | 6  | `color × intensity` clamped to max 1000× unit intensity at pack time | f16×3 (stored as u32×2 via `unpack2x16float`) |
| 22..24 | 2  | reserved (formerly `color_exposure` — dropped; see intensity-cap note below) | — |
| 24..28 | 4  | `direction` octahedral-encoded — **CPU normalizes before encode**; point lights store a sentinel (shader ignores direction when `light_type == Point`) | u32 |
| 28..32 | 4  | `falloff_range` | f32 |
| 32..36 | 4  | `cos_inner` + `cos_outer` | f16×2 |
| 36..40 | 4  | flags (CSM cascade layer, future bits) | u32 |
| 40..48 | 8  | spare (shadow-map scale/bias, or future use) | f32×2 |
| **Total** | **48** | |

Shrink ratio: 80 → 48 = **40% smaller**. 500 lights × 48 B = **24 KB** — fits in L1 on most modern GPUs.

**Intensity cap policy (owner decision required):** `color × intensity` is pre-multiplied on the CPU. `f16` max is 65504; without a cap, any `intensity > ~21000` for a white unit-color light would clip. Plan: clamp at pack time to 1000× unit intensity (a ceiling that covers any reasonable authored value and keeps the boomer-shooter aesthetic grounded). **Confirm this cap with the map author before Task B lands.** If 1000× is too restrictive, adjust the constant — but do not add a per-light `color_exposure` escape hatch.

**Direction normalization:** `MapLight.cone_direction` is passed through at `lighting/mod.rs:82–84` without normalization. The new layout requires: (a) for spot/directional lights, CPU normalizes the direction vector before octahedral encode; (b) for point lights (`[0,0,0]`), store a fixed sentinel (e.g., `[0.0, 1.0]` in octahedral space) — the shader branches on `light_type` so the value is ignored.

Unpacks at the top of the WGSL loop:

```wgsl
let color_rgb = vec3<f32>(unpack2x16float(light.color_xy_half).xy, unpack2x16float(light.color_z_reserved_half).x);
let direction = oct_decode(light.direction_oct);
let cos_inner = unpack2x16float(light.cone_cos_half).x;
let cos_outer = unpack2x16float(light.cone_cos_half).y;
```

Three extra instructions (`unpack2x16float` ×2, `oct_decode`) per light that passes the sphere test. Negligible vs. the `length()` + shadow sample + Lambert that follow.

Cone angles stored as **cosines** rather than radians — the shader already converts `cone_angle_outer` to a cosine inside `cone_attenuation`, and `f16` cosines in [-1, 1] have ~0.0003 precision, more than enough for the angular cutoff.

Octahedral direction encoding is the standard GPU-friendly trick for unit vectors: two 16-bit signed components in a u32, decoded with a ~6-instruction shader helper. Precision ~0.001 radians, fine for the Lambert direction.

---

### Task B — Implement the chosen packing

**Crate:** `postretro` · **Files:** `src/lighting/mod.rs`, `src/lighting/shadow.rs` (writes the shadow slot at `lighting/mod.rs:106`), `src/shaders/forward.wgsl`.

Update `pack_light` (`postretro/src/lighting/mod.rs:55–97`) to emit the new byte layout. Preserve the existing `MapLight` → packed-bytes flow; only the byte layout on the GPU side changes. `pack_lights_with_shadows` signature stays; internal memcpy offsets change.

Update the WGSL `GpuLight` struct at `forward.wgsl:43–49` and its binding at `forward.wgsl:56`. The `bitcast<u32>` accessors at `forward.wgsl:1022–1023` (light_type and falloff_model) become a single bitfield unpack.

Do **not** touch `postretro-level-format/src/alpha_lights.rs`. The AlphaLights section keeps its current on-disk layout; the pack happens on the CPU side just before `queue.write_buffer`. This preserves map compatibility and keeps this plan independent of any PRL format rev.

**`write_shadow_info` is a full rewrite, not a tweak.** The current implementation writes 16 B at offset 64 (`shadow.rs:352–357`). The new layout splits shadow data across: the type-bitfield word (offset 12, bits for `shadow_kind`, `shadow_slot_index`, `cast_shadows`), the flags word (offset 36), and spare bytes (offset 40..48). The old `shadow_info` vec4 slot at 64..80 is gone. This requires:

- Rewrite `write_shadow_info` to accept a new intermediate shape and write to the correct offsets. Define this shape explicitly:
  ```rust
  struct PackedShadowBits {
      type_and_shadow: u32,  // merged into offset 12 bitfield
      flags: u32,            // written to offset 36
      spare: [u32; 2],       // written to offset 40..48
  }
  ```
- `per_light_info: Vec<[u32; 4]>` at `shadow.rs:346–347` flows into `pack_lights_with_shadows` at `lighting/mod.rs:106`. The `[u32; 4]` shape (`[cast_shadows, shadow_map_index, shadow_kind, reserved]`) doesn't map cleanly to the new multi-offset layout. Update `ShadowAssignment.per_light_info` to `Vec<PackedShadowBits>` (or an equivalent tuple) and update all call sites.
- **The tests at `shadow.rs:731–754` hard-code offsets 64..80 and must be rewritten** to check the new split offsets (12, 36, 40..48).

Extensive test coverage already exists in `postretro/src/lighting/mod.rs:141–289`. Update all the offset-asserting tests (`read_f32(&bytes, 0) == 1.0` etc.) to the new offsets; add f16 round-trip tolerance (`1e-3` range) for the color tests.

---

### Task C — Validate and measure

**Crate:** `postretro` · **File:** `src/render/mod.rs`

**Visual validation:** there is no automated golden-image harness in `postretro/tests/`. Use a manual visual diff: screenshot the same scene before and after the change (Alt+Shift+screenshot chord or equivalent) and compare. f16 color introduces ~0.001-magnitude quantization; expect sub-LSB pixel differences on 8-bit output for most fragments. HDR-bright emissive lights specifically checked — f16 clips at ~65504, above any reasonable intensity after the 1000× cap, so overflow should not occur.

**Performance measurement:** `POSTRETRO_GPU_TIMING=1` forward-pass time, targeting a ≥5% drop on a dense-light scene. If a 500-light test map does not exist at validation time, authoring one is a prerequisite step — add `content/base/maps/dense-light-500.map` (or equivalent) as a blocking sub-task before closing Task C. On a sparse map, expect no regression (unpack cost is real but tiny per light).

---

## Files to modify

| File | Task | Change |
|------|------|--------|
| `postretro/src/lighting/mod.rs` | B | `pack_light` emits 48 B layout; `GPU_LIGHT_SIZE = 48`; update all offset-checking tests |
| `postretro/src/lighting/shadow.rs` | B | `write_shadow_info` **rewritten** — shadow data now splits across offsets 12, 36, 40..48; tests at lines 731–754 must be rewritten; `ShadowAssignment.per_light_info` type updated to `Vec<PackedShadowBits>` |
| `postretro/src/shaders/forward.wgsl` | B | New `GpuLight` struct; unpack helpers (`oct_decode`, `unpack2x16float` calls); binding stride change |
| `postretro/src/render/mod.rs` | C | Possibly no change; verify buffer `size` arithmetic uses `GPU_LIGHT_SIZE` rather than a hard-coded 80 |
| `postretro/src/lighting/influence.rs` | — | **Not modified.** Sibling `perf-cone-capped-influence-bounds` owns `LightInfluence` layout changes. |

No changes to `postretro-level-format` or `postretro-level-compiler`. On-disk PRL format stays exactly as-is.

---

## Acceptance Criteria

1. `cargo test -p postretro` passes. Updated packing tests check new offsets with f16 tolerance.
2. `GPU_LIGHT_SIZE` is 48 (or whatever Task A settles on). The comment block at `postretro/src/lighting/mod.rs:14–27` is rewritten to document the new layout.
3. Manual visual diff (screenshot before/after on the same scene) shows no visible difference. HDR-bright emissive lights specifically checked — f16 clips at ~65504, above any value reachable after the 1000× intensity cap.
4. `POSTRETRO_GPU_TIMING=1` forward-pass time on a dense-light map (500+ lights) shows ≥5% improvement. If the test map doesn't exist, authoring it is a prerequisite. Sparse maps: no regression.
5. No new `unsafe`.
6. `cargo clippy -p postretro -- -D warnings` clean.
7. On-disk `.prl` files are **not** touched. A pre-change map loads unchanged.

---

## Out of scope

- **Changing PRL on-disk layout.** `alpha_lights.rs` stays as-is. Packing happens between CPU upload and GPU storage; the CPU-side intermediate (`MapLight`) keeps full f32 precision.
- **Changing `LightInfluence` layout.** Sibling `perf-cone-capped-influence-bounds` owns that buffer; this plan must not touch `postretro/src/lighting/influence.rs` to avoid a merge conflict.
- **Per-chunk light lists.** Sibling `perf-per-chunk-light-lists`.
- **Reducing the number of shadow-casting lights / shadow slots.** Sub-plan 5's concern.
- **Converting to compressed textures for color / range / shadow** (e.g., R10G10B10A2 for direction). `f16` covers 99% of the cache win with dramatically less shader complexity.
- **Hot/cold buffer split.** Future optimization if cache miss rate stays high after Scheme 1 ships.

---

## Open Questions

1. **Intensity cap value.** Plan calls for clamping `color × intensity` to 1000× unit intensity at pack time. Confirm this ceiling fits the level-design intent (e.g., are any existing authored lights expected to exceed it?). If 1000× is too low, raise the constant — but do not restore the `color_exposure` per-light escape hatch.
2. **Dense-light test map.** Does `content/base/maps/` already contain a 500-light interior test level suitable for GPU timing? If not, authoring one is a prerequisite for Task C's acceptance measurement.
3. **Bitfield layout for the type+model+shadow-kind word.** `u32` bitfield layouts in WGSL are done via shift-and-mask. Pin the bit assignments explicitly (e.g., `[0..2]` light_type, `[2..4]` falloff_model, `[4..6]` shadow_kind, `[6..16]` shadow_slot_index, `[16]` cast_shadows, `[17..32]` reserved). Document at the binding site in `forward.wgsl`.
4. **Octahedral encoding precision.** Default 16-bit-per-axis octahedral gives ~0.001 radian error — fine for Lambert and cone attenuation. If banding appears on directional-light Lambert near grazing angles, fall back to f32 for directional lights only (there are few; cheap branch).

Resolved (no longer open):
- Scheme 1 vs Scheme 2: **Scheme 1 chosen.**
- `f16` storage: **use `u32` + `unpack2x16float`** — sidesteps the `shader-f16` extension gate.
- `shadow_info` 4th u32: **always zero** (`shadow.rs:313`). Collapsed into the bitfield.
- `color_exposure` field: **dropped**. Intensity capped at pack time.
