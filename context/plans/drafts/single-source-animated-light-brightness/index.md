# Single-source animated-light brightness

## Goal

An animated light's current brightness is evaluated twice from the same authored
curve — once on the CPU (shadow eligibility + fog), once on the GPU (rendered
cone). Make brightness single-source-of-truth: the CPU bridge already evaluates
`effective_brightness` once per frame; feed that one value to the GPU per light
instead of letting the shader re-derive it. Then shadow-eligibility brightness
equals rendered brightness **by construction**, and a whole class of CPU/GPU
divergence bugs becomes unrepresentable.

## Background

Brightness today has two independent evaluators of one curve:

- **CPU.** `eval_effective_brightness` → `sample_brightness_at`
  (`crates/postretro/src/scripting/systems/light_bridge.rs`): a Rust Catmull-Rom,
  cycle position `(current_time / period_s + phase).rem_euclid(1.0)`, **no clamp**
  (overshoot between keyframes can return negative). Computed every frame in
  `LightBridge::update`, returned as `LightBridgeUpdate.effective_brightness`
  (one f32 per map light, map-light-index order). Two consumers:
  - shadow-pool eligibility — the `< 0.01` suppression in
    `update_dynamic_light_slots` (`crates/postretro/src/render/mod.rs`);
  - fog spot/point lights — via `collect_all_as_map_lights` →
    `FogVolumeBridge::update_points`.
- **GPU.** `sample_curve_catmull_rom` in
  `crates/postretro/src/shaders/forward.wgsl` (~line 881): cycle position
  `fract(uniforms.time / max(period, 0.0001) + phase)`, result `max(..., 0.0)`
  (clamps overshoot to zero), then `effective_color =
  color_and_falloff_model.xyz * brightness`. This is what actually renders.

These must agree but are separate implementations: two Catmull-Rom routines,
`rem_euclid` vs `fract` wrapping, no-clamp vs `max(.,0)` clamping. A recent bug
ran them on **different clocks** (CPU `script_time` vs GPU wall-clock
`app_start.elapsed()`): the shadow pool shadowed the spots the CPU thought were
bright while the GPU lit different spots, so dynamic spotlights cast no visible
pillar shadows. That was fixed by unifying the clock (the GPU `time` uniform is
now fed `script_time`), but the dual-evaluation fragility remains — any future
divergence in either evaluator reopens the bug class.

## Scope

### In scope

- Per-frame: the bridge's single `effective_brightness[i]` value reaches the GPU
  per light, patched into the packed `GpuLight` record on the CPU mirror
  (`last_lights_upload`), exactly like the existing `patch_shadow_slots` flow.
- Forward shader applies the supplied brightness to base color instead of
  re-deriving it from `anim_samples` + `uniforms.time` for the **brightness**
  channel.
- An invariant test asserting the eligibility brightness equals the value handed
  to the GPU, so the two cannot silently drift.
- Remove the brightness-only Catmull-Rom from the forward path and the
  negative-overshoot clamp divergence it carried.

### Out of scope

- **Color and direction curves stay GPU-evaluated.** They are still sampled from
  the descriptor / `anim_samples` path in the shader. They do not gate shadows,
  so their CPU/GPU consistency is cosmetic, not a correctness boundary. Only
  brightness crosses the render↔simulation seam (it decides shadow slots), so
  only brightness needs single-sourcing.
- The **animated-lightmap compose** and **SH delta compose** passes. Those
  evaluate their own curves for baked-light indirect/animated-atlas contributions
  and are unrelated to the per-light forward `GpuLight` brightness. No change.
- Changing the authored curve format, the descriptor layout, or the
  `anim_samples` buffer (brightness samples still upload — color sampling and the
  CPU's own evaluation both still read them; we stop the *forward shader* from
  re-deriving brightness from them, we do not remove the data).
- The shadow-suppression threshold (`< 0.01`) and fog suppression threshold —
  unchanged.

## Acceptance criteria

- [ ] For an animated brightness-only light, the brightness the forward pass
      applies to base color equals the bridge's `effective_brightness` for that
      light on the same frame — not a value the shader re-derived from samples +
      time.
- [ ] The forward path no longer samples the brightness Catmull-Rom curve; the
      brightness-only divergences (negative-overshoot clamp, `fract` vs
      `rem_euclid` wrap) are gone because only one evaluator remains.
- [ ] Color-only and direction animations still render from their GPU-sampled
      curves (no regression to the cosmetic channels).
- [ ] A unit test fails if the value packed for the GPU diverges from the value
      fed to shadow-eligibility for the same light/frame.
- [ ] A static (non-animated) light renders unchanged: brightness is `1.0` and
      the packed scalar leaves base color untouched.
