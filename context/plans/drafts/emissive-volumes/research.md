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

- **Region over surfaces:** `lightmap_scale_region` peels one brush to a world AABB plus its hull planes, but selection tests only the chart's first vertex against the AABB; last-defined wins. `fog_volume` (non-axis-aligned) and `lightmap_scale_region` refuse more than one brush; environment volumes keep one convex leaf per brush. `reflection_volume` and environment volumes resolve overlaps smallest-wins.
- **Scripting:** `FogVolumeHandle extends AnimatableScalar<"density">`, the `setFog*` primitives and `setFogAnimation` semantics (`scripting.md` §10.2), and the dev script `content/dev/scripts/fog-pulse-demo.ts`.
- **Co-op:** fog presentation binding by record index plus a 1 cm origin check (`networking.md` §Presentation state).
- **Photosensitivity:** `render-cpu` `flash_clamp`: `ValueClamp` is public and per value; the keyed container is private and keys `ValueClamp` only in tests; the ghost look clamp keys `LookValueClamp`. `E23--photosensitivity-source-floor` lists emissive animation as a clamp site and leaves a shared cross-source budget as an open question.
- **Neon surface depth:** `Material::Neon` is `SurfaceDepth::FLAT` (pinned flat-by-intent in `render-cpu` surface_depth tests; `docs/level_design.md` calls it deliberate). Default carves.

## Ordering pins

| id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Looping `flicker` installed at `levelLoad`; trigger fires `setEmissiveLevel` 0 | animation installed, then level set, later tick | Level write clears the animation; 0 holds on every machine. |
| P2 | One body: `setEmissiveAnimation` then `setEmissiveLevel` | same drain, body order | Level wins and clears the curve. |
| P3 | One body: `setEmissiveLevel` then `setEmissiveAnimation` | same drain, body order | Curve wins from the first bridge tick that sees it. |
| P4 | Finite `fade` running; `setEmissiveLevel` 0.5 lands mid-fade | level during animation; fade would then end | 0.5 holds; the fade is gone and its settle never writes. |
| P5 | `setEmissiveAnimation` args `null` mid-flicker | clear, then render | Level holds the last presented sample; the host's sample replicates. |
| P6 | Identical animation re-installed while it runs | install, install again | Start time kept (component equality); a running `fade` does not restart. After a finite one settles, re-install restarts it. |
| P7 | `periodMs` 0, `flicker` `rate` 0 (period ∞), empty `level` curve | install | Rejected with a warning; any prior animation and level untouched. |
| P8 | `playCount` 0 | install | Coerced to 1 with a warning; settles once. |
| P9 | `fade` with `from == to` | install, wait `periodMs` | Flat curve; settles on that value; no flash counted. |
| P10 | Level NaN, -0.5, 1.5; a curve sample 1.5 | set / install | NaN reads 0 (fog `glow` precedent); others clamp to [0, 1]; one warning per write. |
| P11 | Tag matches 0 volumes; tag matches a non-emissive entity | dispatch | 0 volumes: debug no-op. Non-emissive target: warn, skip that target, others still written. |
| P12 | Level written by a reaction drained in frame N | Game logic drain → RenderPrep bridge tick → Render | Packed into frame N's strength table, not N+1. |
| P13 | Script time frozen (pause) mid-flicker | freeze, keep presenting | Level holds its sample (bridge samples the freeze-aware script clock); flash clamp keeps aging on presented-frame time. |
| P14 | Limiter on; flicker at `rate` 6 | each presented frame | Presented level shows ≤ 3 flashes per 1 s window and moves ≤ 4.0/s; scripted level (scripts, wire) unchanged. |
| P15 | Limiter turned on mid-flicker; turned off mid-hold | toggle frame | On: that frame presents unchanged, history starts fresh. Off: scripted level presents the same frame, no history kept. |
| P16 | Volume out of view for 2 s, then in view | limit every frame | Clamp limits every volume every frame, culled or not; turning to face it counts no appearance. |
| P17 | Surface acquire fails on one frame | begin → acquire fail → next frame | Every volume's clamp returns to where the frame began; its time merges into the next frame. |
| P18 | Level A volume 0 flickering; change to level B, volume 0 static | unload → install | Clamp history, levels, animations cleared at teardown; B's volume 0 starts fresh at level 1. |
| P19 | Host `restartLevel` after a host-only `setEmissiveLevel` 0 | unload → reinstall → `levelLoad` | Host and client both present max after reinstall; the old life's override never binds (epoch, level life). |
| P20 | `levelLoad` sequence `wait 2000` then `setEmissiveLevel` 0 | host-timeline tail | Host lands it and records an override; client never lands it, receives it as replicated state. |
| P21 | Late joiner; host's 2 s `fade` began 1 s before the join | join mid-animation | Client rebases on host `script_time`; fade settles when the host's does. |
| P22 | Late joiner; host's `fade` ended before the join | join after settle | Client presents the last sample from its first world frame; no replay. |
| P23 | Override for fog record 0 vs emissive volume 0 | client binding | Kind is part of the key; a fog record never binds an emissive volume, and vice versa. |
| P24 | Client demoted, then re-promoted, host level changed between | demote → host write → promote | Demotion unbinds and keeps the client's last applied level; re-promotion baseline brings the host's current level. |
| P25 | Host-only write to all 256 volumes; late joiner | join baseline | Records ride the 16 KiB baseline budget across snapshots; all 256 arrive; bound presentation records go last. |
| P26 | Host-only `setEmissiveAnimation` with 65 samples | capture | Host presents it; record cannot encode (64-key wire cap), host warns once, client keeps the last valid override. |
| P27 | Face lies in the plane of a volume brush's side (volume flush on a wall) | compile membership | Joins; containment is inclusive (`dot ≤ d`), within compile tolerance. |
| P28 | Earlier volume whose every face a later volume takes | compile | Later wins each face; earlier keeps its record and index (co-op binding), lists 0 faces; warnings per F6. |
| P29 | Face cut into sub-faces by the atlas | compile | Every sub-face carries the parent's membership; section indexes final Geometry faces. |

## Capture reference state

Captures shade a reference state. An animated light enters at the cycle mean of its authored KVP curve (`resource_management.md` §5). Script curves are never baked (`scripting.md` §2). A volume has no authored curve, so its reference state is its no-script level, max.

## Ownership rival

Glow keys could sit on the brush entity that owns the faces: `func_group` already carries `_reflection_capture` for its own faces, and `switch` compiles as world. Membership would be exact, with no centroid, overlap or split-face rule. Not chosen:
- TrenchBroom groups and layers drop custom keys on save.
- Ownership forces moving existing worldspawn brushes into an entity.
- A volume retrofits glow over worldspawn and groups untouched, as `lightmap_scale_region` and `reflection_volume` already select faces.

Overlap rule rival: smallest containing volume wins (`reflection_volume`, environment volumes), so nesting needs no warning. Last-wins kept: both emissive volumes and `lightmap_scale_region` override a surface property, not a region's physics.
