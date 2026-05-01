# Fog Volumes

## Goal

Wire up the partially-built fog volume pass so `env_fog_volume` brush regions render as
localized volumetric haze. Extend the raymarch shader with point-light (omnidirectional)
scatter so dynamic `light` entities — neon signs, colored lamps — create visible in-fog
glow halos. Expose fog volumes as first-class ECS entities so scripts can query and
animate them with the same vocabulary as lights and emitters.

The GPU-side infrastructure (`fog_pass.rs`, `fog_volume.wgsl`, `fog_composite.wgsl`) is
already written but orphaned — no module declarations, no CPU data module, no renderer
integration, no level-loading support.

**Assumes entity-model-foundation is complete** — `MapEntity` PRL section (ID 29),
`worldQuery`, `setComponent`, `getComponent`, `ComponentKind`, `ComponentValue`, and the
`sdk/lib/entities/` vocabulary pattern are all available.

---

## Scope

### In scope

- Create `crates/postretro/src/fx/fog_volume.rs` — GPU-side structs, constants, and pack functions required by the orphaned `fog_pass.rs`
- Declare `pub mod fog_volume;` in `fx/mod.rs` and `pub mod fog_pass;` in `render/mod.rs`
- Add `ComponentKind::FogVolume = 5` and `ComponentValue::FogVolume { density, color, scatter, falloff }` to the entity registry; wire `worldQuery("fog_volume")`, `setComponent`, and `getComponent`
- prl-build: parse `env_fog_volume` brush entities, compute world-space AABBs, write new `FogVolumes` PRL section (ID 30) with per-volume tags + `fog_pixel_scale` header
- `postretro-level-format`: define `SectionId::FogVolumes = 30` and `FogVolumesSection`
- Engine level load: parse FogVolumes section, spawn one FogVolume ECS entity per volume (origin = AABB center, tags from section, FogVolume component); apply `fog_pixel_scale`
- Integrate `FogPass` into `Renderer`: instantiate, collect FogVolume components from ECS each frame to rebuild the GPU volumes buffer, dispatch compute + composite passes; handle resize and surface-format change
- Add dynamic point-light scatter to `fog_volume.wgsl`: new `fog_points` binding (group 6, binding 5) iterates active dynamic omni lights and accumulates in-fog glow
- Add `FogPointLight` struct, `upload_points()` method, and binding-5 slot to `fog_pass.rs`; CPU upload filters dynamic `LightType::Point` lights each frame
- `sdk/lib/entities/fog_volumes.{ts,luau}`: `FogVolumeHandle` wrapper plus `pulseDensity`, `fadeDensity`, `fadeColor` animation helpers (tick-driven, no new engine primitives)
- SDK type definitions updated; type-definition drift test passes
- `docs/scripting-reference.md` coverage for fog volume query and animation

### Out of scope

- `registerEntity` for fog volumes — brush entities cannot be spawned at runtime; only map-placed volumes are supported
- AABB exposed through `ComponentValue` — baked geometry, not runtime-settable; AABB is internal to the FogVolumeComponent Rust struct
- Script-controlled `fog_pixel_scale` changes at runtime
- Static light scatter in fog (static lights already tint fog via SH ambient)
- Shadow maps for point lights (not available in this engine)
- Spot light scatter (already implemented in `fog_volume.wgsl`)
- Phase functions, multi-scattering, or physically-based volumetrics
- Bilinear upscaling of the fog scatter buffer (nearest-neighbor is intentional)

---

## Acceptance criteria

- [ ] `env_fog_volume` brush entity placed in a test scene produces visible pixelated haze when the player walks into the volume.
- [ ] A dynamic `light` entity (`_dynamic 1`) adjacent to an `env_fog_volume` region causes the fog to glow with the light's color — a localised tinted halo distinct from the ambient SH tint. Toggling the light off eliminates the halo.
- [ ] `fog_pixel_scale 1` on `worldspawn` produces full-resolution fog; `fog_pixel_scale 8` produces coarser block-pixel fog. Absent defaults to 4.
- [ ] Level with no `env_fog_volume` entities: `FogPass::active()` returns false (driven by `volume_count == 0`); no compute dispatch or composite blit issues — confirmed by checking GPU timing output with `POSTRETRO_GPU_TIMING=1`.
- [ ] `world.query({ component: "fog_volume" })` returns one handle per `env_fog_volume` entity in the loaded map.
- [ ] `world.query({ component: "fog_volume", tag: "neon_haze" })` returns only volumes tagged `neon_haze` in TrenchBroom via `_tags`.
- [ ] `setComponent(id, { kind: "fog_volume", density: 0.0 })` causes the fog volume to visually disappear; restoring `density` to the authored value makes it visible again.
- [ ] A fog volume with `falloff 1` has visibly lower density at its edges than at its center; setting `falloff 0` on the same volume produces uniform density to the AABB boundary.
- [ ] `pulseDensity`, `fadeDensity`, `fadeColor` SDK helpers visually animate fog parameters when called from a test script's tick handler — confirmed by observing the effect in-engine.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo clippy --workspace -- -D warnings` clean.
- [ ] No new `unsafe`.

