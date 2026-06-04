# Scripted-Light Color-Curve Intensity Fix

> Sibling bugfix to the brightness-curve handling in the same scripted-light block. The **color-curve** branch (`forward.wgsl:881–890`) drops the static light's intensity, while the **brightness** branch right below it (`forward.wgsl:900`) and the static baseline (`forward.wgsl:873`) both keep `color × intensity`. A scripted color curve on a light therefore paints raw unit-RGB over the static intensity baseline, so any color-animated light reads as nearly black in the forward pass. The fix makes the color branch consistent with the forward pass's own brightness branch and static baseline. The compose path is **not** the reference (see below). Verified by inspection.

## Goal

Make the forward pass's scripted color-curve branch consistent with the forward pass's own brightness branch (`forward.wgsl:900`) and static baseline (`forward.wgsl:873`), both of which keep `unit_color × intensity`. The color branch must treat its curve samples as **unit-RGB color** and reapply the static light's **intensity scalar** at sample time, so `effective_color = unit_sample × intensity` — the same magnitude convention the brightness branch (`unit_color × intensity × brightness`) and the static baseline (`unit_color × intensity`) already use. This matches the contract `pack_animation_descriptor` already encodes (samples in `anim_samples` are unit RGB; `base_color` is unit RGB; intensity lives on the static `GpuLight` slot).

This is a **shader-side seam fix on a shipped pipeline**, not a new lighting capability. Scope is one branch in `forward.wgsl`, no buffer changes, no descriptor changes, no API changes.

## The asymmetry, precisely

`GpuLight.color_and_falloff_model.xyz` is `linear_rgb × intensity` (`lighting/mod.rs:19, 78–80`; same premultiply on the scripted/dynamic path at `light_bridge.rs:858`). The forward block establishes that as the baseline: `effective_color` initializes to `light.color_and_falloff_model.xyz` (`forward.wgsl:873`) — i.e. `unit_color × intensity`. The brightness branch preserves intensity: `effective_color = light.color_and_falloff_model.xyz * brightness` (`forward.wgsl:900`), giving `unit_color × intensity × brightness`. Both keep the intensity term.

The color branch (`forward.wgsl:881–890`) instead overwrites `effective_color` with `max(sample_color_catmull_rom(...base_color), vec3<f32>(0.0))` — a clamped raw curve sample, no intensity multiply. The sample buffer is fed from `LightComponent::animation.color: Option<Vec<Vec3>>` — the SDK-side type is documented as "Per-sample color curve. Only valid on dynamic lights." (`primitives/light.rs:257–261`) with no implied intensity scaling, and `pack_animation_descriptor` uploads it raw alongside `base_color = component.color` (unit RGB, `light_bridge.rs:658–660`). So the curve sample is unit RGB. The color branch treats it as the *final* `effective_color` — dropping the static light's intensity entirely. Within one block, the static baseline and the brightness branch carry intensity and the color branch does not. That inconsistency is the bug.

### Why the compose path is not the reference

The animated-lightmap compose path is **intensity-free** and is therefore *not* a valid yardstick. The compiler strips intensity out of the weight: `contribution_to_weight` (`animated_light_weight_maps.rs:335–348`) divides by `denom = c_color * intensity` and returns `(c_contrib / denom).max(0.0)` (call site `animated_light_weight_maps.rs:255`). The compose shader then accumulates with no intensity term — `accum = accum + c * b * entry.weight` (`animated_lightmap_compose.wgsl:138–157`), where `c` and `b` are unit-RGB color and brightness and `entry.weight` is a neutral Lambert × falloff × cone scalar (intensity already divided out). Enforcing parity with compose would re-introduce the intensity drop. The correct reference is the forward pass's own intensity-carrying paths (`forward.wgsl:873` and `:900`).

## Scope

### In scope

