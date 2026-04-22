# Animated Curve Eval

## Goal

Replace the linear interpolation in today's SH animation pass with shared Catmull-Rom helpers, so the new animated-lightmap compose pass and the existing SH animation pass evaluate `AnimationDescriptor` curves through one WGSL function. Verifies WGSL correctness against the `splines` crate as the CPU-side reference — no hand-rolled Rust evaluator. Adds unit-test coverage where none exists today.

Prerequisite for `animated-light-weight-maps` (Spec 2). Depends on nothing.

## Scope

### In scope

- New WGSL module `postretro/src/shaders/curve_eval.wgsl` with two functions: `sample_curve_catmull_rom(samples_offset, count, cycle_t) -> f32` and `sample_color_catmull_rom(samples_offset, count, cycle_t, base_color) -> vec3<f32>`. Both read a `anim_samples: array<f32>` storage buffer by lexical name — **the helper does NOT declare the buffer**. Consumers declare `@group(X) @binding(Y) var<storage, read> anim_samples: array<f32>;` in their own shader source *before* the `curve_eval.wgsl` string is concatenated. This lets `forward.wgsl` and the future `animated_lightmap_compose.wgsl` bind the same logical buffer at different `(group, binding)` pairs without conflict. Both handle `count == 0` (return unit / `base_color`) and `count == 1` (return the single sample). The four-tap neighborhood wraps via modulo, matching the "curve is a closed loop over one period" convention already established. `count == 2` evaluates cleanly (neighbors repeat) and degrades to linear-equivalent output — documented behavior, not a bug.
- Runtime shader loader concatenates `curve_eval.wgsl` into every shader module that needs it. Enumerated sites (authoritative): `postretro/src/render/sh_volume.rs` for the forward pipeline (today); `postretro/src/render/animated_lightmap.rs` for the compose pipeline (when Spec 2 lands). The helper source is appended *after* the consumer's `anim_samples` declaration and before pipeline creation. No preprocessor, consistent with current codebase patterns.
- Refactor `forward.wgsl` SH animation path: delete `eval_animated_brightness` and `eval_animated_color` (linear) and call `sample_curve_catmull_rom` / `sample_color_catmull_rom` directly at the two SH-volume use sites. Behavior changes from linear to Catmull-Rom — this is the point of the refactor.
- `splines` crate added as a **dev-dependency** (tests only, not runtime). Test helpers construct a `Spline<f32, f32>` and `Spline<f32, Vec3>` from keyframes with two prepended and two appended samples (closed-loop emulation). Reference values feed the WGSL parity test vector and are available for Spec 2's weight-map bake tests as a shared fixture.
- Unit tests exercising the WGSL helpers via a compute-dispatch test harness: evaluate at a fixed test vector (`count ∈ {2, 3, 4, 8}`, `cycle_t ∈ {0.0, 0.17, 0.5, 0.83, 0.99}`), read back, compare to `splines` reference within `1e-4` tolerance.
- Documentation of the closed-loop convention in the WGSL doc comment. Today it's implicit in the linear code — make it explicit.

### Out of scope

- Changing `AnimationDescriptor` layout, storage format, or the packing in `build_animation_buffers`. Stride stays 48. `anim_samples` buffer stays flat-f32 with the same offset+count convention.
- Non-uniform knot spacing. Samples remain uniformly spaced over `[0, 1)` cycle time.
- Arbitrary tension parameter. Use standard Catmull-Rom (tension = 0.5).
- Monotonic cubic or cubic Hermite with explicit tangents. Research settled on Catmull-Rom; Spec 2 relies on this.
- Hand-rolled CPU-side curve evaluator. `splines` is the CPU reference. The Rust codebase will never call Catmull-Rom at runtime — GPU evaluation is the only production path.
- The `active` flag on `AnimationDescriptor`. Owned by Spec 2 Task 4.
- Any animated-light-weight-maps work. This spec is self-contained.
- Backward compatibility with the linear evaluator. Clean replacement — the bundled-test visual capture is the verification, not dual code paths.

## Acceptance criteria