---

## Tasks

### Task 1 — CPU module: `fx/fog_volume.rs`

Create `crates/postretro/src/fx/fog_volume.rs`. Define the GPU-side types `fog_pass.rs` imports:

- `FogVolume` — 48 bytes packed; mirrors the WGSL `FogVolume` struct in `fog_volume.wgsl` (fields: `min [f32;3]`, `density f32`, `max [f32;3]`, `falloff f32`, `color [f32;3]`, `scatter f32`). Note: `max` is `max_v` in WGSL to avoid keyword collision.
- `FogSpotLight` — 48 bytes, mirrors existing WGSL `FogSpotLight`.
- `FogPointLight` — 32 bytes: `position [f32;3]`, `range f32`, `color [f32;3]` (pre-multiplied by intensity), `_pad f32`.
- `FogParams` — matches existing WGSL `FogParams`.
- Constants: `MAX_FOG_VOLUMES = 16`, `MAX_FOG_POINT_LIGHTS = 32`, `FOG_VOLUME_SIZE`, `FOG_SPOT_LIGHT_SIZE`, `FOG_POINT_LIGHT_SIZE`, `FOG_PARAMS_SIZE`, `DEFAULT_FOG_STEP_SIZE`.
- Pack functions: `pack_fog_volumes`, `pack_fog_spot_lights`, `pack_fog_point_lights`, `pack_fog_params`.
- `clamp_fog_pixel_scale(scale: u32) -> u32` — clamps to `1..=8`, defaults to 4 on 0.

Register `pub mod fog_volume;` in `crates/postretro/src/fx/mod.rs`.

### Task 2 — PRL section and level loading

**`postretro-level-format` crate:** add `SectionId::FogVolumes = 30`. Create `fog_volumes.rs` with `FogVolumesSection { pixel_scale: u32, volumes: Vec<FogVolumeRecord> }`. `FogVolumeRecord` holds AABB + params + tags: `min [f32;3]`, `density f32`, `max [f32;3]`, `falloff f32`, `color [f32;3]`, `scatter f32`, `tags: Vec<String>`. Implement `to_bytes` / `from_bytes` following the pattern of existing section files (little-endian, `u32` count headers, `u32`-length-prefixed strings — same as `LightTagsSection`).

**Wire format** — ID 30, little-endian:
```
pixel_scale: u32
volume_count: u32
per entry:
  min_x, min_y, min_z: f32       (12 bytes)
  density: f32                   ( 4 bytes)
  max_x, max_y, max_z: f32       (12 bytes)
  falloff: f32                   ( 4 bytes)
  color_r, color_g, color_b: f32 (12 bytes)
  scatter: f32                   ( 4 bytes)
  tag_count: u32                 ( 4 bytes)
  tags: tag_count × u32-length-prefixed UTF-8 strings (tag_len counts bytes, not codepoints)
```
Section is optional. Absent → `pixel_scale = 4, volume_count = 0`.

**prl-build:** after the `env_reverb_zone` pass (same structural pattern for brush-entity AABB extraction), add an `env_fog_volume` pass. For each brush entity with classname `env_fog_volume`: exclude brushes from world geometry (same mechanism as `env_reverb_zone`), compute world-space AABB over all brush faces, parse KVPs (`color` RGB default `1 1 1`, `density` f32 default `0.5`, `falloff` f32 default `1.0`, `scatter` f32 default `0.6`), parse `_tags` KVP (space-delimited, matching the `entity-model-foundation` convention). Warn and skip if volume count would exceed `MAX_FOG_VOLUMES` (= 16, matching the context doc). Read `fog_pixel_scale` from `worldspawn` KVP (u32, default 4, clamp 1–8). Write `FogVolumesSection`.