- The scripted-descriptor handler at `forward.wgsl:872–905`: rework the color-curve branch (`forward.wgsl:881–890`) so it reapplies the static intensity scalar instead of writing the clamped unit-RGB sample as the final color.
- Recovering the static light's **intensity scalar** at the shader, from `light.color_and_falloff_model.xyz` and `scripted_desc.base_color` (both already in the bind group / descriptor; no new uploads).
- Resolving the **color × brightness mutual-exclusivity** in the forward branch so a script that supplies *both* curves on the same light gets multiplicative behavior, instead of color silently shadowing brightness. See *Open questions* — this may be deferred if the orchestrator's reviewer prefers a smaller patch.

### Out of scope (non-goals)

- Any change to `anim_samples` layout, `ScriptedLightDescriptor` layout, or `pack_animation_descriptor`.
- Any change to the SDK-side `LightAnimation.color` semantics or validation (`primitives/light.rs:86–104`).
- Any change to the animated-lightmap compose path. It is intensity-free and out of scope — neither the patient nor the reference.
- Any change to the brightness branch (`forward.wgsl:900`) — it already keeps intensity and stays as-is.
- Any change to spot-shadow / fog / billboard consumers of `color_and_falloff_model`. They each compose intensity differently and are out of scope (the bug is forward-pass-specific).
- Direction-curve handling at `forward.wgsl:902–904` (correct today; an orientation, not a magnitude).

## Acceptance criteria

Automated (test- or tooling-gated):

- [ ] `forward.wgsl`'s color-curve branch (`forward.wgsl:881–890`) no longer assigns the clamped `sample_color_catmull_rom(...)` result as the final `effective_color`. The clamped unit sample is multiplied by an intensity term derived from `light.color_and_falloff_model.xyz` and `scripted_desc.base_color`. Asserted via source-string check in the existing forward-shader test style: the branch now contains an intensity-multiply, and the bare `effective_color = max(sample_color_catmull_rom(...), vec3<f32>(0.0));` shape is gone. Scope the assertion to the **assignment**, not the sample call: the fix retains `max(sample_color_catmull_rom(...), vec3<f32>(0.0))` (rebound to `unit_sample`), so a substring check for that fragment alone would still match post-fix and give false confidence. Assert on `effective_color = unit_sample * intensity` (within the color branch) and the absence of `effective_color = max(sample_color_catmull_rom`; do not assert on the brightness branch's legitimate `effective_color = light.color_and_falloff_model.xyz * brightness` at `forward.wgsl:900`. [T1]
- [ ] A unit test on a synthetic `GpuLight` + `ScriptedLightDescriptor` with `base_color = (1,1,1)`, static intensity 10, a single-keyframe color curve at `(1,1,1)` and no brightness curve produces an `effective_color` of `(10,10,10)` at `t=0` — matching the brightness branch on the same light (which would give `unit_color × intensity × 1.0 = (10,10,10)`). Implemented as a Rust unit test that mirrors the WGSL math directly in the intensity-recovery formula — no GPU-dispatch test exists for the forward-pass surface. [T1]
- [ ] Same synthetic case with a single-keyframe color curve at `(0.5, 0, 0)` produces `effective_color = (5, 0, 0)` — the curve sets *hue*, the static intensity sets *magnitude*. [T1]
- [ ] If the orchestrator opts in to the multiplicative-with-brightness change (see *Open questions*): a synthetic case with both a color curve at `(1,0,0)` and a brightness curve at `0.5`, static intensity 10, produces `effective_color = (5, 0, 0)` — `unit_sample × intensity × brightness`, the multiplicative form the brightness branch already implies. [T1, conditional]

Manual / visual (observed by a human running the engine — not machine-verified):

- [ ] On a map with a scripted color-curve light (`content/dev/maps/campaign-test.prl` if one exists there, otherwise a one-off test map), the light is visibly *colored and bright* under the forward pass instead of nearly black. Concretely: a red-pulsing color-curve light on a beige wall reads as a clearly red tint at expected intensity, not a nearly-imperceptible darkening.
- [ ] On the same light, swapping the color curve for an equivalent brightness curve (or comparing against the brightness branch at matching values) produces the same forward-pass magnitude: a color curve at `(1,1,1)` now lights the surface as brightly as a brightness curve at `1.0` on the same static light. The color branch and the brightness branch agree in magnitude. (Do **not** check forward-vs-lightmap parity — the compose path is intensity-free and would read dimmer by construction.)
- [ ] No visible regression on lights with brightness curves only (`forward.wgsl:900` path untouched) or no animation (`forward.wgsl:873` baseline untouched).