- [ ] `start_active: Some(false)` reports `0.0` brightness to the GPU and the
      light renders dark (matching the bridge's `0.0` eligibility value).

## Rough sketch

### Where the brightness scalar lives in `GpuLight`

`GpuLight` is four `vec4<f32>` (64 bytes; `GPU_LIGHT_SIZE`). Slot 3,
`cone_angles_and_pad`, currently uses:

| Component | Byte offset | Meaning |
|---|---|---|
| x | 48 | inner cone angle (rad) |
| y | 52 | outer cone angle (rad) |
| z | 56 | shadow slot index (`SHADOW_SLOT_BYTE_OFFSET`) |
| **w** | **60..64** | **reserved pad — currently always zero** |

The `w` component (bytes 60..64) is the natural home for a per-frame brightness
scalar: it is already a free, deterministically-zeroed slot in slot 3, and slot 3
already carries the other per-frame patched field (the shadow slot). This keeps
the brightness patch and the shadow-slot patch in the same vec4, mirroring the
existing pattern. (Constraint, not prescription — the implementor confirms the
free slot when landing; do not assume offset 60 if a future change has claimed
the pad.)

`pack_light_with_slot` (`crates/postretro/src/lighting/mod.rs`) writes slot 3
today. The base pack (bridge-owned, in `light_bridge.rs` via `pack_light`)
writes a neutral brightness (`1.0`) into this scalar so a never-patched / static
light renders unchanged. The per-frame patch overwrites it with
`effective_brightness[i]`.

### Per-frame patch — mirror `patch_shadow_slots`

Add a sibling to `patch_shadow_slots` in `crates/postretro/src/lighting/mod.rs`
that overwrites only the brightness scalar of each already-packed `GpuLight` in a
mirror buffer, leaving every other byte (the bridge's animated base data and the
shadow-slot field) untouched, returning `true` if any byte changed. Same
contract, same change-detection so a redundant GPU upload is skipped.

Plumbing — the brightness patch must run on `last_lights_upload`, the CPU mirror
the renderer keeps in lock-step with the GPU light buffer:

- `upload_bridge_lights` (`render/mod.rs`) seeds `last_lights_upload` from the
  bridge's base bytes (which now carry neutral `1.0` brightness).
- The renderer already holds `light_effective_brightness`
  (`set_light_effective_brightness`, fed from `LightBridgeUpdate.effective_brightness`).
- `update_dynamic_light_slots` already takes `effective_brightness` and calls
  `patch_shadow_slots` on `last_lights_upload`. Apply the brightness patch in the
  same place, on the same mirror, before/after the slot patch, OR-ing the
  `changed` flags so one upload covers both. `effective_brightness` is keyed on
  `level_lights` indices — the same index space `last_lights_upload` /
  `patch_shadow_slots` use — so no re-keying is needed here (unlike the
  candidate-space slot translation).
- The full-pack fallback branch (mirror not yet sized — first frame, light-count
  change) must also write the brightness scalar so frame-zero is consistent;
  thread `effective_brightness` into the `pack_lights_with_slots_into` path there
  (or patch the scratch after packing) so the fallback doesn't ship a stale
  brightness.

### Forward shader change

In `forward.wgsl`, the per-light loop currently, when `scripted_desc.is_active`,
samples the brightness curve and multiplies base color. Replace the
**brightness** branch: read the per-frame scalar from `cone_angles_and_pad.w`
(bitcast not needed — it is a real f32) and multiply
`color_and_falloff_model.xyz` by it. Keep the **color** and **direction**
branches sampling their curves exactly as today (those channels stay
GPU-evaluated). The brightness Catmull-Rom call (`sample_curve_catmull_rom`) and
its `max(.,0)` clamp drop out of the forward path; the helper itself stays
(other passes / the color path may still reference shared curve helpers — verify
before deleting any shared function).

Note the precedence: today color-curve-present wins over brightness; brightness
only applies when `color_count == 0`. Preserve that precedence — the CPU scalar
multiplies base color only on the brightness path, and a color-curve light keeps
overriding `effective_color` from its sampled color (the CPU scalar does not
double-apply on color lights). Pin this interaction explicitly so the implementor
doesn't apply brightness twice or zero out color lights.

### Invariant test

Per `context/lib/testing_guide.md` (seam-crossing, behavior-over-implementation,
no GPU context). A pure-function test in `lighting/mod.rs` or `light_bridge.rs`:
pack a light, patch it with a known `effective_brightness[i]`, then assert the
scalar read back from the packed `GpuLight` slot-3 `w` equals the value that
`update_dynamic_light_slots` would consult for eligibility for that same light —
i.e. the same `effective_brightness[i]`. Because both now read one source, the
test is a guard that no future code path packs a *different* number for the GPU
than it feeds to eligibility. Reference the existing
`patch_shadow_slots_sets_slot_and_preserves_base_bytes` test as the shape to
follow.

## Tradeoffs

- **Cost:** one extra f32 per light patched into the mirror per frame, plus the
  re-upload when it changes. But the per-frame light-buffer patch + conditional
  upload already happens (`patch_shadow_slots`), so the marginal cost is a second
  field in the same loop and the same single `write_buffer` — negligible.
- **Net work is *lower*.** The CPU already evaluates the curve every frame for
  shadows + fog regardless. The GPU Catmull-Rom for brightness was **duplicated**
  work, not saved work; removing it nets out cheaper on the GPU side.
- **Concrete divergence removed:** the CPU `sample_brightness_at` returns negative
  on Catmull-Rom overshoot while the GPU clamps with `max(.,0)`. Single-sourcing
  collapses this to one behavior. Decide the one clamp policy as part of this work
  (the CPU value is the source; clamp it once at the seam — e.g. at pack time —
  so the GPU and shadow eligibility see the identical clamped number). State the
  chosen clamp in the implementation.

## Open questions

- **Clamp location.** The single surviving evaluator (CPU) currently does not
  clamp; the GPU did. Where does the one clamp live so eligibility and render see
  the same number — inside `eval_effective_brightness`, or at pack/patch time?
  Recommendation: clamp at the seam (pack/patch) so the raw curve value and the
  GPU-applied value are identical and the `< 0.01` suppression sees the same
  clamped value the GPU renders. Decide before promotion.
- **Static-light neutral value.** Confirm `pack_light` / the bridge base pack
  writes `1.0` (not `0.0`) into the brightness scalar so an un-patched or static
  light is unaffected. The full-pack fallback branch must do the same.