**Engine (`prl.rs`):** parse the FogVolumes section if present. Populate `LevelWorld` with `fog_volumes: Vec<FogVolumeRecord>` and `fog_pixel_scale: u32`.

**Level load (`main.rs`):** after the existing entity dispatch, iterate `world.fog_volumes`. For each entry, spawn an ECS entity via `let id = registry.try_spawn(Transform { position: (entry.min + entry.max) * 0.5, rotation: Quat::IDENTITY, scale: Vec3::ONE })?;` then `registry.set_tags(id, entry.tags.clone()).ok();` (matching the pattern in `primitives_light.rs`). Attach a `ComponentValue::FogVolume { density: entry.density, color: entry.color, scatter: entry.scatter }` component. Store the AABB in a side-table keyed by `EntityId` (analogous to the KVP side-table from entity-model-foundation Task 1). The renderer reads this side-table when packing the GPU volume buffer.

### Task 3 — `FogVolume` component kind

**`scripting/registry.rs`:** add `ComponentKind::FogVolume = 5` and `ComponentValue::FogVolume { density: f32, color: [f32; 3], scatter: f32, falloff: f32 }`. Add to the `VARIANTS` const array. Implement the `Component` trait for `FogVolumeComponent`.

**`scripting/conv.rs`:** extend `component_kind_from_name` with `"FogVolume" → ComponentKind::FogVolume` (matching PascalCase convention of existing variants). The `worldQuery({ component: "fog_volume" })` filter uses snake_case — this is a separate lookup from the kind name. Extend `FromJs`/`IntoJs` and `FromLua`/`IntoLua` for `ComponentValue::FogVolume`. `setComponent` accepts `density`, `color`, `scatter`, `falloff`; AABB fields are silently ignored if present. `getComponent` returns all four.

**`scripting/primitives.rs`:** add `"fog_volume"` to `worldQuery`'s filter string set. `worldQuery({ component: "fog_volume" })` returns handles with shape `{ id, position, tags, component: { density, color, scatter, falloff } }`. Follow the entity-model-foundation handle-shape convention for return values.

**`scripting/typedef.rs`:** add `"FogVolume"` to the `ComponentKind` union. Regenerate `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau`; type-definition drift test passes.

### Task 4 — Point-light scatter

**`fog_pass.rs`:** add `fog_points_buffer: wgpu::Buffer` sized for `MAX_FOG_POINT_LIGHTS × FOG_POINT_LIGHT_SIZE`. Add `BIND_FOG_POINTS: u32 = 5` binding constant. Add the binding-5 entry to the group-6 BGL (storage buffer, read-only, compute-visible). Rebuild `build_group6` to include it. Add `upload_points(queue: &wgpu::Queue, points: &[FogPointLight])` method.

**`fog_volume.wgsl` — falloff attenuation in `sample_fog_volumes`:** replace the bare `out.density += v.density` line with an edge-distance fade. Semantics: `falloff = 0` → uniform density to the AABB boundary; `falloff = 1` → linear ramp from zero at the face to full at the center; higher values → sharper interior dropoff.

```wgsl
// Proposed design (remove comment after implementation)
let half_ext = (v.max_v - v.min) * 0.5;
let center   = (v.min + v.max_v) * 0.5;
// local_abs: [0..1] per axis, 0 = center, 1 = face
let local_abs = abs(pos - center) / max(half_ext, vec3<f32>(1.0e-6));
// edge_t: 1 at volume center, 0 at nearest face
let edge_t = 1.0 - clamp(max(local_abs.x, max(local_abs.y, local_abs.z)), 0.0, 1.0);
let fade = pow(clamp(edge_t, 0.0, 1.0), v.falloff);
out.density += v.density * fade;
```

Replace the corresponding `out.color` accumulation (`v.color * v.density`) with `v.color * v.density * fade` so the weighted color blend tracks the attenuated density.

**`fog_volume.wgsl` — point-light loop:** add `struct FogPointLight { position: vec3<f32>, range: f32, color: vec3<f32>, _pad: f32 }` and `@group(6) @binding(5) var<storage, read> fog_points: array<FogPointLight>`. In `cs_main`, after the existing spot-light loop, add:

```wgsl
// Proposed design (remove comment after implementation)
let pt_count = arrayLength(&fog_points);
for (var pi: u32 = 0u; pi < pt_count; pi = pi + 1u) {
    let pt = fog_points[pi];
    let to_light = pt.position - pos;
    let dist = length(to_light);
    if dist > pt.range || dist < 1.0e-4 { continue; }
    let atten = clamp(1.0 - dist / pt.range, 0.0, 1.0);
    accum = accum + transmittance * weight * pt.color * atten;
}
```