### Task ↔ AC cross-check

| Task | Covering ACs |
|---|---|
| T1 | source-string check; synthetic-light unit tests (both basic and hue cases); visual ACs |

## Tasks

### Task 1: Forward-pass color-curve intensity reapplication

Replace the body of the `scripted_desc.color_count > 0u` branch at `forward.wgsl:881–890` with:

```wgsl
if scripted_desc.color_count > 0u {
    let unit_sample = max(
        sample_color_catmull_rom(
            scripted_desc.color_offset,
            scripted_desc.color_count,
            cycle_t,
            scripted_desc.base_color,
        ),
        vec3<f32>(0.0),
    );
    // Recover the static intensity scalar so the curve sets hue while the
    // static intensity sets magnitude — matching this block's brightness
    // branch (`unit_color * intensity * brightness`) and static baseline
    // (`unit_color * intensity`). Dominant channel avoids divide-by-near-zero
    // on weak channels.
    let intensity = scripted_light_intensity_scalar(
        light.color_and_falloff_model.xyz,
        scripted_desc.base_color,
    );
    effective_color = unit_sample * intensity;
}
```

`scripted_light_intensity_scalar` is a small helper added near the top of `forward.wgsl` (before `fs_main`, consistent with how `sample_animated_direction` is placed at `forward.wgsl:362` — there is no separate include file for this shader). It picks the dominant channel of `base_color`, divides the matching channel of `color_and_falloff_model.xyz` by it, and falls back per *Open questions* Q2 if `base_color`'s max channel is below an epsilon. This matches the dominant-channel pick in `animated_light_weight_maps.rs:336–342`.

**Conditional sub-step** — if the *Open questions* resolution allows it, change the `else if scripted_desc.brightness_count > 0u` at `forward.wgsl:891` to a plain `if`, and fold the brightness factor into the color-curve branch too: `effective_color = unit_sample * intensity * brightness;` where `brightness = sample_curve_catmull_rom(...)` returns 1.0 when absent. This collapses the two branches into one expression so a script supplying both curves gets multiplicative behavior. The standalone brightness-only branch (`if scripted_desc.brightness_count > 0u { effective_color = light.color_and_falloff_model.xyz * brightness; }`) is retained unchanged so a brightness-only light still works correctly — the fold is additive inside the color branch, not a replacement.

**Sequencing trap (must get right):** once `else if` becomes a plain `if`, the brightness-only branch will also run for a color+brightness light and would overwrite `effective_color` from the static slot, clobbering the folded color result. Guard against it: either keep the brightness branch as `if scripted_desc.color_count == 0u && scripted_desc.brightness_count > 0u { ... }`, or leave it as a true `else if` so it only fires when the color branch did not. The fold inside the color branch already applies brightness (`sample_curve_catmull_rom` returns 1.0 when absent), so the brightness branch must never run on a light that took the color branch.

### Sequencing

Single task. No dependencies, no follow-ups in this spec.

## Wire format

No changes. No PRL bump, no descriptor layout change, no `anim_samples` change, no bind-group change.

## Rough sketch

- Curve samples are unit RGB by contract; the SDK and the bake side already agree on this. The forward pass is the only consumer that dropped intensity on the floor.
- Fix lives entirely in `forward.wgsl`. ~10 lines, one new shader-local helper, no Rust touched.
- The reference is the forward pass's own intensity-carrying paths: the static baseline (`forward.wgsl:873`, `unit_color × intensity`) and the brightness branch (`forward.wgsl:900`, `unit_color × intensity × brightness`). The color branch's analogue: `unit_sample × intensity_scalar`, with intensity recovered from the static `GpuLight` slot. The compose path is **not** the reference — it is intensity-free (`accum = accum + c * b * entry.weight`, no intensity term) and matching it would re-drop intensity.
- Dominant-channel intensity recovery only borrows `contribution_to_weight`'s *channel-selection* convention (`animated_light_weight_maps.rs:336–342`) — the same way of recovering a scalar from `(unit_color, color × intensity)`. It does not adopt compose's magnitude (compose strips intensity; the forward fix reapplies it).

