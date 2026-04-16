# Sub-plan 3 — Direct Lighting + Ambient Floor

> **Parent plan:** [Lighting Foundation](./index.md) — read first for goals.
> **Scope:** Upload map lights to GPU, evaluate per-fragment direct lighting via a flat light loop, add an ambient floor uniform. No clustered binning, no shadow maps, no normal maps — those are subsequent sub-plans.
> **Crates touched:** `postretro` only.
> **Depends on:** sub-plan 1 (`MapData.lights` populated with map lights).
> **Blocks:** sub-plan 4 (light influence volumes optimize the light loop built here), sub-plan 5 (shadow maps modulate the direct term built here), sub-plan 6 (normal maps perturb the shading normal used here).

---

## Description

Replace flat ambient with per-fragment direct light evaluation. Upload map lights to a GPU storage buffer, loop over all lights per fragment, evaluate Lambert diffuse with per-type attenuation. Add a uniform ambient floor to prevent pitch-black unlit areas.

This sub-plan uses a **flat per-fragment loop** over all active lights — not clustered forward+. With 3–10 lights on test maps, a flat loop produces identical pixels to a clustered approach and is far simpler to build, debug, and validate. The plan targets up to **500 authored lights per level**; sub-plan 4 adds per-light influence-volume early-outs so the per-fragment cost scales with nearby lights rather than total lights. Clustered forward+ binning is a future optimization if profiling shows the flat loop + influence-volume combination bottlenecks at high visible-light counts; the fragment shader's light evaluation code (falloff, cone attenuation, Lambert) carries over unchanged.

---

## Light buffer

Storage buffer containing packed light structs, uploaded once at level load. One entry per `MapLight`. Animation data is not uploaded here — animated lights contribute to indirect (SH) only; direct evaluation uses the light's static base properties.

