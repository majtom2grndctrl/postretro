# Sub-plan 7 — Animated SH Layers

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals.
> **Scope:** Runtime loading and evaluation of per-light animated SH layers baked in sub-plan 2. Single packed storage buffer for all per-light SH data, animation descriptor buffer, curve interpolation and manual trilinear interpolation in the fragment shader. Extends the base SH sampling path from sub-plan 6.
> **Crates touched:** `postretro` only.
> **Depends on:** sub-plan 6 (base SH volume sampling must be working — animated layers add to the base SH).
> **Blocks:** nothing. This is the final sub-plan in the lighting foundation.

---

## Description

When the `ShVolume` PRL section contains animated light layers (`animated_light_count > 0`), the loader packs all per-light monochrome SH probe data into a single storage buffer. The fragment shader evaluates each animated light's contribution at the current time, performs manual trilinear interpolation over the probe grid, modulates the monochrome SH by the light's animated color and brightness, and adds it to the base SH before irradiance reconstruction.

When `animated_light_count = 0`, no per-light buffer is created and the shader path is identical to sub-plan 6's static-only case.

---

## Per-light SH storage buffer

All per-light monochrome SH probe data is packed into a single `array<f32>` storage buffer. Each animated light contributes `probe_count * 9` floats, where `probe_count = grid_x * grid_y * grid_z`. The layout is:

```
index = (light_index * probe_count + probe_index) * 9 + band
```

Band ordering matches the base SH convention (L0..L2, 9 coefficients total). The shader performs manual trilinear interpolation at sample time — no GPU sampler involved.

### Upload

After loading base SH probes (sub-plan 6), if `animated_light_count > 0`:
1. Parse the animation descriptor table (one entry per animated light: period, phase, base_color, brightness samples, color samples).
2. Allocate a buffer of `animated_light_count * probe_count * 9 * 4` bytes.
3. For each animated light, write its monochrome SH coefficients into the buffer at the appropriate offset.
4. Upload the completed buffer via `queue.write_buffer()`.
5. Upload animation descriptors to a separate storage buffer.

---

## Animation descriptor buffer

```wgsl
struct AnimationDescriptor {
    period: f32,             // cycle duration in seconds
    phase: f32,              // 0-1 offset within cycle
    base_color: vec3<f32>,   // linear RGB
    brightness_offset: u32,  // index into brightness_samples array
    brightness_count: u32,   // number of brightness samples (0 = no brightness animation)
    color_offset: u32,       // index into color_samples array
    color_count: u32,        // number of color samples (0 = no color animation)
    _padding: vec2<f32>,
}
```

Two storage buffers:
- `animation_descriptors: array<AnimationDescriptor>` — one entry per animated light
- `animation_samples: array<f32>` — packed brightness and color sample data, indexed by offset/count from the descriptor

Color samples are stored as interleaved `[r, g, b, r, g, b, ...]` in the samples array.

---

## Bind group changes

Extend **group 3 (SH volume)** with animated light bindings:

```
// existing from sub-plan 6:
@group(3) @binding(0) var sh_sampler: sampler;
@group(3) @binding(1..N) var sh_texture_*: texture_3d<f32>;  // base SH
@group(3) @binding(N+1) var<uniform> sh_grid: ShGridInfo;

// new for animated lights:
@group(3) @binding(N+2) var<storage, read> anim_descriptors: array<AnimationDescriptor>;
@group(3) @binding(N+3) var<storage, read> anim_samples: array<f32>;
@group(3) @binding(N+4) var<storage, read> anim_sh_data: array<f32>;  // all per-light monochrome SH
```

Animated light bindings are exactly 3 storage buffers regardless of `animated_light_count`. There are no per-light texture bindings.

### Per-frame uniform addition

Add `time: f32` to the per-frame uniforms (group 0). This is the elapsed time in seconds, used for animation curve evaluation. Wrapping is handled per-light in the shader via `fract()`.

---

