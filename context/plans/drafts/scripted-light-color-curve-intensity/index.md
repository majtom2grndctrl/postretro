# Scripted-Light Color-Curve Intensity Fix

> Sibling bugfix to the recent brightness-curve correction at `forward.wgsl:783`. The same intensity-drop class of bug lives in the **color-curve** branch immediately above it (`forward.wgsl:764–773`). A scripted color curve on a pulsing light currently paints near-zero RGB over the static intensity baseline, so any color-animated light reads as nearly black in the forward pass while reading correctly in the animated-baked lightmap compose path. Verified by inspection against the compose-side reference.

## Goal

Make the forward pass's scripted color-curve branch numerically agree with the animated-lightmap compose path (`animated_lightmap_compose.wgsl:200–205`): treat the curve samples as **unit-RGB color**, with the static light's **intensity scalar** reapplied at sample time. This restores parity between dynamic and lightmap-routed contributions for the same scripted light, and matches the contract `pack_animation_descriptor` already encodes (samples in `anim_samples` are unit RGB; `base_color` is unit RGB; intensity lives on the static `GpuLight` slot).

This is a **shader-side seam fix on a shipped pipeline**, not a new lighting capability. Scope is one branch in `forward.wgsl`, no buffer changes, no descriptor changes, no API changes.

## The asymmetry, precisely

`GpuLight.color_and_falloff_model.xyz` is `linear_rgb × intensity` (`lighting/mod.rs:19, 69`). The brightness branch (post-fix, line 783) reads this directly and multiplies by `brightness`, giving `unit_color × intensity × brightness` — correct.

The color branch (lines 764–773) writes `effective_color = sample_color_catmull_rom(...)` directly. The sample buffer is fed from `LightComponent::animation.color: Option<Vec<Vec3Lit>>` — the SDK-side type is documented as "Per-sample color curve" (`primitives/light.rs:258–261`) with no implied intensity scaling, and `pack_animation_descriptor` uploads it raw alongside `base_color = component.color` (unit RGB, `light_bridge.rs:653–655`). So the curve sample is unit RGB. Today the forward pass treats it as the *final* `effective_color` — i.e. drops the static light's intensity entirely.

The animated-lightmap compose path (`animated_lightmap_compose.wgsl:200–205`) instead computes:

```wgsl
let b = max(sample_curve_catmull_rom(brightness), 0.0);            // 1.0 when absent
let c = max(sample_color_catmull_rom(color, base_color), vec3(0)); // unit RGB
let radiance = c * b * entry.weight;                                // weight bakes in intensity
```

where `entry.weight` carries the static light's `intensity` (`animated_light_weight_maps.rs:208`, `contribution_to_weight` at line 257 strips `intensity × dominant-channel color` so it is reapplied via `c * b * weight`). The compose path is multiplicative across `c` and `b`; the forward path is mutually exclusive via `else if`. Two divergences, one root cause.

## Scope

### In scope

- The scripted-descriptor handler at `forward.wgsl:758–784`: replace the color-curve branch's direct assignment with an intensity-aware reapplication that mirrors the compose path.
- Recovering the static light's **intensity scalar** at the shader, from `light.color_and_falloff_model.xyz` and `scripted_desc.base_color` (both already in the bind group / descriptor; no new uploads).
- Resolving the **color × brightness mutual-exclusivity** in the forward branch so a script that supplies *both* curves on the same light gets the multiplicative behavior the compose path already produces, instead of color silently shadowing brightness. See *Open questions* — this may be deferred if the orchestrator's reviewer prefers a smaller patch.

### Out of scope (non-goals)

- Any change to `anim_samples` layout, `ScriptedLightDescriptor` layout, or `pack_animation_descriptor`.
- Any change to the SDK-side `LightAnimation.color` semantics or validation (`primitives/light.rs:86–104`).
- Any change to the animated-lightmap compose path — it is the reference, not the patient.
- Any change to the brightness-only path (the recent fix at line 783 stays as-is).
- Any change to spot-shadow / fog / billboard consumers of `color_and_falloff_model`. They each compose intensity differently and are out of scope (the bug is forward-pass-specific).
- Direction-curve handling at lines 785–787 (correct today; an orientation, not a magnitude).

## Acceptance criteria

Automated (test- or tooling-gated):

- [ ] `forward.wgsl`'s color-curve branch no longer writes `effective_color` directly from `sample_color_catmull_rom`. The curve sample is multiplied by an intensity term derived from `light.color_and_falloff_model.xyz` and `scripted_desc.base_color`. Asserted via source-string check in the existing forward-shader test style (search for the substring removed). [T1]
- [ ] A unit test on a synthetic `GpuLight` + `ScriptedLightDescriptor` with `base_color = (1,1,1)`, static intensity 10, a single-keyframe color curve at `(1,1,1)` and no brightness curve produces an `effective_color` of `(10,10,10)` at `t=0` (parity with the brightness-only branch on the same light). Implemented as a shader-host test if one exists for this surface, otherwise as a Rust-side helper that mirrors the WGSL math. [T1]
- [ ] Same synthetic case with a single-keyframe color curve at `(0.5, 0, 0)` produces `effective_color = (5, 0, 0)` — the curve sets *hue*, the static intensity sets *magnitude*. [T1]
- [ ] If the orchestrator opts in to the multiplicative-with-brightness change (see *Open questions*): a synthetic case with both a color curve at `(1,0,0)` and a brightness curve at `0.5`, static intensity 10, produces `effective_color = (5, 0, 0)` — matching the compose path's `c * b * weight` semantic. [T1, conditional]