- [ ] `sample_curve_catmull_rom` and `sample_color_catmull_rom` exist in `curve_eval.wgsl` and are called directly from `forward.wgsl`'s SH-volume use sites.
- [ ] No inline curve interpolation remains in `forward.wgsl`. `eval_animated_brightness` and `eval_animated_color` are deleted; call sites invoke the shared helpers directly.
- [ ] WGSL helper output agrees with `splines`-produced reference values to within `1e-4` on a fixed test vector covering `count ∈ {2, 3, 4, 8}` and `cycle_t ∈ {0.0, 0.17, 0.5, 0.83, 0.99}`. Tolerance documented as conservative to absorb GPU-side f32 order-of-ops drift.
- [ ] At every sample point `t_k = k / count`, the WGSL evaluator returns the stored sample exactly (within `1e-4`). Asserted for `count ∈ {2, 3, 4, 8}`.
- [ ] Cycle-boundary wrap is continuous: `sample(t = 1 - ε)` and `sample(t = 0)` agree within `O(ε)`. Finite-difference check runs on a smooth authored curve (e.g. `sin(2πt)`).
- [ ] Degenerate-case behavior: `count == 0` returns unit (`1.0` scalar / `base_color` RGB); `count == 1` returns the single sample; `count == 2` matches `splines` output (degrades to linear-equivalent); no NaN / Inf / out-of-bounds reads at any count.
- [ ] Runtime frame capture on the bundled SH-volume test map: max per-channel luma delta vs the linear baseline stays under `0.15` (normalized) across 120 frames; zero NaN, Inf, or negative channels. Before/after frame pair saved to the plan's commit.
- [ ] `forward.wgsl` and the future `animated_lightmap_compose.wgsl` can declare `anim_samples` at different `(group, binding)` pairs without symbol conflict. Spec 2 validates this when its compose pipeline lands; this spec's AC is satisfied by the helper being textually binding-agnostic.
- [ ] `cargo check -p postretro` clean. `cargo test -p postretro curve_eval` passes.
- [ ] `POSTRETRO_GPU_TIMING=1` shows no regression in the SH animation pass time.

## Tasks

### Task 1: WGSL helper module

Create `postretro/src/shaders/curve_eval.wgsl`. Implements:

- `sample_curve_catmull_rom(samples_offset: u32, count: u32, cycle_t: f32) -> f32` — four-tap uniform Catmull-Rom over a closed loop. `cycle_t ∈ [0, 1)`.
- `sample_color_catmull_rom(samples_offset: u32, count: u32, cycle_t: f32, base_color: vec3<f32>) -> vec3<f32>` — same, applied componentwise to RGB triplets packed as `[r, g, b, r, g, b, ...]` at `samples_offset`.

The helper does **not** declare `anim_samples`. Consumer shaders declare the buffer at their preferred `(group, binding)` before the helper source is concatenated. Degenerate-case early-outs (`count <= 1`) precede the four-tap computation. Document the closed-loop wrap convention at the top of the file; cite Wikipedia "Cubic Hermite spline § Catmull–Rom spline" for the basis matrix.

### Task 2: Test fixture + WGSL parity test

Add `splines = "4"` to `postretro`'s dev-dependencies (`[dev-dependencies]` in `postretro/Cargo.toml`). Build a test-only helper `fn catmull_rom_reference(samples: &[f32], cycle_t: f32) -> f32` (and RGB variant) that constructs a `Spline` with two prepended and two appended samples to emulate closed-loop wrap, then evaluates at the scaled cycle position. Place the helper at `postretro/src/render/curve_eval_test.rs` (gated by `#[cfg(test)]`).

Write a compute-shader-based parity test: dispatch a small compute pipeline that calls `sample_curve_catmull_rom` over the fixed test vector, reads back via a `MAP_READ` buffer, and asserts each value agrees with the `splines` reference within `1e-4`. RGB variant gets the same treatment using three channel-parallel `Spline<f32, f32>` instances — `splines` does not implement `Interpolate` for `[f32; 3]`.

### Task 3: Refactor forward.wgsl SH path