No shadow map occlusion for point lights.

### Task 5 — Renderer integration

Register `pub mod fog_pass;` in `render/mod.rs`. Add `fog: FogPass` to `Renderer`; construct in `Renderer::new`. Each frame:

1. Query ECS for `ComponentKind::FogVolume` components. For each entity, look up its AABB from the side-table (Task 2). Construct a `FogVolume` GPU entry from `(aabb, component.density, component.color, component.scatter, component.falloff)`. Pass the slice to `fog.upload_volumes(queue, &volumes)`.
2. `fog.upload_params(queue, inv_view_proj, camera_pos, near, far)`.
3. Build the `FogSpotLight` list from active shadow-mapped spots → `fog.upload_spots(queue, &spots)`.
4. Build the `FogPointLight` list from active dynamic `LightType::Point` lights: discard any light whose bounding sphere (`position` ± `falloff_range`) does not intersect at least one fog volume AABB (sphere-AABB test, O(lights × volumes)). Pack survivors as `{ position, range: falloff_range, color: [r×intensity, g×intensity, b×intensity], _pad: 0.0 }`, cap at `MAX_FOG_POINT_LIGHTS` → `fog.upload_points(queue, &points)`.
5. If `fog.active()`: dispatch the raymarch compute pass, then dispatch the composite blit over the forward-rendered surface.

Apply `fog_pixel_scale` at level load: call `fog.set_pixel_scale(device, world.fog_pixel_scale, width, height, depth_view)`.

Handle `Renderer::resize` — call `fog.resize(device, width, height, depth_view)`. Handle surface-format change — call `fog.rebuild_composite_for_format(device, format)`.

### Task 6 — SDK fog volumes module

Create `sdk/lib/entities/fog_volumes.ts` (and `.luau` twin). Define `FogVolumeHandle` — the return type of `world.query({ component: "fog_volume" })` — with fields from Task 3 (`id`, `position`, `tags`, `component`) plus three mutating methods:

- `setDensity(density: number, transitionMs?: number)` — transitions density over time via `setComponent` calls in each tick, using the existing `timeline` / `sequence` utilities from `sdk/lib/util/`.
- `setColor(color: [number, number, number], transitionMs?: number)` — same pattern for color.
- `setScatter(scatter: number)` — instant (no tween needed for this property).

Export animation constructors at module level:
- `pulseDensity(handle, { min, max, period })` — returns a running animation controller that oscillates density sinusoidally each tick. Cancel via the returned controller's `.stop()`.
- `fadeDensity(handle, to: number, durationMs: number)` — one-shot fade to target density.
- `fadeColor(handle, to: [number, number, number], durationMs: number)` — one-shot color transition.

All animation is tick-driven; no new engine primitives are required. Tick callbacks use `registerHandler("tick", ...)` internally.

Wire `fog_volumes` into the SDK prelude: add `export type { FogVolumeHandle } from "./entities/fog_volumes"; export { pulseDensity, fadeDensity, fadeColor } from "./entities/fog_volumes";` to `sdk/lib/index.ts`, then regenerate `sdk/lib/prelude.js` via `cargo run -p postretro-script-compiler -- --prelude --sdk-root sdk/lib --out sdk/lib/prelude.js`. For Luau, add `fog_volumes.luau` to `LUAU_SDK_LIB_BLOCK` in `crates/postretro/src/scripting/typedef.rs` (alongside `entities/lights.luau` and `entities/emitters.luau`).

Add fog volume entries to `docs/scripting-reference.md`: `world.query({ component: "fog_volume" })`, `FogVolumeHandle` methods, animation constructors, relationship to FGD `env_fog_volume` and `_tags`.

---

## Sequencing

**Phase 1 (sequential):** Task 1 — `fx/fog_volume.rs` unblocks everything; `fog_pass.rs` does not compile without it.

**Phase 2 (concurrent):** Task 2 (PRL pipeline) and Task 4 (shader + point-light binding) — no shared files.

**Phase 3 (concurrent):** Task 3 (ComponentKind + scripting) and Task 5 (Renderer integration) — Task 3 touches only `scripting/`; Task 5 touches only `render/`. Both depend on Tasks 1 and 2. Task 5 also depends on Task 4.

**Phase 4 (sequential):** Task 6 (SDK + docs) — depends on Task 3 for the component API shape.

