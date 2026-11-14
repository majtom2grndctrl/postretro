# emissive-volumes — research

Read at eb44499. Derivation behind the brief's Decisions; facts cited by symbol.

## Neon consumers

Every reader of the `neon` prefix or `emissive_strength()`:
- **Prefix table:** `MATERIAL_PREFIXES` and `Material::emissive_strength` (Neon 4.0, all else 0) in `crates/render-data/src/material.rs`. Neon otherwise: `shininess()` 32 (as Default), `surface_depth()` FLAT, `properties().ricochet` unread.
- **Runtime upload:** `MaterialUniformPlan::new` and `build_material_uniform` (`crates/render-cpu/src/material_plan.rs`) pack strength at bytes 4..8 of the 32-byte material uniform. `build_material_bind_group` (`crates/renderer/src/render/material_plan.rs`) is the bind-group build site.
- **Shaders:** `forward.wgsl` `fs_main`, `kinematic_brush.wgsl` and `fluid_surface.wgsl` gate on `emissive_strength > 0`, then add `emissive × emissive_strength`.
- **Material tables:**
  - `level_texture_materials` (`crates/postretro/src/level_load/tasks.rs`)
  - `derive_texture_materials` (`crates/postretro/src/capture/setup.rs`)
  - loader `derive_material_with_warning` (`crates/level-loader/src/prl_loader/geometry.rs`), which warns once per unknown prefix
- **Bake:**
  - `MaterialStats::from_slots` / `mean_from_slots` (`surface_materials/stats.rs`, called from `texture_mips/world.rs`) exclude glow texels only when strength > 0.
  - Reflection capture multiplies the emissive texel by strength (`surface_materials.rs`, `reflection_probes/shading.rs`) and hashes it into `reflection_probes/stage_key.rs`.
- **Pins:**
  - `MATERIAL_PREFIX_NAMES` (xtask `environment_gates.rs`)
  - `neon_emissive_strength_clears_the_bloom_threshold` (`bloom.rs`)
  - shader-mirror and kinematic layout tests
  - `neon_*` fixtures in texture_mips, surface_materials, coloured_bounce_cache and texture_stage tests
- **Content:**
  - `neon/neon_glow_panel` (`_e` peak sRGB 255) and `neon/neon_dim_panel` (peak 136 → 0.985 at 4.0), used by combat-demo and reflection-probes-test. e24-environment-test uses `fluids/water_neon_64` (prefix `water`, no `_e`), unaffected.
  - `fluids/lava_64_e.png` is baked but dark (prefix `lava` → 0); its README anticipates "no glow until it gets a strength".
- **Tools and docs:**
  - `tools/gen_emissive.py` `DEFAULT_CUTOFFS` (neon 0.35, default 0.75)
  - `docs/level_design.md` material tables
  - `resource_management.md` §4.5
- **Models:** skinned models bind `Material::Default`, so model `_e` never glows (`renderer_models.rs`). Unchanged here.

## Per-face strength path

Strength is per material uniform today. No per-face or per-draw scalar exists for static world faces.
- **Precedent:** `animated_block`, a per-vertex u16 in Geometry (id 17) read flat in `forward.wgsl` `vs_main`, indexing a group-4 table. It is stamped by `animated_block_ids.rs`, whose guard requires unshared vertices on block faces.
- **Second stream:** `spec_owner_buffer` rides vertex slot 1 beside the 36-byte world vertex, so per-vertex data can be added without changing the Geometry ABI.
- **Binding budget:** the forward pass is at 16/16 sampled textures. A scalar table costs no texture; a new storage buffer counts against `max_storage_buffers_per_shader_stage`.
- **Movers:** kinematic movers have a per-instance storage record (`kinematic_brush.wgsl` `instances`), the door for mover glow.

## Engine comparison

Coloured emissive texture × HDR scalar is the common model:
- **glTF:** `emissiveTexture` (RGB) × `emissiveFactor` (clamped 0–1) × `KHR_materials_emissive_strength`, a unitless multiplier.
- **Godot:** `emission_texture` per-pixel colour × `emission_energy_multiplier`, often tweened from scripts.
- **Unity:** emission map colour × HDR colour intensity; a per-material setting chooses whether emission feeds baked GI.
- **Material Maker:** the Material node's emissive input × an energy parameter.
- **The outlier is Source:** `$selfillum` masks the base colour by base alpha or `$selfillummask`, tinted by `$selfillumtint`.

The mask model loses the off state (an unlit diffuse) and glow that differs from surface colour. The `_e` × strength model keeps both, and `gen_emissive.py` already gives "glow in the diffuse colour" by default.

Sources:
- https://github.com/KhronosGroup/glTF/blob/master/extensions/2.0/Khronos/KHR_materials_emissive_strength/
- https://docs.godotengine.org/en/4.3/classes/class_basematerial3d.html
- https://docs.unity3d.com/ru/2020.1/Manual/StandardShaderMaterialParameterEmission.html
- https://rodzill4.github.io/material-maker/doc/node_miscellaneous_material.html
- https://developer.valvesoftware.com/wiki/$selfillum

## Range and floor

- Black stays zero glow. A floor above black would light a sign's dark background whenever min > 0 (owner ruling: 0 is the only floor).
- Min and max bound the scripted level, not the texel mapping. Texel colour scales by the current strength, so a white texel at level 1 glows at exactly `emissive_max`.
- Linear-light note for authors: sRGB 128 decodes to about 0.22, not 0.5.

## Precedents

- **Region over surfaces:** `lightmap_scale_region` peels brushes to world AABBs plus source-hull planes; last-defined wins.
- **Scripting:** `FogVolumeHandle extends AnimatableScalar<"density">`, the `setFog*` primitives and `setFogAnimation` semantics (`scripting.md` §10.2), and the dev script `content/dev/scripts/fog-pulse-demo.ts`.
- **Co-op:** fog presentation binding by record index plus a 1 cm origin check (`networking.md` §Presentation state).
- **Photosensitivity:** `render-cpu` `flash_clamp` (`ValueClamp`, keyed per source; ghost look clamp precedent). `E23--photosensitivity-source-floor` lists emissive animation as a clamp site.