Delete `eval_animated_brightness` and `eval_animated_color` from `forward.wgsl`. Update the two (or three) call sites to invoke `sample_curve_catmull_rom(desc.brightness_offset, desc.brightness_count, t)` and `sample_color_catmull_rom(desc.color_offset, desc.color_count, t, desc.base_color)` directly. Update `postretro/src/render/sh_volume.rs`'s shader-source construction to append `curve_eval.wgsl` after `forward.wgsl`'s `anim_samples` declaration and before pipeline creation.

### Task 4: Frame capture verification

Run the bundled SH-volume test map before and after Task 3. Capture a normalized per-channel luma delta histogram across 120 frames; assert the max stays under `0.15`, no NaN / Inf, no negative channels. Save one before/after frame pair to the plan's commit as reviewer reference.

## Sequencing

**Phase 1 (sequential):** Task 1 — helper must exist before callers refactor.
**Phase 2 (concurrent):** Task 2, Task 3 — parity test and WGSL refactor are independent.
**Phase 3 (sequential):** Task 4 — frame capture validates Task 3.

## Rough sketch

Uniform Catmull-Rom with knots at `t_k = k / count` over a closed loop. Basis matrix per Wikipedia "Cubic Hermite spline § Catmull–Rom spline" (tension 0.5). For `cycle_t ∈ [0, 1)`:

```
let scaled = cycle_t * f32(count);
let i1 = u32(floor(scaled)) % count;
let i0 = (i1 + count - 1u) % count;
let i2 = (i1 + 1u) % count;
let i3 = (i1 + 2u) % count;
let f = fract(scaled);
let p0 = anim_samples[samples_offset + i0];
let p1 = anim_samples[samples_offset + i1];
let p2 = anim_samples[samples_offset + i2];
let p3 = anim_samples[samples_offset + i3];
// Uniform Catmull-Rom basis (tension 0.5)
let a = -0.5*p0 + 1.5*p1 - 1.5*p2 + 0.5*p3;
let b =      p0 - 2.5*p1 + 2.0*p2 - 0.5*p3;
let c = -0.5*p0           + 0.5*p2;
let d =                 p1;
return ((a*f + b)*f + c)*f + d;
```

RGB variant reads three samples per knot at `samples_offset + i*3 + {0,1,2}` and runs the basis per channel.

Reference path (tests only):

```rust
// postretro/src/render/curve_eval_test.rs
use splines::{Interpolation, Key, Spline};

fn reference_scalar(samples: &[f32], cycle_t: f32) -> f32 {
    let n = samples.len();
    // Pad for closed-loop: two before, two after, at uniform spacing.
    let keys: Vec<Key<f32, f32>> = (-2i32..=(n as i32 + 1))
        .map(|k| Key::new(k as f32 / n as f32, samples[k.rem_euclid(n as i32) as usize], Interpolation::CatmullRom))
        .collect();
    Spline::from_vec(keys).clamped_sample(cycle_t).unwrap()
}
```

Key files touched:
- `postretro/src/shaders/curve_eval.wgsl` — new.
- `postretro/src/shaders/forward.wgsl` — delete linear helpers; update call sites.
- `postretro/src/render/sh_volume.rs` — shader-source concatenation update.
- `postretro/src/render/curve_eval_test.rs` — new, `#[cfg(test)]`.
- `postretro/Cargo.toml` — `splines` in `[dev-dependencies]`.

## Spec 2 follow-up

Spec 2's (`animated-light-weight-maps`) WGSL pseudocode shows `sample_curve_catmull_rom(desc.brightness, t)` and `sample_color_catmull_rom(desc.color, t, desc.base_color)`. Those signatures predate this spec. When Spec 2 promotes (or when its compose pipeline lands), its pseudocode and implementation must read:

```
let b = sample_curve_catmull_rom(desc.brightness_offset, desc.brightness_count, t);
let c = sample_color_catmull_rom(desc.color_offset, desc.color_count, t, desc.base_color);
```

Flag captured here so the Spec 2 author sees it before implementation starts.

## Open questions

None.