## Open questions

1. **Color × brightness multiplicativity in the forward path.** The forward path makes the two curves mutually exclusive today (`else if`), so color silently shadows brightness when a script sets both. The only existing consumer that combines them — the compose pre-pass — does so multiplicatively (`c * b`), which is the structural precedent for combining the two curves (a separate question from magnitude, where compose is *not* the reference). Folding the forward path into the multiplicative form is the principled fix and is what the AC tests would cover. But it is a behavior change for any script that sets *both* curves expecting color to win — there is no documented precedent that color overrides brightness, yet the current code does so. Recommendation: include the multiplicative change in T1, since the SDK doc strings (`primitives/light.rs:253–260`) describe both curves independently with no precedence note. Reviewer to confirm. If no contrary feedback is received before implementation starts, proceed with the multiplicative fold.

2. **Intensity-recovery formula on a zero-channel `base_color`.** If `base_color = (0,0,0)` (a script-pathological case, but admissible — the SDK does not validate non-zero color), the dominant-channel division degenerates. The bake-side precedent is explicit: `contribution_to_weight` guards `denom = c_color * intensity` and **returns `0.0`** when `denom <= 1.0e-6` (`animated_light_weight_maps.rs:344–345`) — a degenerate light contributes nothing. Adopt that precedent: on a sub-epsilon dominant channel, yield a zero intensity scalar so the light goes dark, matching the compiler's handling of the same degenerate input. Returning `1.0` (letting `unit_sample` carry the magnitude) would diverge from this precedent and re-create today's intensity-free behavior for that input — choose it only with explicit justification. Reviewer to confirm; an alternative is clamping `base_color` max to `epsilon` at pack time in `light_bridge.rs`. If no contrary feedback is received before implementation starts, adopt the 0.0 precedent.

## Investigation notes

- `forward.wgsl:872–905` — the scripted-descriptor handler block. `effective_color` init at `:873` (`unit_color × intensity`); the buggy color branch at `:881–890` (clamped unit-RGB sample, no intensity multiply); the brightness branch at `:900` (`unit_color × intensity × brightness`, keeps intensity); direction-curve handling at `:902–904`.
- `animated_lightmap_compose.wgsl:138–157` — the compose path. **Not** the reference: it is intensity-free. The accumulator line is `accum = accum + c * b * entry.weight;` (`:157`), where `c`/`b` are unit-RGB color/brightness and `entry.weight` already has intensity divided out. Matching it would re-drop intensity.
- `animated_light_weight_maps.rs:335–348` (level-compiler crate — there is a separate level-format copy; cite level-compiler) — `contribution_to_weight` divides intensity OUT: `denom = c_color * intensity; if denom <= 1.0e-6 { return 0.0; } (c_contrib / denom).max(0.0)`. Call site at `:255`. Dominant-channel pick at `:336–342` — only the channel-selection convention is reused by the fix, not the intensity-stripping magnitude.
- `light_bridge.rs:658–660` — `base_color` upload: `bytes[16..28] = component.color`, unit RGB. `pack_animation_descriptor` starts ~`:633`; curve samples uploaded raw alongside `base_color`.
- `scripting/primitives/light.rs:257–261` — SDK doc-field registration for `color: Option<Vec<Vec3>>`, "Per-sample color curve. Only valid on dynamic lights." No intensity semantics implied. (The "Only valid on dynamic lights." clause is stale — the validation block at `primitives/light.rs:86–104` no longer gates on `is_dynamic` — but the live doc string in the file still reads this way.) (The Rust `LightAnimation` type itself lives in `scripting/components/light.rs:44`, not in primitives.)
- `lighting/mod.rs:19, 78–80` — `color_and_falloff_model.xyz` is `linear RGB × intensity`: doc at `:19`, the `color × intensity` premultiply at `:78–80` (load-bearing for the recovery formula).
- `curve_eval.wgsl:16–22` — `sample_curve_catmull_rom` returns `1.0` when `count == 0`, which is what makes the multiplicative-fold cleanup work without a branch.
