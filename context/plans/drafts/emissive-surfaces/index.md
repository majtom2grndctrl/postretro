# Emissive Surfaces

## Goal

Emissive surfaces render at full brightness regardless of scene lighting — no lightmap
or dynamic light modulation. Enables neon signs, tube lights, and light panels that glow
in dark corridors. Completes the "rendering bypass" stub in `resource_management.md §3`.

## Scope

### In scope

- `MaterialUniform` extended with an emissive intensity field (float; 0.0 = non-emissive,
  1.0 = full bypass).
- Emissive mask texture (`_e.png` sibling, R8Unorm linear). Per-texel weight: 1.0 = full
  bypass, 0.0 = normal lighting. Fallback: shared 1×1 white texture (entire surface
  emissive). Reuses base_sampler from group 1.
- Forward shader bypass: `rgb = albedo × mix(total_light, vec3(1.0), mask.r × emissive_intensity)`.
  When `emissive_intensity == 0.0`, output is identical to the current path — no regression.
- `Material::Neon` (prefix `neon_`) is the initial emissive material. Additional prefixes
  added as content demands, with no engine changes required.
- Loader validation: `_e.png` dimensions must match diffuse; mismatch falls back to white
  placeholder with a warning.

### Out of scope

- Post-process bloom / glow halo (separate plan).
- Emissive surfaces contributing to baked or dynamic lighting (radiosity, light extraction).
- Per-material emissive color tint (albedo controls color; mask controls intensity).
- Animated emissive intensity (use animated light entities).
- New FGD entity or TrenchBroom workflow beyond the existing texture naming convention.

## Acceptance criteria

- [ ] `neon_` prefixed surfaces in a dark room render at full albedo brightness — no
      lightmap darkening visible.
- [ ] A `neon_sign_01.png` with a companion `neon_sign_01_e.png` mask shows the masked
      region emissive and the unmasked region normally lit in the same surface.
- [ ] Absence of `_e.png` produces the same visual result as a 1×1 white mask (entire
      surface emissive).
- [ ] Non-emissive surfaces (all existing material prefixes) produce visually identical
      output to pre-change frames.
- [ ] `MaterialUniform` struct size and alignment remain valid (no WGSL validation errors,
      no padding violations).
- [ ] Dimensions mismatch between `_e.png` and diffuse logs a warning and falls back to
      white, not a crash or silent corruption.

## Tasks

### Task 1: MaterialUniform emissive field

Extend `MaterialUniform` in `forward.wgsl` with `emissive_intensity: f32`. Extend the
matching CPU-side struct in `render/mod.rs`. Upload `1.0` for materials where
`Material::properties().emissive == true`, `0.0` otherwise. Adjust struct padding so the
16-byte alignment rule is still satisfied.

### Task 2: Emissive mask texture loading

At level load, probe for `{name}_e.png` alongside the diffuse load. Load as R8Unorm linear.
Validate that dimensions match the diffuse texture; on mismatch log a warning and substitute
the shared 1×1 white placeholder. Missing `_e.png` silently substitutes the same placeholder.
Bind the emissive mask at `@group(1) @binding(5)` in the per-bucket bind group. The existing
`base_sampler` at binding 1 also samples the emissive mask — no new sampler binding required.

### Task 3: Forward shader emissive bypass

Add `@group(1) @binding(5) var t_emissive: texture_2d<f32>;` to `forward.wgsl`. At the
output site (currently `let rgb = base_color.rgb * total_light`), sample the emissive mask
and blend: `let rgb = base_color.rgb * mix(total_light, vec3(1.0), emissive_weight)` where
`emissive_weight = textureSample(t_emissive, base_sampler, in.uv).r * material.emissive_intensity`.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes `MaterialUniform` layout and the binding
slot contract consumed by Tasks 2 and 3.

**Phase 2 (concurrent):** Task 2, Task 3 — loader and shader changes are independent once
the binding slot is established.

## Rough sketch

`MaterialUniform` currently has `shininess: f32` + 12 bytes padding to 16 bytes. Adding
`emissive_intensity: f32` fills 8 bytes of data, leaving 8 bytes padding — valid alignment.

CPU-side: when building the per-bucket material upload in `render/mod.rs`, read
`material.properties().emissive` and set `emissive_intensity` accordingly.

Shader formula derivation:
- `emissive_weight = mask.r × emissive_intensity`  (in [0,1] for valid inputs)
- `total_with_emissive = mix(total_light, vec3(1.0), emissive_weight)`
- `rgb = base_color.rgb × total_with_emissive`
- When `emissive_intensity = 0.0`: `emissive_weight = 0.0` → `rgb = base_color × total_light` (no change)
- When `emissive_intensity = 1.0`, `mask.r = 1.0`: `rgb = base_color × 1.0` (full bypass)
- Mixed: texels with `mask.r = 0.5` blend halfway — useful for soft emissive edges

Spec map and normal map already reuse `base_sampler` (binding 1). Emissive mask follows
the same pattern.

## Open questions

- **Emissive intensity > 1.0?** A value above 1.0 would over-brighten the surface (HDR
  intent). With no bloom pass, values clamped to display range — not useful without a
  bloom plan. Keep to 1.0 for now; revisit when bloom is planned.
- **Additional emissive material prefixes?** `neon_` covers signs and tubes. For light
  panels the mapper could use `neon_panel_...`. A dedicated `panel_` or `emissive_`
  prefix is a content call, not an engine change — no blocker here.