## Fragment shader: animated SH evaluation

After sampling base SH (sub-plan 6), for each animated light:

```wgsl
// Per animated light (loop 0..animated_light_count)
let desc = anim_descriptors[light_idx];

// 1. Compute cycle position
let t = fract(uniforms.time / desc.period + desc.phase);

// 2. Interpolate brightness curve
var brightness = 1.0;
if (desc.brightness_count > 0u) {
    let sample_pos = t * f32(desc.brightness_count);
    let idx0 = u32(floor(sample_pos)) % desc.brightness_count;
    let idx1 = (idx0 + 1u) % desc.brightness_count;
    let frac = fract(sample_pos);
    brightness = mix(
        anim_samples[desc.brightness_offset + idx0],
        anim_samples[desc.brightness_offset + idx1],
        frac
    );
}

// 3. Interpolate color curve (or use base_color)
var color = desc.base_color;
if (desc.color_count > 0u) {
    let sample_pos = t * f32(desc.color_count);
    let idx0 = u32(floor(sample_pos)) % desc.color_count;
    let idx1 = (idx0 + 1u) % desc.color_count;
    let frac = fract(sample_pos);
    let off0 = desc.color_offset + idx0 * 3u;
    let off1 = desc.color_offset + idx1 * 3u;
    color = mix(
        vec3(anim_samples[off0], anim_samples[off0+1u], anim_samples[off0+2u]),
        vec3(anim_samples[off1], anim_samples[off1+1u], anim_samples[off1+2u]),
        frac
    );
}

// 4. Manual trilinear interpolation of per-light monochrome SH from storage buffer
//    grid_uv is a vec3<f32> in [0,1]^3
let probe_count = sh_grid.grid_x * sh_grid.grid_y * sh_grid.grid_z;
let base_offset = light_idx * probe_count;

let gf = clamp(grid_uv, vec3(0.0), vec3(1.0)) * vec3(
    f32(sh_grid.grid_x) - 1.0,
    f32(sh_grid.grid_y) - 1.0,
    f32(sh_grid.grid_z) - 1.0,
);
let gi = vec3<u32>(floor(gf));
let gfrac = fract(gf);

// 8 corner indices in (x, y, z) order
var corner: array<u32, 8>;
for (var dz = 0u; dz < 2u; dz++) {
    for (var dy = 0u; dy < 2u; dy++) {
        for (var dx = 0u; dx < 2u; dx++) {
            let cx = min(gi.x + dx, sh_grid.grid_x - 1u);
            let cy = min(gi.y + dy, sh_grid.grid_y - 1u);
            let cz = min(gi.z + dz, sh_grid.grid_z - 1u);
            corner[dz * 4u + dy * 2u + dx] =
                base_offset + (cz * sh_grid.grid_y + cy) * sh_grid.grid_x + cx;
        }
    }
}

// Trilinear lerp per band
var mono_sh: array<f32, 9>;
for (var band = 0u; band < 9u; band++) {
    var c: array<f32, 8>;
    for (var i = 0u; i < 8u; i++) {
        c[i] = anim_sh_data[(corner[i]) * 9u + band];
    }
    let c00 = mix(c[0], c[1], gfrac.x);
    let c01 = mix(c[2], c[3], gfrac.x);
    let c10 = mix(c[4], c[5], gfrac.x);
    let c11 = mix(c[6], c[7], gfrac.x);
    let c0  = mix(c00, c01, gfrac.y);
    let c1  = mix(c10, c11, gfrac.y);
    mono_sh[band] = mix(c0, c1, gfrac.z);
}

// 5. Modulate: monochrome × color × brightness → RGB SH contribution
for (var band = 0u; band < 9u; band++) {
    sh_coeffs[band] += mono_sh[band] * color * brightness;
}
```

The modulated SH coefficients are added to the base SH *before* irradiance reconstruction. This is important — the SH dot product with the normal is linear, so `SH(base + anim) · N = SH(base) · N + SH(anim) · N`. Adding before reconstruction is mathematically equivalent and avoids a second irradiance evaluation.