---

## Rough sketch

`fog_pass.rs` is in `render/` but not yet declared as a module in `render/mod.rs` — adding `pub mod fog_pass;` is the minimal step to bring it into the build. The compiler will then surface all missing imports from `crate::fx::fog_volume`, which Task 1 resolves.

The AABB is not in `ComponentValue::FogVolume` because it is baked level geometry. Scripts that need volume bounds for spatial logic should query the AABB side-table via a dedicated primitive (e.g., `getEntityBounds(id) -> { min, max }`). Adding this primitive is deferred (see Decisions).

`FogPointLight.color` is pre-multiplied by intensity on the CPU before upload, matching the `FogSpotLight.color` convention already in `fog_pass.rs`. No additional intensity field in the GPU struct.

The `falloff` field attenuates density at AABB edges — `falloff = 0` gives a hard box boundary; `falloff = 1` gives a linear center-to-edge ramp (the default). The attenuation is applied inside `sample_fog_volumes`, not in the outer scatter accumulation, so the weighted color blend stays consistent with the attenuated density value.

For the `env_fog_volume` FGD, `_tags` works identically to all other FGD entities since entity-model-foundation established it as a universal KVP convention. TrenchBroom authors tag fog volumes with `_tags neon_haze` and scripts filter with `world.query({ component: "fog_volume", tag: "neon_haze" })`.

Static neon lights (`_dynamic 0`) tint fog via SH ambient (baked into the irradiance volume). For localized halos, authors must use `_dynamic 1`. The plan does not change static-light behavior.

---

## Boundary inventory

| Name | Rust | Wire / serde | TS / JS | Luau | FGD KVP |
|---|---|---|---|---|---|
| `ComponentKind::FogVolume` | `ComponentKind::FogVolume` | `"FogVolume"` | `"fog_volume"` | `"fog_volume"` | n/a |
| `ComponentValue::FogVolume` | `ComponentValue::FogVolume { density, color, scatter, falloff }` | `{ kind: "fog_volume", density, color, scatter, falloff }` | same | same | n/a |
| fog volume entity | `env_fog_volume` brush entity | PRL section 30 | `FogVolumeHandle` | `FogVolumeHandle` | `env_fog_volume` |
| fog_pixel_scale | `LevelWorld.fog_pixel_scale: u32` | PRL section 30 header `u32` | n/a | n/a | `fog_pixel_scale` on worldspawn |
| `FogPointLight` | `fx::fog_volume::FogPointLight` | group 6 binding 5 storage buffer | n/a | n/a | n/a |

---

## Wire format

**FogVolumes section (ID 30)** — little-endian throughout. New surface; no existing section mirrors this layout exactly.

```
pixel_scale:  u32          header
volume_count: u32          entry count
--- per entry (variable length due to tags) ---
min_x, min_y, min_z: f32   12 bytes
density: f32                4 bytes
max_x, max_y, max_z: f32   12 bytes
falloff: f32                4 bytes
color_r, color_g, color_b: f32  12 bytes
scatter: f32                4 bytes
tag_count: u32              4 bytes
  tag_len: u32              4 bytes each (byte count, not codepoints)
  tag_utf8: [u8; tag_len]
```

Empty tag list serialises as `tag_count = 0` with no following bytes. Absent section: `pixel_scale` defaults to 4, `volume_count` defaults to 0.

---

## Decisions

- **`getEntityBounds` primitive.** ~~Out of scope here.~~ **Resolved: deferred.** The side-table exists after this plan's implementation; adding `getEntityBounds(id) -> { min: Vec3, max: Vec3 } | null` is a one-task follow-up when a concrete scripting use case arrives. Scope here is sufficient without it.

- **FogVolumes section always vs. conditional.** ~~Decide at implementation time.~~ **Resolved: always emit.** prl-build writes the FogVolumes section even when `volume_count = 0`, so `fog_pixel_scale` on `worldspawn` is always honoured. Section overhead is 8 bytes.

- **Influence-volume pre-culling for point lights.** ~~Leave as "iterate all" until profiling.~~ **Resolved: pre-cull at CPU upload.** Before filling the `FogPointLight` GPU buffer, filter the active dynamic point lights against the union of all fog volume AABBs: skip any light whose sphere (centre + `falloff_range`) does not intersect at least one fog volume AABB. This is O(lights × volumes) on the CPU and eliminates per-step GPU work for lights that cannot possibly illuminate any fog region. Implement in the `upload_points` path in Task 5.

