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
- **Models:** skinned models load only their diffuse slot (`ModelDiffuseOnly` in `loaded_texture.rs`), so model `_e` never glows. Unchanged here.

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
- Min and max bound the scripted brightness, not the texel mapping. Texel colour scales by the current strength, so a white texel at brightness 1 glows at exactly `emissive_max`.
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
| P1 | Looping `flicker` installed at `levelLoad`; trigger fires `setEmissiveBrightness` 0 | animation installed, then brightness set, later tick | Brightness write clears the animation; 0 holds on every machine. |
| P2 | One body: `setEmissiveAnimation` then `setEmissiveBrightness` | same drain, body order | Brightness wins and clears the curve. |
| P3 | One body: `setEmissiveBrightness` then `setEmissiveAnimation` | same drain, body order | Curve wins from the first bridge tick that sees it. |
| P4 | Finite `fade` running; `setEmissiveBrightness` 0.5 lands mid-fade | brightness during animation; fade would then end | 0.5 holds; the fade is gone and its settle never writes. |
| P5 | `setEmissiveAnimation` args `null` mid-flicker | clear, then render | Brightness holds the last presented sample; the host's sample replicates. |
| P6 | Identical animation re-installed while it runs | install, install again | Start time kept (component equality); a running `fade` does not restart. After a finite one settles, re-install restarts it. |
| P7 | `periodMs` 0, `flicker` `rate` 0 (period ∞), empty `brightness` curve | install | Rejected with a warning; any prior animation and brightness untouched. |
| P8 | `playCount` 0 | install | Coerced to 1 with a warning; settles once. |
| P9 | `fade` with `from == to` | install, wait `periodMs` | Flat curve; settles on that value; no flash counted. |
| P10 | Brightness NaN, -0.5, 1.5; a curve sample 1.5 | set / install | NaN reads 0 (fog `glow` precedent); others clamp to [0, 1]; one warning per write. |
| P11 | Tag matches 0 volumes; tag matches a non-emissive entity | dispatch | 0 volumes: debug no-op. Non-emissive target: warn, skip that target, others still written. |
| P12 | Brightness written by a reaction drained in frame N | Game logic drain → RenderPrep bridge tick → Render | Packed into frame N's strength table, not N+1. |
| P13 | Script time frozen (pause) mid-flicker | freeze, keep presenting | Brightness holds its sample (bridge samples the freeze-aware script clock); flash clamp keeps aging on presented-frame time. |
| P14 | Limiter on; flicker at `rate` 6 | each presented frame | Presented strength shows ≤ 3 flashes per 1 s window and moves ≤ 4.0/s; scripted brightness (scripts, wire) unchanged. |
| P15 | Limiter turned on mid-flicker; turned off mid-hold | toggle frame | On: that frame presents unchanged, history starts fresh. Off: scripted brightness presents the same frame, no history kept. |
| P16 | Volume out of view for 2 s, then in view | limit every frame | Clamp limits every volume every frame, culled or not; turning to face it counts no appearance. |
| P17 | Surface acquire fails on one frame | begin → acquire fail → next frame | Every volume's clamp returns to where the frame began; its time merges into the next frame. |
| P18 | Map A volume 0 flickering; change to map B, volume 0 static | unload → install | Clamp history, brightness and animations cleared at teardown; B's volume 0 starts fresh at brightness 1. |
| P19 | Host `restartLevel` after a host-only `setEmissiveBrightness` 0 | unload → reinstall → `levelLoad` | Host and client both present max after reinstall; the old life's override never binds (epoch, level life). |
| P20 | `levelLoad` sequence `wait 2000` then `setEmissiveBrightness` 0 | host-timeline tail | Host lands it and records an override; client never lands it, receives it as replicated state. |
| P21 | Late joiner; host's 2 s `fade` began 1 s before the join | join mid-animation | Client rebases on host `script_time`; fade settles when the host's does. |
| P22 | Late joiner; host's `fade` ended before the join | join after settle | Client presents the last sample from its first world frame; no replay. |
| P23 | Override for fog record 0 vs emissive volume 0 | client binding | Kind is part of the key; a fog record never binds an emissive volume, and vice versa. |
| P24 | Client demoted, then re-promoted, host brightness changed between | demote → host write → promote | Demotion unbinds and keeps the client's last applied record, animation included; re-promotion baseline brings the host's current brightness. |
| P25 | Host-only write to all 256 volumes; late joiner | join baseline | Records ride the 16 KiB baseline budget across snapshots; all 256 arrive; bound presentation records go last. |
| P26 | Host-only `setEmissiveAnimation` with 65 samples | capture | Host presents it; record cannot encode (64-key wire cap), host warns once, client keeps the last valid override. |
| P27 | Face lies in the plane of a volume brush's side (volume flush on a wall) | compile membership | Joins; containment is inclusive (`dot ≤ d`), within compile tolerance. |
| P28 | Earlier volume whose every face a later volume takes | compile | Later wins each face; earlier keeps its record and index (co-op binding), lists 0 faces, warns as a volume with no `_e` face (Decisions §Membership), and the pair warns. |
| P29 | Face cut into sub-faces by the atlas | compile | Every sub-face carries the parent's membership; section indexes final Geometry faces. |
| P30 | Running `flicker`; one body: `setEmissiveBrightness` 0, then the same `flicker` again | clear and identical re-install in one drain; bridge never sees the gap | The brightness write cleared the curve, so the re-install is a new animation: it starts from its first sample on host and client. The bridge's cached start time and the capture's `anim_start` do not survive the clear. |
| P31 | Client runs a flicker (its own `levelLoad` copy or a host override); host `powerCut` sets brightness 0 | host write → capture → client SnapshotApply → client bridge tick | The record carries brightness 0 and no animation; the client drops its curve and holds 0 from that frame. |
| P32 | Late joiner after the host's `powerCut`; the example's `levelLoad` `flicker` runs on the joiner too | joiner install runs `levelLoad` pre-wait steps → first accepted baseline applies | Baseline clears the joiner's local flicker; the joiner holds 0. Joiner presents what the host presents. |
| P33 | Limiter on and holding a volume (presented ≠ scripted); `setEmissiveAnimation` args `null` | clear during a clamp hold | Brightness holds the bridge's last scripted sample; the wire carries it; the clamp eases presented strength toward it under its rules. |
| P34 | Limiter on; a reaction writes brightness 0 and 1 alternately six times a second, no animation | static writes, each in its own drain | Presented strength shows ≤ 3 flashes in any 1 s, as the rate-6 `flicker` does (P14). Scripts and the wire see the written values. |
| P35 | Client: a host record and a client-local write on one volume in one frame | SnapshotApply → ScriptDrain → RenderPrep | Local write lands after the record and holds; the client re-applies only when the host's record changes (last-applied memo). |
| P36 | Map with 256 volumes; a face in volume index 255 and a non-member face | load → per-vertex id stream → strength table | Volume 255's face presents volume 255's strength; the non-member presents 1.0. No volume index aliases "no volume". |
| P37 | Limiter on; volume max 20 flickering between brightness 0.2 and 1 (strength 4 to 20, wholly above the bloom threshold) | each presented frame | The HDR rule maps 4 → 0.8 and 20 → 0.95; the swing counts and the strobe is limited. |

