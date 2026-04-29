# Emissive Surfaces

## Goal

Emissive surfaces render at full brightness regardless of scene lighting — no lightmap
or dynamic light modulation. Enables neon signs, tube lights, and light panels that glow
in dark corridors. Completes the "rendering bypass" stub in `resource_management.md §3`.

## Scope

### In scope

- `Material::Emissive` added (prefix `emissive_`). Sole emissive material type — the only
  prefix that triggers the rendering bypass.
- `Material::Neon` emissive flag stripped. `neon_` becomes a pure aesthetic type (shininess,
  future audio behaviors) with no rendering bypass.
- `MaterialUniform` extended with `emissive_intensity: f32` (0.0 = non-emissive; values
  above 0.0 drive the bypass, including above 1.0 for HDR intent ahead of a bloom pass).
- Emissive mask texture (`_e.png` sibling, R8Unorm linear). Per-texel weight: 1.0 = full
  bypass, 0.0 = normal lighting. Fallback: shared 1×1 white texture (entire surface
  emissive). Reuses base_sampler at binding 1.
- Forward shader bypass: `rgb = albedo × mix(total_light, vec3(emissive_intensity), mask.r × clamp(emissive_intensity, 0.0, 1.0))`.
  When `emissive_intensity == 0.0`, output is identical to the current path — no regression.
- Loader validation: `_e.png` dimensions must match diffuse; mismatch falls back to white
  placeholder with a warning.

### Out of scope

- Post-process bloom / glow halo (separate plan; `emissive_intensity > 1.0` is the hook).
- Emissive surfaces contributing to baked or dynamic lighting (radiosity, light extraction).
- Per-material emissive color tint (albedo controls color; mask controls intensity).
- Animated emissive intensity (use animated light entities).
- New FGD entity or TrenchBroom workflow beyond the existing texture naming convention.

## Acceptance criteria

- [ ] `emissive_` prefixed surfaces in a dark room render at full albedo brightness — no
      lightmap darkening visible.
- [ ] An `emissive_sign_01.png` with a companion `emissive_sign_01_e.png` mask shows the
      masked region emissive and the unmasked region normally lit within the same surface.
- [ ] Absence of `_e.png` produces the same visual result as a 1×1 white mask (entire
      surface emissive).
- [ ] `neon_` prefixed surfaces are not emissive — they respond to lightmap and dynamic
      lights like any other non-emissive material.
- [ ] Non-emissive surfaces (all existing material prefixes) produce visually identical
      output to pre-change frames.
- [ ] `MaterialUniform` struct size and alignment remain valid (no WGSL validation errors,
      no padding violations).
- [ ] Dimensions mismatch between `_e.png` and diffuse logs a warning and falls back to
      white, not a crash or silent corruption.

## Tasks

### Task 1: Material system changes

Add `Material::Emissive` with prefix `emissive_` and `emissive: true` in its properties;
`shininess()` returns `0.0` for `Material::Emissive` — emissive surfaces bypass lighting
and don't need specular highlights. Strip `emissive: true` from `Material::Neon`. Extend
`MaterialUniform` in `forward.wgsl` with `emissive_intensity: f32`; extend the matching
CPU-side struct in `render/mod.rs`. If the struct size changes, update `MATERIAL_UNIFORM_SIZE`
in `render/mod.rs` to match. Upload `1.0` for `Material::Emissive`, `0.0` for all others.
Adjust struct padding to maintain alignment. The following tests in `material.rs` test
`Material::Neon`'s emissive behavior and must be updated when `emissive: true` is stripped
from `Neon`:
- `derive_material_maps_neon_prefix_with_emissive`
- `neon_has_emissive`

Post-implementation doc update: update `rendering_pipeline.md §9`'s group 1 bind-group
slot table to add the emissive mask texture at binding 5.

### Task 2: Emissive mask texture loading

At level load, probe for `{name}_e.png` alongside the diffuse load. Load as R8Unorm linear.
`_e.png` must not be sRGB-tagged — reject sRGB-tagged files at load time with a warning and
fall back to the white placeholder, matching the behavior for `_s.png` and `_n.png` siblings.
If the diffuse resolved to a placeholder (missing diffuse), skip the `_e.png` probe entirely
and bind the white placeholder directly. Validate that dimensions match the diffuse texture;
on mismatch log a warning and substitute the shared 1×1 white placeholder. Missing `_e.png`
silently substitutes the same placeholder. Non-emissive material buckets bind the same
shared 1×1 white placeholder at binding 5 — the pipeline layout requires every binding to
be satisfied for every draw call, matching how `spec_texture` (binding 2) and `t_normal`
(binding 4) bind their neutral placeholders for materials that don't use them.

Extend the group 1 `BindGroupLayout` in `render/mod.rs` with a new `BindGroupLayoutEntry`
at binding 5 (texture_2d, float, non-filtering, fragment visibility) alongside the existing
diffuse / sampler / spec / uniform / normal entries. Extend each per-bucket `BindGroup`
assembly with a corresponding `BindGroupEntry` at binding 5 carrying either the loaded
emissive mask view or the shared white placeholder view. The existing `base_sampler` at
binding 1 also samples the emissive mask — no new sampler binding required.

### Task 3: Forward shader emissive bypass

Add `@group(1) @binding(5) var t_emissive: texture_2d<f32>;` to `forward.wgsl`. At the
output site (currently `let rgb = base_color.rgb * total_light`), sample the emissive mask
and blend: `let rgb = base_color.rgb * mix(total_light, vec3(emissive_intensity), emissive_weight)`
where `emissive_weight = textureSample(t_emissive, base_sampler, in.uv).r * clamp(material.emissive_intensity, 0.0, 1.0)`.
The raw `emissive_intensity` (potentially > 1.0) drives the blended `total_light` target;
clamping only applies to the mix weight so that `emissive_weight` stays in [0, 1].

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes `MaterialUniform` layout and the binding
slot contract consumed by Tasks 2 and 3.

**Phase 2 (concurrent):** Task 2, Task 3 — loader and shader changes are independent once
the binding slot is established.

## Rough sketch

`MaterialUniform` currently has `shininess: f32` (4 bytes) at offset 0 followed by
`_pad: vec3<f32>` (12 bytes) at offset 4. Because `vec3<f32>` has 16-byte alignment in the
uniform address space, the struct rounds up to 32 bytes total — `MATERIAL_UNIFORM_SIZE = 32`.
Adding `emissive_intensity: f32` replaces the first 4 bytes of the padding slot, giving
`shininess: f32` + `emissive_intensity: f32` + `_pad: vec2<f32>` + 16 bytes trailing pad
— still 32 bytes, alignment valid, `MATERIAL_UNIFORM_SIZE` unchanged.

Shader formula with `emissive_intensity > 1.0`:
- `emissive_weight = mask.r × clamp(emissive_intensity, 0.0, 1.0)` — blend weight stays [0, 1]
- `effective_target = vec3(emissive_intensity)` — above 1.0 when bloom-ready content authors push it
- `total_with_emissive = mix(total_light, effective_target, emissive_weight)`
- `rgb = base_color.rgb × total_with_emissive`
- At `emissive_intensity = 1.0`: full emissive texels output `base_color × 1.0` (no change from before)
- At `emissive_intensity = 2.0`: full emissive texels output `base_color × 2.0` (over-bright; bloom hook)
- At `emissive_intensity = 0.0`: path is identical to today — zero regression risk

Spec and normal maps already reuse `base_sampler` (binding 1). Emissive mask follows the
same pattern.