---

## Graceful degradation

| Condition | Behavior |
|-----------|----------|
| `animated_light_count = 0` | No per-light storage buffer created. Shader skips animation loop. Identical to sub-plan 6. |
| SH section missing entirely | No SH textures at all. Indirect = 0. Same as sub-plan 6 degradation. |
| Animation with `brightness_count = 0` | Brightness stays 1.0 (constant). Only color animation applies. |
| Animation with `color_count = 0` | Color stays `base_color`. Only brightness animation applies. |

---

## Acceptance criteria

- [ ] Per-light monochrome SH layers packed into a single `anim_sh_data` storage buffer when `animated_light_count > 0`
- [ ] Animation descriptor table and sample arrays uploaded to storage buffers
- [ ] `time: f32` added to per-frame uniforms
- [ ] Fragment shader evaluates brightness and color curves per animated light via linear interpolation with wrapping
- [ ] Monochrome SH modulated by `color × brightness` and added to base SH before irradiance reconstruction
- [ ] Zero animated lights degrades to static-only SH path (no per-light buffer, no animation loop)
- [ ] Animated lights visually pulse/flicker on test maps matching their `LightAnimation` curves
- [ ] Phase offsets produce visible desynchronization between lights with identical animation presets
- [ ] `cargo test -p postretro` passes
- [ ] `cargo clippy -p postretro -- -D warnings` clean

---

## Implementation tasks

1. Extend SH volume loader: parse animation descriptor table and per-light monochrome SH layers from the PRL section.

2. Allocate `anim_sh_data` storage buffer (`animated_light_count * probe_count * 9 * 4` bytes) and write all per-light SH coefficients via `queue.write_buffer()`.

3. Upload animation descriptors and sample arrays to storage buffers.

4. Extend group 3 bind group with `anim_descriptors`, `anim_samples`, and `anim_sh_data` storage buffers.

5. Add `time: f32` to per-frame uniforms. Update CPU-side uniform upload to write elapsed time each frame.

6. Fragment shader: implement animation curve evaluation (brightness + color interpolation with wrapping), manual trilinear interpolation of monochrome SH from `anim_sh_data`, modulation, and addition to base SH.

7. Validate on test maps: author lights with animation presets, verify visual pulsing matches expected curves, verify phase offsets desynchronize identical presets.

---

## Notes for implementation

- **Linear interpolation with wrapping.** The `% count` wrapping in the sample index produces seamless looping. The brightness and color sample arrays are uniformly spaced over the period — no explicit timestamps needed.
- **SH linearity.** The key mathematical property enabling this approach: SH irradiance reconstruction is a linear operation. Adding modulated monochrome SH to the base before reconstruction is equivalent to reconstructing each separately and summing. One reconstruction pass, not `1 + animated_light_count`.
- **Memory per animated light.** Storage buffer at 60×60×20 grid: `probe_count * 9 * 4 = 72000 * 9 * 4 = 2.59 MB` per light (f32 vs f16, so slightly more than a texture approach). Five animated lights: `5 * 72000 * 9 * 4 = 12.96 MB`. No binding explosion — always 3 storage buffer bindings regardless of light count.
- **No per-light branching on light type.** All animated lights are treated identically in the SH layer — the monochrome SH captures the light's spatial contribution regardless of whether it's a point, spot, or directional source. The baker handles the light-type differences; the runtime just modulates and sums.
- **Design note — indirect animation only; direct stays static.** This sub-plan animates the indirect (SH bounce) contribution of each light. The direct lighting term (sub-plan 3) uses static base properties uploaded at level load and does not evaluate animation curves at runtime. A flickering torch therefore has animated bounce light but a constant direct contribution. The indirect animation provides the majority of the visual effect. Runtime direct light animation is a Milestone 6+ follow-up and is purely additive — it requires no changes to the pipeline built here.