Manual / visual (observed by a human running the engine — not machine-verified):

- [ ] On a map with a scripted color-curve light (`content/dev/maps/campaign-test.prl` if one exists there, otherwise a one-off test map), the light is visibly *colored and bright* under the forward pass instead of nearly black. Concretely: a red-pulsing color-curve light on a beige wall reads as a clearly red tint at expected intensity, not a nearly-imperceptible darkening.
- [ ] The same light, viewed where it also influences animated-baked lightmap surfaces, shows consistent magnitude between the dynamic forward contribution and the lightmap contribution — no visible "the dynamic shading is dimmer than the baked shading" seam.
- [ ] No visible regression on lights with brightness curves only (line 783 path untouched) or no animation (line 756 path untouched).

### Task ↔ AC cross-check

| Task | Covering ACs |
|---|---|
| T1 | source-string check; synthetic-light unit tests (both basic and hue cases); visual ACs |

## Tasks

### Task 1: Forward-pass color-curve intensity reapplication

Replace the body of the `scripted_desc.color_count > 0u` branch at `forward.wgsl:764–773` with:

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
    // baked intensity sets magnitude. Mirrors animated_lightmap_compose.wgsl
    // where `c * b * weight` keeps intensity in the weight term. Dominant-
    // channel division matches `contribution_to_weight`'s convention
    // (animated_light_weight_maps.rs:257) and avoids divide-by-near-zero on
    // weak channels.
    let intensity = scripted_light_intensity_scalar(
        light.color_and_falloff_model.xyz,
        scripted_desc.base_color,
    );
    effective_color = unit_sample * intensity;
}
```

`scripted_light_intensity_scalar` is a small helper added near the top of `forward.wgsl` (or in a shared include if one is in use for this file — confirm during implementation). It picks the dominant channel of `base_color`, divides the matching channel of `color_and_falloff_model.xyz` by it, and falls back to `1.0` if `base_color`'s max channel is below an epsilon. This matches the dominant-channel pick in `animated_light_weight_maps.rs:258–264`.

**Conditional sub-step** — if the *Open questions* resolution allows it, change the `else if scripted_desc.brightness_count > 0u` at line 774 to a plain `if`, and fold the brightness factor into the color-curve branch too: `effective_color = unit_sample * intensity * brightness;` where `brightness = sample_curve_catmull_rom(...)` returns 1.0 when absent. This collapses the two branches into one expression and is what the compose path already does.

### Sequencing

Single task. No dependencies, no follow-ups in this spec.

## Wire format

No changes. No PRL bump, no descriptor layout change, no `anim_samples` change, no bind-group change.

## Rough sketch

- Curve samples are unit RGB by contract; the SDK and the bake side already agree on this. The forward pass is the only consumer that dropped intensity on the floor.
- Fix lives entirely in `forward.wgsl`. ~10 lines, one new shader-local helper, no Rust touched.
- The compose path is the reference: `c * b * weight` with intensity baked into `weight`. The forward path's analogue: `unit_sample * intensity_scalar` with intensity recovered from the static `GpuLight` slot.
- Dominant-channel intensity recovery matches the same convention `contribution_to_weight` uses for the bake-side weight strip, so the two paths agree on how to recover a scalar from `(unit_color, color_x_intensity)`.

## Open questions

1. **Color × brightness multiplicativity in the forward path.** The compose path multiplies them; the forward path makes them mutually exclusive today (`else if`). Folding the forward path into the multiplicative form is the principled fix and is what the AC tests would cover. But it is a behavior change for any script that sets *both* curves expecting color to win — there is no documented precedent that color overrides brightness, but the current code does so. Recommendation: include the multiplicative change in T1, because it brings the forward path in line with the compose-side semantics and the SDK doc strings (`primitives/light.rs:253–260`) describe both curves independently with no precedence note. Reviewer to confirm.

2. **Intensity-recovery formula on a zero-channel `base_color`.** If `base_color = (0,0,0)` (a script-pathological case, but admissible — the SDK does not validate non-zero color), the dominant-channel division degenerates. Proposed fallback: return `1.0` and let `unit_sample` carry the magnitude as written — equivalent to today's broken behavior, but only for this degenerate input, and arguably correct given there is no static intensity to reapply. Reviewer to confirm vs. alternatives (e.g. clamp `base_color` max to `epsilon` at pack time in `light_bridge.rs`).

## Investigation notes

- `forward.wgsl:756, 764–773, 774–784` — the bug site and the recently-fixed sibling branch.
- `animated_lightmap_compose.wgsl:193–217` — the reference path. `radiance = c * b * entry.weight` with intensity baked into `weight`.
- `animated_light_weight_maps.rs:208, 257–270` — `contribution_to_weight` strips `intensity × dominant-channel color`; the runtime compose reapplies via `c * b * weight`. Dominant-channel convention reused below.
- `light_bridge.rs:628–670` — descriptor upload. `base_color` (bytes 16–28) is `component.color`, unit RGB. Curve samples uploaded raw.
- `scripting/primitives/light.rs:86–104, 252–266` — SDK contract. `color: Option<Vec<Vec3>>` with the doc string "Per-sample color curve." No intensity semantics implied.
- `lighting/mod.rs:19, 69` — `color_and_falloff_model.xyz` is `linear RGB × intensity` (load-bearing for the recovery formula).
- `curve_eval.wgsl:16–22` — `sample_curve_catmull_rom` returns `1.0` when `count == 0`, which is what makes the multiplicative-fold cleanup work without a branch.
