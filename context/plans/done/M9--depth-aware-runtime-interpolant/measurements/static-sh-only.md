# M9 Depth-Aware Runtime Interpolant - StaticSHOnly Measurement

> **Status: VISUALLY CONFIRMED (2026-05-23).** The through-wall indirect-light
> bleed the depth-aware interpolant targets is gone in an interactive GPU run,
> with no over-occlusion blackout on the occluded side. Non-visual coverage
> (below) was already green; this note records the interactive confirmation that
> was the last open gate.

## Visual Gate

- **Map:** `content/dev/maps/occlusion-test.map` / `content/dev/maps/occlusion-test.prl`
- **Camera pose:** `pos (-23, 2, -34)` (occlusion-test divider-wall room, HUD readout)
- **Diagnostics mode:** `LightingIsolation::StaticSHOnly = 6` (also cross-checked in `Normal = 0`)
- **After worktree:** `6c27de8` plus M9 runtime-interpolant changes (this branch)

### Result

- The exterior shadow pattern that previously bled through the small building's
  thin divider wall onto the interior face is no longer present — the depth
  moments now exclude far-side probe contributions across the wall.
- The occluded interior face does **not** black out: it retains soft, even fill,
  confirming `SH_DEPTH_MIN_VISIBILITY = 0.03` keeps a residual floor rather than
  driving fully-occluded samples to zero.
- Residual brightness on faces with no direct light is attributable to the flat
  `ambient_floor` term (`forward.wgsl:515`) plus legitimate SH bounce, **not** to
  through-wall leak. Confirmed by comparing `AmbientOnly = 4` (flat floor alone)
  against `StaticSHOnly = 6` (floor + SH): the SH delta on the occluded face is
  small, i.e. the depth term is suppressing the cross-wall contribution.

### Term-is-load-bearing check

Temporarily set exaggerated over-occlusion constants
(`SH_DEPTH_MIN_VARIANCE_M2 = 1.0e-7`, `SH_DEPTH_BIAS_CELL_FRACTION = 0.0`,
`SH_DEPTH_MIN_VISIBILITY = 0.0`) and observed the interior collapse to near-black
with hard self-occlusion acne, then reverted to the shipping values
(`1.0e-4` / `0.05` / `0.03`). This confirms the Chebyshev visibility term is
active and materially shaping the result, not a no-op.

## Non-Visual Coverage (already green)

- `cargo check -p postretro`
- `cargo test -p postretro wgsl_passes_naga_validation`
- `cargo test -p postretro depth_moments`
- `cargo test -p postretro chebyshev_visibility_reference`

These cover CPU packing of valid/invalid probe depth moments into RG f16 bits,
the missing-SH dummy payload and disabled grid flag, Rust group-3 layout
agreement with the shader, forward/billboard/fog WGSL parse/validation after
concatenating `sh_sample.wgsl`, and the CPU reference behavior of the Chebyshev
term (full visibility before mean+bias, smooth attenuation after, finite
zero-variance handling, invalid-probe zero contribution).
