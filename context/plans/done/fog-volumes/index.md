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

**Assumes entity-model-foundation is complete** — `MapEntity` PRL section,
`worldQuery`, `setComponent`, `getComponent`, `ComponentKind`, `ComponentValue`, and the
`sdk/lib/entities/` vocabulary pattern are all available.

---

## Scope

### In scope

- Create `crates/postretro/src/fx/fog_volume.rs` — GPU-side structs, constants, and pack functions required by the orphaned `fog_pass.rs`
- Declare `pub mod fog_volume;` in `fx/mod.rs` and `pub mod fog_pass;` in `render/mod.rs`
- Add `ComponentKind::FogVolume = 5` and `ComponentValue::FogVolume { density, color, scatter, falloff }` to the entity registry; wire `worldQuery("fog_volume")`, `setComponent`, and `getComponent`
- prl-build: parse `env_fog_volume` brush entities, compute world-space AABBs, write new `FogVolumes` PRL section with per-volume tags + `fog_pixel_scale` header
- `postretro-level-format`: define `SectionId::FogVolumes` (next available ID after entity-model-foundation's MapEntity section) and `FogVolumesSection`
- `sdk/TrenchBroom/postretro.fgd`: declare `@SolidClass = env_fog_volume` with `color`, `density`, `falloff`, `scatter`, `_tags` keys; correct `fog_pixel_scale` on worldspawn to `integer` type with default `"4"`
- Fix stale `` `_tag` `` singular in `LightTagsSection` rustdoc (`` `crates/level-format/src/lib.rs:127` ``)
- Engine level load: parse FogVolumes section, spawn one FogVolume ECS entity per volume (origin = AABB center, tags from section, FogVolume component); apply `fog_pixel_scale`
- Integrate `FogPass` into `Renderer`: instantiate, dispatch compute + composite passes, handle resize and surface-format change. The renderer never queries the ECS or owns the AABB side-table — a game-side `FogVolumeBridge` (mirroring the existing `LightBridge` / `ParticleRenderCollector` pattern) walks the registry, packs `FogVolume` GPU bytes, and uploads via a new `Renderer::upload_fog_volumes(&[u8])` method between Game logic and Render (preserving the rendering-pipeline §9 boundary rule)
- Add dynamic point-light scatter to `fog_volume.wgsl`: new `fog_points` binding (group 6, binding 5) iterates active dynamic omni lights and accumulates in-fog glow
- Add `FogPointLight` struct, `upload_points()` method, and binding-5 slot to `fog_pass.rs`; CPU upload filters dynamic `LightType::Point` lights each frame
- `sdk/lib/entities/fog_volumes.{ts,luau}`: `FogVolumeHandle` wrapper plus `pulseDensity` animation helper and tween-capable `setDensity` / `setColor` setters (tick-driven, no new engine primitives)
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
- [ ] A dynamic `light` entity (`_dynamic 1`) adjacent to an `env_fog_volume` region causes the fog to glow with the light's color — a localised tinted halo distinct from the ambient SH tint. Setting the light's intensity to 0 via `setComponent(id, { kind: "light", intensity: 0 })` eliminates the halo.
- [ ] `fog_pixel_scale 1` on `worldspawn` produces full-resolution fog; `fog_pixel_scale 8` produces coarser block-pixel fog. Absent defaults to 4.
- [ ] Level with no `env_fog_volume` entities: `FogPass::active()` returns false (driven by `volume_count == 0`); no compute dispatch or composite blit issues — confirmed by checking GPU timing output with `POSTRETRO_GPU_TIMING=1`.
- [ ] `world.query({ component: "fog_volume" })` returns one handle per `env_fog_volume` entity in the loaded map.
- [ ] `world.query({ component: "fog_volume", tag: "neon_haze" })` returns only volumes tagged `neon_haze` in TrenchBroom via `_tags`.
- [ ] `setComponent(id, { kind: "fog_volume", density: 0.0 })` causes the fog volume to visually disappear; restoring `density` to the authored value makes it visible again.
- [ ] A fog volume with `falloff 1` has visibly lower density at its edges than at its center; setting `falloff 0` on the same volume produces uniform density to the AABB boundary.
- [ ] A fog volume compiled with `height_gradient 1` (set in TrenchBroom and prl-built) is visibly denser at the bottom of the volume (min.y) and fades toward the top (max.y). A second test volume compiled with `height_gradient 0` shows uniform vertical density.
- [ ] A fog volume compiled with `radial_falloff 1` produces a sphere-shaped fog cloud centred on the AABB, visibly thinner toward the corners. A second test volume compiled with `radial_falloff 0` shows box-only falloff behavior.
- [ ] `pulseDensity` oscillates density visually when called from a test script. `setDensity(to, durationMs)` and `setColor(to, durationMs)` produce a smooth tween when called with a nonzero `transitionMs` — confirmed by observing the effect in-engine.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo clippy --workspace -- -D warnings` clean.
- [ ] No new `unsafe`.

---

## Tasks

### Task 1 — CPU module: `fx/fog_volume.rs`

Create `crates/postretro/src/fx/fog_volume.rs`. Define the GPU-side types `fog_pass.rs` imports:

- `FogVolume` — 64 bytes packed; defines the canonical 64-byte layout; Task 4 extends the WGSL `FogVolume` struct to match (fields: `min [f32;3]`, `density f32`, `max [f32;3]`, `falloff f32`, `color [f32;3]`, `scatter f32`, `height_gradient f32`, `radial_falloff f32`, `_pad0 f32`, `_pad1 f32`). Note: `max` is `max_v` in WGSL to avoid keyword collision. Layout: four 16-byte rows — `(min, density)`, `(max_v, falloff)`, `(color, scatter)`, `(height_gradient, radial_falloff, _pad0, _pad1)`.
- `FogSpotLight` — 48 bytes, mirrors existing WGSL `FogSpotLight`.
- `FogPointLight` — 32 bytes: `position [f32;3]`, `range f32`, `color [f32;3]` (pre-multiplied by intensity), `_pad f32`.
- `FogParams` — matches existing WGSL `FogParams`.
- Constants: `MAX_FOG_VOLUMES = 16`, `MAX_FOG_POINT_LIGHTS = 32`, `FOG_VOLUME_SIZE`, `FOG_SPOT_LIGHT_SIZE`, `FOG_POINT_LIGHT_SIZE`, `FOG_PARAMS_SIZE`, `DEFAULT_FOG_STEP_SIZE`.
- Pack functions: `pack_fog_volumes`, `pack_fog_spot_lights`, `pack_fog_point_lights`, `pack_fog_params`.
- All GPU structs (`FogVolume`, `FogSpotLight`, `FogPointLight`, `FogParams`) must be `#[repr(C)]` and derive `bytemuck::Pod + bytemuck::Zeroable`, matching the existing convention in `fog_pass.rs`.
- `clamp_fog_pixel_scale(scale: u32) -> u32` — clamps to `1..=8`, defaults to 4 on 0.

Register `pub mod fog_volume;` in `crates/postretro/src/fx/mod.rs`.

**Tests:** Add unit tests in `fx/fog_volume.rs`:
- `clamp_fog_pixel_scale(0)` returns 4; `clamp_fog_pixel_scale(1)` returns 1; `clamp_fog_pixel_scale(8)` returns 8; `clamp_fog_pixel_scale(9)` returns 8.
- `pack_fog_volumes` with one entry round-trips through `FogVolume` byte layout correctly (spot-check `density` offset = 12, `falloff` offset = 28).

### Task 2 — PRL section and level loading

**`postretro-level-format` crate:** add `SectionId::FogVolumes` (assign the next available discriminant at implementation time). Create `fog_volumes.rs` with `FogVolumesSection { pixel_scale: u32, volumes: Vec<FogVolumeRecord> }`. `FogVolumeRecord` holds AABB + params + tags: `min [f32;3]`, `density f32`, `max [f32;3]`, `falloff f32`, `color [f32;3]`, `scatter f32`, `height_gradient f32`, `radial_falloff f32`, `tags: Vec<String>`. Implement `to_bytes` / `from_bytes` following the pattern of existing section files (little-endian, `u32` count headers, `u32`-length-prefixed strings — same as `LightTagsSection`). Also update `SectionId::from_u32` (in `crates/level-format/src/lib.rs`) to map the new discriminant to `SectionId::FogVolumes`.

**Tests:** Add a round-trip test in `level-format/src/fog_volumes.rs`: serialize a `FogVolumesSection` with two entries (one with tags, one without), deserialize, assert field equality. Follow the pattern of existing section tests.

**Wire format** — little-endian:
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
  height_gradient: f32           ( 4 bytes)
  radial_falloff: f32            ( 4 bytes)
  tag_count: u32                 ( 4 bytes)
  tags: tag_count × u32-length-prefixed UTF-8 strings (tag_len counts bytes, not codepoints)
```
Section is optional. Absent → `pixel_scale = 4, volume_count = 0`.

**prl-build:** add an `env_fog_volume` pass following the brush-entity AABB extraction pattern (same structure as any future `env_reverb_zone` pass would use — iterate brush entities of the target classname, collect all face vertices, compute world-space min/max). For each brush entity with classname `env_fog_volume`: exclude fog-volume brushes from the worldspawn geometry collection: skip any brush entity whose classname is `env_fog_volume` during the brush collection pass so its faces are not included in BSP construction or world geometry output, compute world-space AABB over all brush faces, parse KVPs (`color` RGB default `"255 255 255"` — parsed as three integers 0..=255 and divided by 255.0 to linear float, matching the existing light entity color parser; no sRGB curve applied; `density` f32 default `0.5`, `falloff` f32 default `1.0`, `scatter` f32 default `0.6`, `height_gradient` f32 default `0.0`, `radial_falloff` f32 default `0.0`), parse `_tags` KVP (plural, space-delimited — the universal per-entity tag convention established by `entity-model-foundation` and already used by the `quake_map.rs` reader at the `_tags` key). Warn and skip if volume count would exceed `MAX_FOG_VOLUMES` (= 16, matching the context doc). Read `fog_pixel_scale` from `worldspawn` KVP (u32, default 4, clamp 1–8). Write `FogVolumesSection`.

**Tag KVP convention — `_tags` is canonical; `_tag` rustdoc is stale.** The universal per-entity KVP key is `_tags` (plural, space-delimited). This is what `entity-model-foundation` formalises and what the existing reader in `crates/level-compiler/src/format/quake_map.rs` already parses (see the `_tags` literal at `quake_map.rs:392`, plus the round-trip tests at `quake_map.rs:1352`, `:1364`, `:1387`). The `LightTagsSection` (PRL section 26) is sourced from this same `_tags` FGD key — only the rustdoc comment on `LightTagsSection` in `crates/level-format/src/lib.rs:127` mentions `_tag` (singular), and that comment is stale. As a small bookkeeping fix, this plan corrects that single-line rustdoc to read `_tags` so future readers do not mis-cite it as a competing convention. No new tag KVP key is introduced; fog volumes use `_tags` everywhere — FGD, prl-build parser, PRL section, ECS tag column, scripting `worldQuery({ tag: ... })` — exactly as lights and any other entity do.

**`sdk/TrenchBroom/postretro.fgd`:** declare `@SolidClass = env_fog_volume : "Per-region volumetric fog" [ color(color255) : "Fog color (R G B)" : "255 255 255", density(float) : "Fog density" : "0.5", falloff(float) : "Edge fade (0=hard, 1=linear ramp)" : "1.0", scatter(float) : "Scatter fraction toward camera" : "0.6", height_gradient(float) : "Height density gradient (0=uniform, 1=dense at bottom)" : "0.0", radial_falloff(float) : "Radial density falloff (0=none, 1=sphere-shaped cloud)" : "0.0", _tags(string) : "Space-delimited tags for script queries" : "" ]`. Also correct the existing `fog_pixel_scale` worldspawn key from `float` type with default `"1.0"` to `integer` type with default `"4"`, and update the key description from `"Fog density scale"` to `"Fog low-resolution downscale factor (1=full-res, 8=coarsest)"`.

**Engine (`prl.rs`):** parse the FogVolumes section if present. Populate `LevelWorld` with `fog_volumes: Vec<FogVolumeRecord>` and `fog_pixel_scale: u32`.

**Level load (`main.rs`):** after the existing entity dispatch, call `self.fog_volume_bridge.populate_from_level(&mut registry, &world.fog_volumes)?`. The bridge's `populate_from_level` method iterates `world.fog_volumes` and, for each entry, spawns an ECS entity via `let id = registry.try_spawn(Transform { position: (entry.min + entry.max) * 0.5, rotation: Quat::IDENTITY, scale: Vec3::ONE }, &entry.tags)?;` (using the `try_spawn(transform, tags)` signature established by entity-model-foundation Task 2), attaches a `ComponentValue::FogVolume { density: entry.density, color: entry.color, scatter: entry.scatter, falloff: entry.falloff }` component, and records `FogVolumeAabb { min: entry.min.into(), max: entry.max.into() }` into the bridge's `FogVolumeAabbs` side-table keyed by the new `EntityId`. The `Application` struct in `main.rs` gains a `fog_volume_bridge: FogVolumeBridge` field next to `light_bridge`.

**AABB side-table ownership.** The side-table is a plain non-wgpu, non-wire structure owned by the game layer, mirroring the `LightBridge` / `ParticleRenderCollector` precedent in `crates/postretro/src/scripting/systems/`. Concretely:

- Define `pub struct FogVolumeAabb { pub min: Vec3, pub max: Vec3, pub height_gradient: f32, pub radial_falloff: f32 }` (a small local type in the new `fog_volume_bridge.rs` module — there is no pre-existing engine-wide `Aabb` type to reuse; `height_gradient`/`radial_falloff` are baked shape parameters stored here so the bridge can pack them into the GPU `FogVolume` record without reading `ComponentValue`) and `pub struct FogVolumeAabbs { table: HashMap<EntityId, FogVolumeAabb> }`. Both live in `crates/postretro/src/scripting/systems/fog_volume_bridge.rs` alongside `FogVolumeBridge` (defined in Task 5). Use `glam::Vec3` (already pervasive in game-side code, non-wgpu).
- The side-table is owned by `FogVolumeBridge`, which itself is owned by `Application` in `main.rs` — exactly parallel to how `Application` owns `LightBridge` and `ParticleRenderCollector`. It lives for the duration of the loaded level; `FogVolumeBridge::populate_from_level` (called from level load) inserts one entry per `FogVolumeRecord`, and a paired `FogVolumeBridge::clear` runs on level unload. `EntityRegistry` itself is *not* extended — fog AABBs are bridge-local, just as `LightBridge`'s `MapLightShape` table is bridge-local. `FogVolumeBridge::clear()` is called at level unload alongside `light_bridge.clear()` in the same `Application` hook in `main.rs`.
- The side-table is read each frame *only* by `FogVolumeBridge::update`, never by the renderer. The bridge correlates each `(EntityId, ComponentValue::FogVolume { density, color, scatter, falloff })` it pulls from the registry with the entity's stored AABB, packs `FogVolume` GPU records (the 64-byte struct from Task 1) into a `Vec<u8>`, and hands the bytes to `Renderer::upload_fog_volumes(&[u8])`. The renderer therefore never imports `postretro-level-format` types, never imports `EntityRegistry`, and never reads the side-table — it only consumes opaque packed bytes, exactly as it does today for `upload_bridge_lights` / `upload_bridge_descriptors` / `upload_bridge_samples`. This satisfies the rendering-pipeline §9 boundary rule.

### Task 3 — `FogVolume` component kind

**`scripting/registry.rs`:** add `ComponentKind::FogVolume = 5` and `ComponentValue::FogVolume { density: f32, color: [f32; 3], scatter: f32, falloff: f32 }`. Add to the `VARIANTS` const array. Note: `EntityRegistry::new` initialises component storage as an explicit array literal at `registry.rs:269` (`Vec<T>` is not `Copy`, so the repeat `[expr; N]` syntax cannot be used) — extend the literal from five to six `Vec::new()` entries to accommodate the new `FogVolume` kind.

**`scripting/conv.rs`:** extend `component_kind_from_name` with `"fog_volume" → ComponentKind::FogVolume` (matching snake_case convention of existing variants at `conv.rs:285-306`). Note: adding a new kind requires updating four locations in this file — `component_kind_name`, `component_kind_from_name`, `FromJs`/`IntoJs`, `FromLua`/`IntoLua`. Extend `FromJs`/`IntoJs` and `FromLua`/`IntoLua` for `ComponentValue::FogVolume`. `setComponent` accepts `density`, `color`, `scatter`, `falloff`; AABB fields are silently ignored if present. `getComponent` returns all four.

**`scripting/primitives.rs`:** add `"fog_volume"` to `worldQuery`'s filter string set. `worldQuery({ component: "fog_volume" })` returns handles with shape `{ id, position, tags, component: { density, color, scatter, falloff } }`. Follow the entity-model-foundation handle-shape convention for return values.

**`scripting/typedef.rs`:** add `"fog_volume"` to the `ComponentKind` union. Regenerate `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau`; type-definition drift test passes. The existing type-definition drift test in `cargo test` will catch stale generated files automatically once `FogVolume` is added to the registry. Add a unit test in `scripting/conv.rs` for the `ComponentValue::FogVolume` round-trip (JS and Luau).

### Task 4 — Point-light scatter

**`fog_pass.rs`:** add `fog_points_buffer: wgpu::Buffer` sized for `MAX_FOG_POINT_LIGHTS × FOG_POINT_LIGHT_SIZE`. Add `BIND_FOG_POINTS: u32 = 5` binding constant. Add the binding-5 entry to the group-6 BGL (storage buffer, read-only, compute-visible). Rebuild `build_group6` to include it. Add `upload_points(queue: &wgpu::Queue, points: &[FogPointLight])` method. Also add: `set_pixel_scale(device: &wgpu::Device, scale: u32, width: u32, height: u32, depth_view: &wgpu::TextureView)` — reallocates the scatter target at the new scaled resolution; `resize(device: &wgpu::Device, width: u32, height: u32, depth_view: &wgpu::TextureView)` — same reallocation on window resize.

**`fog_volume.wgsl` — extend `FogVolume` struct:** add `height_gradient: f32`, `radial_falloff: f32`, `_pad0: f32`, `_pad1: f32` to the WGSL `FogVolume` struct (`fog_volume.wgsl:71-78`) to match the 64-byte Task 1 layout. This step is a prerequisite for the `sample_fog_volumes` edits below.

**`fog_volume.wgsl` — falloff attenuation in `sample_fog_volumes`:** replace the bare `out.density += v.density` line with an edge-distance fade. Semantics: `falloff = 0` → uniform density to the AABB boundary; `falloff = 1` → linear ramp from zero at the face to full at the center; higher values → sharper interior dropoff.

```wgsl
// Proposed design (remove comment after implementation)
let half_ext = (v.max_v - v.min) * 0.5;
let center   = (v.min + v.max_v) * 0.5;
// local_abs: [0..1] per axis, 0 = center, 1 = face
let local_abs = abs(pos - center) / max(half_ext, vec3<f32>(1.0e-6));
// edge_t: 1 at volume center, 0 at nearest face
let edge_t   = 1.0 - clamp(max(local_abs.x, max(local_abs.y, local_abs.z)), 0.0, 1.0);
let box_fade = pow(clamp(edge_t, 0.0, 1.0), v.falloff);

// Height gradient: dense at min.y, fades toward max.y; 0 = no effect (height_fade = 1)
let height_t    = clamp((pos.y - v.min.y) / max(v.max_v.y - v.min.y, 1.0e-6), 0.0, 1.0);
let height_fade = clamp(1.0 - height_t * v.height_gradient, 0.0, 1.0);

// Radial gradient: density peaks at center, falls off to AABB half-diagonal; 0 = no effect (radial_fade = 1)
let half_diag   = length(half_ext);
let radial_t    = clamp(length(pos - center) / max(half_diag, 1.0e-6), 0.0, 1.0);
let radial_fade = pow(clamp(1.0 - radial_t, 0.0, 1.0), v.radial_falloff);

let fade = box_fade * height_fade * radial_fade;
out.density += v.density * fade;
```

Replace the corresponding `out.color` accumulation (`v.color * v.density`) with `v.color * v.density * fade` so the weighted color blend tracks the attenuated density. All three fade terms are independent and composable — setting any parameter to `0.0` collapses its term to `1.0`. The existing post-loop `out.color = out.color / out.density;` normalization line is preserved unchanged — it correctly normalizes the fade-weighted color sum.

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

### Task 5 — Renderer integration and FogVolumeBridge

This task has two halves: the game-side **bridge** that walks the ECS and produces GPU-ready bytes, and the renderer-side **FogPass wiring** that consumes those bytes.

**Game-side bridge** (`crates/postretro/src/scripting/systems/fog_volume_bridge.rs` — new module, registered in `scripting/systems/mod.rs`):

Define `pub(crate) struct FogVolumeBridge { aabbs: FogVolumeAabbs, entity_ids: Vec<EntityId>, volumes_bytes: Vec<u8>, points_bytes: Vec<u8> }` plus the `FogVolumeAabbs` / `FogVolumeAabb` types from Task 2. The bridge mirrors `LightBridge`'s shape: it owns retained scratch buffers across frames so capacity is reused.

API:

- `populate_from_level(&mut self, registry: &mut EntityRegistry, records: &[FogVolumeRecord]) -> Result<()>` — called once at level load. Spawns ECS entities, attaches components, and writes the AABB side-table (see "Level load" in Task 2).
- `clear(&mut self)` — called on level unload; drops `aabbs`, `entity_ids`, retains scratch capacity.
- `update_volumes(&mut self, registry: &EntityRegistry) -> Option<&[u8]>` — walks `registry.iter_with_kind(ComponentKind::FogVolume)`, looks up each entity's `FogVolumeAabb` from `self.aabbs` (providing `min`, `max`, `height_gradient`, `radial_falloff`), and together with the `ComponentValue::FogVolume { density, color, scatter, falloff }` from the registry, packs a complete `FogVolume` GPU record (Task 1's 64-byte struct) into `self.volumes_bytes`. Returns `Some(&self.volumes_bytes)` if non-empty, else `None`. Mirrors `ParticleRenderCollector::collect` + `iter_collections`.
- `update_points(&mut self, lights: &[MapLight]) -> &[u8]` — given the active dynamic light list (passed in by `main.rs`, sourced from the same place `render_frame_indirect` reads its lights), filters to `LightType::Point` with `_dynamic 1`, performs the sphere-vs-AABB pre-cull against `self.aabbs.values()` (O(lights × volumes); skip any light whose sphere intersects no fog volume AABB), packs survivors as `FogPointLight { position, range: falloff_range, color: [r×intensity, g×intensity, b×intensity], _pad: 0.0 }`, caps at `MAX_FOG_POINT_LIGHTS`, returns `&self.points_bytes`.

**Frame call site** (`main.rs`, between Game logic and the existing `render_frame_indirect` call, alongside `light_bridge.update`):

```rust
{
    let registry = self.script_ctx.registry.borrow();
    if let Some(bytes) = self.fog_volume_bridge.update_volumes(&registry) {
        renderer.upload_fog_volumes(bytes);
    } else {
        renderer.upload_fog_volumes(&[]); // signals zero volumes; FogPass::active() → false
    }
    let point_bytes = self.fog_volume_bridge.update_points(renderer.level_lights());
    renderer.upload_fog_points(point_bytes);
}
```

**Renderer-side wiring** (`crates/postretro/src/render/`):

Register `pub mod fog_pass;` in `render/mod.rs`. Add `fog: FogPass` to `Renderer`; construct in `Renderer::new`. Add three new methods on `Renderer` (each is a thin pass-through to `FogPass`, *not* a place where ECS or side-table types are referenced):

- `pub fn upload_fog_volumes(&mut self, bytes: &[u8])` — forwards to `self.fog.upload_volumes_bytes(&self.queue, bytes)`, which computes `volume_count = bytes.len() / FOG_VOLUME_SIZE`. An empty slice sets `volume_count = 0`, deactivating the pass.
- `pub fn upload_fog_points(&mut self, bytes: &[u8])` — forwards to `self.fog.upload_points_bytes(&self.queue, bytes)`.
- `pub fn set_fog_pixel_scale(&mut self, scale: u32)` — called at level load to apply `world.fog_pixel_scale`. Forwards to `self.fog.set_pixel_scale(...)` with the cached `width`, `height`, `depth_view`.

Inside `render_frame_indirect`, after the existing SH compose / shadow / forward / smoke passes:

1. `self.fog.upload_params(&self.queue, inv_view_proj, camera_pos, near, far)` — params come from the renderer's own camera state; no ECS needed.
2. `self.fog.upload_spots(&self.queue, &spots)` — spot scatter list, built from the renderer's existing dynamic-light array (the `level_lights` / shadow-light path), same source the existing forward shader uses. No new boundary crossing. `spots` is assembled from the renderer's own `level_lights()` data already present in `render_frame_indirect` scope — no additional data crossing is required.
3. If `self.fog.active()`: dispatch the raymarch compute pass, then the composite blit.

Note the renderer **does not** import `ComponentKind`, `ComponentValue`, `EntityRegistry`, `FogVolumeAabbs`, or any `scripting::*` type. The volumes and point-lights buffers are populated only via `upload_fog_volumes` / `upload_fog_points` from `main.rs`. This is identical to the precedent set by `upload_bridge_lights` for scripted-light data.

`Renderer::resize` calls `self.fog.resize(device, width, height, depth_view)`.

### Task 6 — SDK fog volumes module

Create `sdk/lib/entities/fog_volumes.ts` (and `.luau` twin). Define `FogVolumeHandle` — the return type of `world.query({ component: "fog_volume" })` — with fields from Task 3 (`id`, `position`, `tags`, `component`) plus four mutating methods:

- `setDensity(density: number, durationMs = 0)` — sets density instantly when `durationMs` is 0 or omitted; transitions over the given duration via a tick handler that lerps each frame and calls `setComponent`, mirroring the `LightEntity.setIntensity` pattern at `sdk/lib/entities/lights.ts:43-104`.
- `setColor(color: [number, number, number], durationMs = 0)` — same pattern for color; instant when unspecified.
- `setScatter(scatter: number)` — instant (no tween needed for this property).
- `setFalloff(falloff: number)` — instant. Mirrors the per-field-setter pattern established by `LightEntity.setIntensity` / `LightEntity.setColor` in `sdk/lib/entities/lights.ts` (Luau: `LightEntityHandle`), where every authorable component field has a dedicated setter rather than forcing callers through raw `setComponent`. Required so AC8 (verifying the `falloff = 0` vs `falloff = 1` visual contrast) has an SDK-level surface, not just a `setComponent({ kind: "fog_volume", falloff: 0 })` raw call. Instant rather than tweenable because falloff drives a subtle edge-shape parameter that authors typically toggle at scene-setup time, not animate.

Export animation constructors at module level:
- `pulseDensity(handle, { min, max, period })` — returns a running animation controller that oscillates density sinusoidally each tick. Cancel via the returned controller's `.stop()`.

All animation is tick-driven; no new engine primitives are required. Tick callbacks use `registerHandler("tick", ...)` internally.

Wire `fog_volumes` into the SDK prelude: add `export type { FogVolumeHandle } from "./entities/fog_volumes"; export { pulseDensity } from "./entities/fog_volumes";` to `sdk/lib/index.ts`, then regenerate `sdk/lib/prelude.js` via `cargo run -p postretro-script-compiler -- --prelude --sdk-root sdk/lib --out sdk/lib/prelude.js`. For Luau, add `fog_volumes.luau` to `LUAU_SDK_LIB_BLOCK` and the TS equivalent to `TS_SDK_LIB_BLOCK` in `crates/postretro/src/scripting/typedef.rs` (alongside the existing lights and emitters entries).

Add fog volume entries to `docs/scripting-reference.md`: `world.query({ component: "fog_volume" })`, `FogVolumeHandle` methods, animation constructors, relationship to FGD `env_fog_volume` and `_tags`.

---

## Sequencing

**Phase 1 (sequential):** Task 1 — `fx/fog_volume.rs` unblocks everything; `fog_pass.rs` does not compile without it.

**Phase 2 (concurrent):** Task 2 (PRL pipeline) and Task 4 (shader + point-light binding) — no shared files.

**Phase 3 (concurrent):** Task 3 (ComponentKind + scripting) and Task 5 (FogVolumeBridge + renderer integration) — Task 3 touches only `scripting/registry.rs`, `scripting/conv.rs`, `scripting/primitives.rs`, `scripting/typedef.rs`; Task 5 touches `scripting/systems/fog_volume_bridge.rs` (new), `scripting/systems/mod.rs`, `render/mod.rs`, `render/fog_pass.rs`, and adds the bridge call site in `main.rs`. The two tasks do not share files. Both depend on Tasks 1 and 2. Task 5 also depends on Task 3 (the bridge uses `ComponentKind::FogVolume`) and Task 4.

**Phase 4 (sequential):** Task 6 (SDK + docs) — depends on Task 3 for the component API shape.

---

## Rough sketch

`fog_pass.rs` is in `render/` but not yet declared as a module in `render/mod.rs` — adding `pub mod fog_pass;` is the minimal step to bring it into the build. The compiler will then surface all missing imports from `crate::fx::fog_volume`, which Task 1 resolves.

The AABB is not in `ComponentValue::FogVolume` because it is baked level geometry. Scripts that need volume bounds for spatial logic should query the AABB side-table via a dedicated primitive (e.g., `getEntityBounds(id) -> { min, max }`). Adding this primitive is deferred (see Decisions).

`FogPointLight.color` is pre-multiplied by intensity on the CPU before upload, matching the `FogSpotLight.color` convention already in `fog_pass.rs`. No additional intensity field in the GPU struct.

The `falloff` field attenuates density at AABB edges — `falloff = 0` gives a hard box boundary; `falloff = 1` gives a linear center-to-edge ramp (the default). The attenuation is applied inside `sample_fog_volumes`, not in the outer scatter accumulation, so the weighted color blend stays consistent with the attenuated density value.

For the `env_fog_volume` FGD, `_tags` works identically to all other FGD entities since entity-model-foundation established it as a universal KVP convention. TrenchBroom authors tag fog volumes with `_tags neon_haze` and scripts filter with `world.query({ component: "fog_volume", tag: "neon_haze" })`.

Fog volumes are loaded exclusively from the FogVolumes PRL section — the engine does not register an `env_fog_volume` handler in `ClassnameDispatch`. The classname is consumed only by prl-build at compile time.

Static neon lights (`_dynamic 0`) tint fog via SH ambient (baked into the irradiance volume). For localized halos, authors must use `_dynamic 1`. The plan does not change static-light behavior.

---

## Boundary inventory

| Name | Rust | Wire / serde | TS / JS | Luau | FGD KVP |
|---|---|---|---|---|---|
| `ComponentKind::FogVolume` | `ComponentKind::FogVolume` | `"fog_volume"` | `"fog_volume"` | `"fog_volume"` | n/a |
| `ComponentValue::FogVolume` | `ComponentValue::FogVolume { density, color, scatter, falloff }` | `{ kind: "fog_volume", density, color, scatter, falloff }` | same | same | n/a |
| fog volume entity | `env_fog_volume` brush entity | FogVolumes PRL section | `FogVolumeHandle` | `FogVolumeHandle` | `env_fog_volume` |
| fog_pixel_scale | `LevelWorld.fog_pixel_scale: u32` | FogVolumes PRL section header `u32` | n/a | n/a | `fog_pixel_scale` on worldspawn |
| `FogPointLight` | `fx::fog_volume::FogPointLight` | group 6 binding 5 storage buffer | n/a | n/a | n/a |

---

## Wire format

**FogVolumes section** — little-endian throughout. New surface; no existing section mirrors this layout exactly. Section ID assigned at implementation time.

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
height_gradient: f32        4 bytes
radial_falloff: f32         4 bytes
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

---

## Known limitations

- **Mid-game dynamic lights not enrolled in fog scatter.** `absorb_dynamic_lights` runs once at level load, not per frame. Lights spawned by gameplay scripts after load (e.g. from a `tick` handler) are never added to `LightBridge.entity_ids` and therefore never reach fog scatter. This matches current engine behavior for direct lighting — it is not a fog-specific bug. Authors who need fog scatter from script-spawned lights must place `_dynamic 1` lights in the map rather than spawning them at runtime.