## Capture reference state

Captures shade a reference state. An animated light enters at the cycle mean of its authored KVP curve (`resource_management.md` §5). Script curves are never baked (`scripting.md` §2). A volume has no authored curve, so its reference state is its no-script brightness, max.

## Ownership rival

Glow keys could sit on the brush entity that owns the faces: `func_group` already carries `_reflection_capture` for its own faces, and `switch` compiles as world. Membership would be exact, with no centroid, overlap or split-face rule. Not chosen:
- TrenchBroom groups and layers drop custom keys on save.
- Ownership forces moving existing worldspawn brushes into an entity.
- A volume retrofits glow over worldspawn and groups untouched, as `lightmap_scale_region` and `reflection_volume` already select faces.

Overlap rule rival: smallest containing volume wins (`reflection_volume`, environment volumes), so nesting needs no warning. Last-wins kept: both emissive volumes and `lightmap_scale_region` override a surface property, not a region's physics.

## HDR clamp rule

The per-value clamp (`ValueClamp::limit`) clamps its input to [0, 1], and `BLOOM_THRESHOLD` is 1.0. Dividing strength by the threshold saturates everything at or above it, so a strobe between strength 4 and 20 would pass unlimited. s ÷ (s + 1) keeps every HDR step distinct, compresses bright values as the soft-knee tonemap does, and inverts exactly (s = v ÷ (1 − v)). An SDR step from 0 to 1 enters as 0 to 0.5, still a counted flash. Rivals:
- strength ÷ threshold, saturated: ignores HDR strobes
- strength ÷ the volume's max: weighs one step less on brighter volumes

## Compile-time key handling

`index.md` §2 "Designers over structure": load errors are for placements that cannot work. A swapped, negative or non-finite emissive key has an obvious working reading (swap, 0, default), so each warns and compiles. `lightmap_scale_region` fails on a bad scale; it predates this reading and is not changed here.