**Load path:** compiler (sub-plan 1) writes `MapLight` records to the AlphaLights PRL section (ID 18). The engine parses that section at level load, deserializes the flat record array into `Vec<MapLight>`, and immediately converts each `MapLight` to a `GpuLight` for upload. `MapData` (the compiler's in-memory intermediate) is not accessible to the engine; the PRL section is the sole source of light data at runtime.

### GPU light struct

```
struct GpuLight {                         // 80 bytes, 16-byte aligned (5 × vec4<f32>)
    position_and_type: vec4<f32>,         // xyz = world position, w = bitcast light_type (0=Point, 1=Spot, 2=Directional)
    color_and_falloff_model: vec4<f32>,   // xyz = linear RGB × intensity (pre-multiplied), w = bitcast falloff_model (0=Linear, 1=InverseDistance, 2=InverseSquared)
    direction_and_range: vec4<f32>,       // xyz = normalized direction (Spot/Directional), w = falloff_range (meters)
    cone_angles_and_pad: vec4<f32>,       // x = cone_angle_inner (radians), y = cone_angle_outer (radians), zw = unused
    shadow_info: vec4<f32>,               // reserved for sub-plan 4 (shadow maps); zero-initialized here
}
```

Pack `color × intensity` on the CPU at upload time. For `Directional` lights, `position` is unused (direction-only); for `Point` lights, `direction` and `cone_angles` are unused. The shader branches on `light_type`.

### Rust-side upload

Convert each `MapLight` to the GPU struct, pack into a `Vec<u8>`, create a `wgpu::Buffer` with `STORAGE | COPY_DST` usage. Upload via `queue.write_buffer()` at level load. The buffer does not change per frame until transient gameplay lights exist (Milestone 6+).

---

## Bind group changes

The current bind group layout:
- **Group 0:** per-frame uniforms (view_proj + ambient_light, 80 bytes)
- **Group 1:** per-material (base_texture + sampler)

After this sub-plan:
- **Group 0:** per-frame uniforms — extended (see below)
- **Group 1:** per-material — unchanged
- **Group 2:** lighting — **new**, created once at level load

### Group 0 uniform changes

Replace `ambient_light: vec3<f32>` with:

```
struct Uniforms {
    view_proj: mat4x4<f32>,       // 64 bytes (unchanged)
    camera_position: vec3<f32>,   // 12 bytes (new — needed for light-to-fragment vector)
    ambient_floor: f32,           // 4 bytes (new — replaces ambient_light)
    light_count: u32,             // 4 bytes (new — loop bound)
    _padding: vec3<f32>,          // 12 bytes (align to 16)
}
// Total: 96 bytes
```

`ambient_light: vec3<f32>` is removed. It was a placeholder. The ambient floor is a scalar — no color, just a minimum brightness level.

### Group 2 (lighting)

```
@group(2) @binding(0) var<storage, read> lights: array<GpuLight>;
```

Single binding. Created once at level load. Bind group layout is defined alongside the forward pipeline; the bind group itself is created after the light buffer is uploaded.

---

## Fragment shader evaluation

```
// pseudocode — actual WGSL in implementation
var total_light = ambient_floor;

for (var i = 0u; i < light_count; i++) {
    let light = lights[i];
    let light_type = bitcast<u32>(light.position_and_type.w);

    // Light vector and distance
    var L: vec3<f32>;
    var attenuation: f32;

    switch light_type {
        case 0u: { // Point
            let to_light = light.position_and_type.xyz - frag_world_pos;
            let dist = length(to_light);
            L = to_light / dist;
            attenuation = falloff(dist, light);
        }
        case 1u: { // Spot
            let to_light = light.position_and_type.xyz - frag_world_pos;
            let dist = length(to_light);
            L = to_light / dist;
            attenuation = falloff(dist, light) * cone_attenuation(L, light);
        }
        case 2u: { // Directional
            L = -light.direction_and_range.xyz;
            attenuation = 1.0; // no distance falloff
        }
    }

    let NdotL = max(dot(N, L), 0.0);
    total_light += light.color_and_falloff_model.xyz * attenuation * NdotL;
}

let rgb = base_color.rgb * total_light;
```

`N` is the geometric normal (octahedral-decoded from vertex attribute). Normal map perturbation comes in sub-plan 5; until then, the geometric normal is the shading normal.

---

## Falloff models

Evaluated per fragment based on the light's `falloff_model` field:

- **Linear (0):** `max(1.0 - distance / range, 0.0)`
- **InverseDistance (1):** `1.0 / max(distance, 0.001)` — zeroed when `distance > range`
- **InverseSquared (2):** `1.0 / max(distance * distance, 0.001)` — zeroed when `distance > range`

Epsilon values prevent division by zero when a fragment is at the light origin. The upper clamp is intentionally absent for `InverseDistance` and `InverseSquared` — attenuation can exceed 1.0 at close range. This is deliberate: `color × intensity` (pre-multiplied on upload) controls absolute brightness, so the falloff function is just the raw distance curve. A light can be genuinely bright up close without requiring inflated intensity values. The range cutoff is a hard zero — no smooth fade at the edge. This matches `MapLight`'s `falloff_range` semantics and avoids energy-conservation complexity that doesn't serve the retro aesthetic.

---

## Cone attenuation (Spot lights)

`light.direction_and_range.xyz` is the direction the light **points** — the aim direction, from light position outward toward the illuminated area. `-L` is the vector from the light toward the fragment (opposite of `L`, which points from fragment to light), so the dot product measures how closely the fragment falls within the aimed cone.

```
fn cone_attenuation(L: vec3<f32>, light: GpuLight) -> f32 {
    let cos_angle = dot(-L, light.direction_and_range.xyz);
    let cos_inner = cos(light.cone_angles_and_pad.x);
    let cos_outer = cos(light.cone_angles_and_pad.y);
    return smoothstep(cos_outer, cos_inner, cos_angle);
}
```

Full brightness inside the inner cone, smooth falloff to zero at the outer cone edge. `smoothstep` provides a visually clean transition without hard edges.

---

## Ambient floor

A uniform minimum light level added to the lighting sum before albedo multiplication. Prevents pitch-black areas where no lights contribute.

- **Default:** 0.05 (provisional — tune during manual testing with the full pipeline running; the right default is the lowest value where a player can still navigate dark areas)
- **Player-facing setting:** slider 0.0–1.0 in the settings menu
- **Not affected** by shadows, falloff, or any light source — it's a floor, not a light
- **Replaces** `ambient_light: vec3<f32>` in the uniform buffer; the old vec3 white ambient goes away entirely

---

## Acceptance criteria

- [ ] `GpuLight` struct defined in `postretro` with 80-byte layout matching the GPU struct above; `shadow_info` zero-initialized
- [ ] Engine parses AlphaLights PRL section (ID 18) at level load, deserializes into `Vec<MapLight>`, converts to `GpuLight` structs, and uploads to a storage buffer
- [ ] Forward pipeline bind group layout extended: group 2 with light storage buffer
- [ ] Uniforms extended with `camera_position`, `ambient_floor`, `light_count`; old `ambient_light: vec3<f32>` removed
- [ ] Fragment shader evaluates Lambert diffuse per light in a flat loop
- [ ] Point light falloff matches all three `FalloffModel` variants
- [ ] Spot light cone attenuation via smoothstep between inner and outer angles
- [ ] Directional light evaluates without distance attenuation
- [ ] Ambient floor applied as a minimum light level before albedo multiply
- [ ] Ambient floor exposed as a player-facing setting with default 0.05
- [ ] Test maps from sub-plan 1 (which place light entities as part of FGD validation) render with visible, correct illumination and falloff
- [ ] Unlit areas are not pitch-black (ambient floor working)
- [ ] `cargo test -p postretro` passes
- [ ] `cargo clippy -p postretro -- -D warnings` clean

---

## Implementation tasks

1. Define `GpuLight` Rust struct with `bytemuck` derivation for buffer upload. Write conversion from `MapLight`.

2. At level load, parse the AlphaLights PRL section (ID 18) into `Vec<MapLight>`, convert to `Vec<GpuLight>`, and create the light storage buffer and bind group (group 2). If the section is absent, use an empty list and log a warning.

3. Extend `Uniforms` struct: add `camera_position`, `ambient_floor`, `light_count`; remove `ambient_light`. Update `update_view_projection()` (or rename to `update_per_frame_uniforms()`).

4. Update forward pipeline layout to include group 2 bind group layout.

5. Write the fragment shader light loop: per-light-type branching, falloff evaluation, cone attenuation, Lambert diffuse, ambient floor.

6. Wire ambient floor to a player-facing setting (settings menu slider).

7. Validate visually using the test maps from sub-plan 1 — those maps already contain point, spot, and directional light entities placed as part of FGD validation. No new maps needed.

---

## Notes

**Known limitation — animated lights have static direct contribution.** The GPU light buffer (uploaded at level load) uses each light's static base properties. Animation curves are not evaluated at runtime for direct lighting, so a flickering torch will have animated indirect light (SH bounce, sub-plan 7) but a constant direct contribution. The indirect animation alone provides the majority of the visual effect — ambient pulsing and bounce light color shifts. Runtime evaluation of direct light animation curves is a Milestone 6+ follow-up; adding it later is purely additive and requires no rework of the pipeline built here.
