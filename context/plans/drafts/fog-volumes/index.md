# Fog Volumes

## Goal

Wire up the partially-built fog volume pass so `env_fog_volume` brush regions render
as localized volumetric haze. Extend the raymarch shader with point-light
(omnidirectional) scatter so dynamic `light` entities — neon signs, colored lamps —
create visible in-fog glow halos.

The GPU-side infrastructure (`fog_pass.rs`, `fog_volume.wgsl`, `fog_composite.wgsl`)
is already written. What is missing is the CPU module those files import from, the
renderer integration, the level-loading plumbing, and the point-light scatter path.

---

## Scope

### In scope

- Create `crates/postretro/src/fx/fog_volume.rs` — CPU structs, constants, and pack functions required by the orphaned `fog_pass.rs`
- Register `pub mod fog_volume;` in `fx/mod.rs` and `pub mod fog_pass;` in `render/mod.rs`
- Integrate `FogPass` into `Renderer` — instantiate, call each frame (compute dispatch + composite blit), handle resize and surface-format change
- prl-build: parse `env_fog_volume` brush entities, compute world-space AABBs, write new `FogVolumes` PRL section (ID 30)
- prl-build: extract `fog_pixel_scale` from worldspawn and write it as a header field in the FogVolumes section
- `postretro-level-format`: define `SectionId::FogVolumes = 30` and a `FogVolumesSection` parser/writer
- Engine: load FogVolumes section at level load; upload volumes buffer; apply `fog_pixel_scale` to `FogPass`
- Add dynamic point-light scatter to `fog_volume.wgsl`: new `fog_points` binding (group 6, binding 5) iterates active dynamic omni lights and accumulates in-fog glow
- Add `FogPointLight` struct, `upload_points()` method, and binding-5 slot to `FogPass` and its group-6 BGL
- CPU-side upload: filter dynamic lights for `LightType::Point`, pack as `FogPointLight`, upload each frame

### Out of scope

- Static light scatter in fog (static point lights already tint fog via SH ambient; per-light localised scatter for static lights requires iterating baked per-light data and is deferred)
- Shadow maps for point lights (point lights have no shadow maps in this engine)
- Spot light scatter (already implemented in `fog_volume.wgsl`)
- Animated fog density or color (driven by scripting if ever needed)
- Bilinear upscaling of the fog scatter buffer (nearest-neighbor is intentional)
- Per-volume `fog_pixel_scale` override (single global pass resolution)

---

## Acceptance criteria

- [ ] `env_fog_volume` brush entity placed in a test scene produces visible pixelated haze when the player walks into the volume.
- [ ] A dynamic `light` entity (`_dynamic 1`) inside or adjacent to an `env_fog_volume` region causes the fog in that region to glow with the light's color — visible as a localised tinted halo distinct from the ambient SH tint.
- [ ] Removing the `env_fog_volume` (or disabling the dynamic light) eliminates the halo — confirming the scatter path is driving it, not an ambient term.
- [ ] `fog_pixel_scale 1` on `worldspawn` produces smooth full-resolution fog; `fog_pixel_scale 8` produces coarser block-pixel fog.
- [ ] `fog_pixel_scale` absent from `worldspawn` defaults to 4.
- [ ] Level with no `env_fog_volume` entities: fog pass compute dispatch and composite blit are skipped entirely (`FogPass::active()` returns false), no render-time cost.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo clippy --workspace -- -D warnings` clean.
- [ ] No new `unsafe`.

---

## Tasks

### Task 1 — CPU module: `fx/fog_volume.rs`

Create `crates/postretro/src/fx/fog_volume.rs`. Define:

- `FogVolume { min, density, max, falloff, color, scatter }` — 48 bytes packed, mirrors the WGSL struct in `fog_volume.wgsl`. Field `max` must be packed as `max_v` in the WGSL (keyword collision) but the Rust struct can use `max`.
- `FogSpotLight { position, slot, direction, cos_outer, color, range }` — 48 bytes, mirrors existing WGSL `FogSpotLight`.
- `FogPointLight { position, range, color, _pad }` — 32 bytes. `color` is pre-multiplied by intensity. No slot or direction fields.
- `FogParams { inv_view_proj, camera_position, step_size, volume_count, near_clip, far_clip, _pad }` — matches existing WGSL `FogParams`.
- Constants: `MAX_FOG_VOLUMES`, `MAX_FOG_POINT_LIGHTS`, `FOG_VOLUME_SIZE`, `FOG_SPOT_LIGHT_SIZE`, `FOG_POINT_LIGHT_SIZE`, `FOG_PARAMS_SIZE`, `DEFAULT_FOG_STEP_SIZE`.
- Pack functions: `pack_fog_volumes`, `pack_fog_spot_lights`, `pack_fog_point_lights`, `pack_fog_params`.
- `clamp_fog_pixel_scale(scale: u32) -> u32` — clamps to 1..=8, defaulting to 4 on 0.

Register `pub mod fog_volume;` in `crates/postretro/src/fx/mod.rs`.

### Task 2 — Renderer integration

Register `pub mod fog_pass;` in `crates/postretro/src/render/mod.rs` (currently the file exists but is orphaned). Add `fog: FogPass` field to `Renderer`. Construct it in `Renderer::new`, passing the camera BGL, SH BGL, and spot-shadow BGL.

Each frame, after the billboard sprite pass and before Present:
1. `fog.upload_params(queue, inv_view_proj, camera_pos, near, far)`
2. Collect active dynamic spot lights with shadow slots → `fog.upload_spots(queue, &fog_spots)`.
3. Collect active dynamic point lights (see Task 4) → `fog.upload_points(queue, &fog_points)`.
4. If `fog.active()`: dispatch the raymarch compute pass, then dispatch the composite blit over the forward-rendered surface.

Handle `Renderer::resize` — call `fog.resize(device, width, height, depth_view)`. Handle surface-format change — call `fog.rebuild_composite_for_format(device, format)`.

Group-6 bind group is owned by `FogPass` and rebuilt by `FogPass::resize` as needed; the renderer passes no bind-group references — it just calls the `FogPass` methods.

### Task 3 — PRL section and level loading

**`postretro-level-format` crate:**

Add `SectionId::FogVolumes = 30` to `lib.rs`. Add `30 => Some(Self::FogVolumes)` to `SectionId::from_u32`. Create `fog_volumes.rs` with a `FogVolumesSection` struct: `pixel_scale: u32`, `volumes: Vec<FogVolumeRecord>`. `FogVolumeRecord` holds the same six f32 groups as `FogVolume` in the engine (min, density, max, falloff, color, scatter). Implement `FogVolumesSection::to_bytes` and `FogVolumesSection::from_bytes` following the pattern of existing section files. Wire `pub mod fog_volumes;` in the crate.

**Wire format** — FogVolumes section (ID 30), little-endian throughout:

```
pixel_scale: u32
volume_count: u32
per entry (48 bytes each):
  min_x, min_y, min_z: f32   (12 bytes)
  density: f32               ( 4 bytes)
  max_x, max_y, max_z: f32   (12 bytes)
  falloff: f32               ( 4 bytes)
  color_r, color_g, color_b: f32  (12 bytes)
  scatter: f32               ( 4 bytes)
```

Section is optional. Engine absent-reads default to `pixel_scale = 4, volume_count = 0`.

**`postretro-level-compiler` (prl-build):**

After the `env_reverb_zone` brush-entity resolution pass (same structural pattern), add an `env_fog_volume` pass:
- Iterate brush entities with classname `env_fog_volume`.
- Exclude their brushes from world geometry (already done by the brush-entity path for `env_reverb_zone`; follow the same mechanism).
- For each such entity, compute the world-space AABB over all its brush faces.
- Parse KVPs: `color` (RGB, default `1 1 1`), `density` (f32, default `0.5`), `falloff` (f32, default `1.0`), `scatter` (f32, default `0.6`). Log a warning and skip if volume count would exceed `MAX_FOG_VOLUMES`.
- Read `fog_pixel_scale` from the worldspawn entity KVP (u32, default 4, clamp 1–8).
- Write `FogVolumesSection` to the PRL.

**Engine (`prl.rs`):**

At level load, read the `FogVolumes` section if present. Populate `LevelWorld` with `fog_volumes: Vec<FogVolume>` and `fog_pixel_scale: u32`. The renderer reads these fields at level load and calls `fog_pass.upload_volumes()` and `fog_pass.set_pixel_scale()`.

### Task 4 — Point-light scatter

**`fog_pass.rs`:**

Add `fog_points_buffer: wgpu::Buffer` sized for `MAX_FOG_POINT_LIGHTS × FOG_POINT_LIGHT_SIZE`. Add `BIND_FOG_POINTS: u32 = 5` binding constant. Add the binding-5 entry to the group-6 BGL (`wgpu::BufferBindingType::Storage { read_only: true }`, compute-visible). Rebuild `build_group6` to include the new buffer. Add `upload_points(queue: &wgpu::Queue, points: &[FogPointLight])` method.

**`fog_volume.wgsl`:**

Add `struct FogPointLight { position: vec3<f32>, range: f32, color: vec3<f32>, _pad: f32 }` and `@group(6) @binding(5) var<storage, read> fog_points: array<FogPointLight>`. In `cs_main`, after the spot-light loop, add a point-light loop:

```wgsl
// Proposed design (remove comment after implementation)
let pt_count = arrayLength(&fog_points);
for (var pi: u32 = 0u; pi < pt_count; pi = pi + 1u) {
    let pt = fog_points[pi];
    let to_light = pt.position - pos;
    let dist = length(to_light);
    if dist > pt.range || dist < 1.0e-4 { continue; }
    let atten = clamp(1.0 - dist / pt.range, 0.0, 1.0);
    accum = accum + transmittance * weight * vs.color * pt.color * atten;
}
```

No shadow map occlusion for point lights.

**Renderer (per-frame upload, `render/mod.rs`):**

From `level_lights` (the already-filtered dynamic light list in `Renderer`), additionally collect lights where `light_type == LightType::Point`, cap at `MAX_FOG_POINT_LIGHTS`, pack as `FogPointLight { position, range: falloff_range, color: [r*intensity, g*intensity, b*intensity], _pad: 0.0 }`, and pass to `fog.upload_points()` each frame. This collection happens alongside the existing spot-light collection for fog.

---

## Sequencing

**Phase 1 (sequential):** Task 1 — `fx/fog_volume.rs` unblocks everything; `fog_pass.rs` does not compile without it.

**Phase 2 (concurrent):** Task 3 (level-format + prl-build + engine PRL loading) and Task 4 (shader + fog_pass.rs binding extension) — Task 3 touches only the level-format crate, prl-build, and `prl.rs`; Task 4 touches only `fog_volume.wgsl` and `fog_pass.rs`. No shared files.

**Phase 3 (sequential):** Task 2 (renderer wiring) — consumes the completed `FogPass` API from Tasks 1 and 4, and the level-loaded volumes and pixel-scale from Task 3.

---

## Rough sketch

`fog_pass.rs` is in `crates/postretro/src/render/` but not yet declared as a module in `render/mod.rs`. Adding `pub mod fog_pass;` is the minimal change to bring it into the build. The compiler will then surface all missing imports from `crate::fx::fog_volume`.

`FogPointLight` is deliberately simpler than `FogSpotLight`: no slot, no direction, no cone angle. The distance check against `range` is the only culling; influence-volume pre-culling would require pulling in the influence-volume buffer (group 2) which is not currently bound to the fog pipeline. At retro-scale light counts (≤ 32), iterating all dynamic point lights per raymarch step is acceptable.

Static neon lights (`_dynamic 0`) continue to tint fog via the SH ambient term (their contribution is baked into the irradiance volume). For localized halos, authors must use `_dynamic 1`. The plan does not change static-light behavior.

The `env_fog_volume` brush in the level compiler should follow the same pattern as `env_reverb_zone` for brush-entity AABB extraction — search the level-compiler for the reverb zone resolution pass when implementing Task 3.

---

## Open questions

- **Influence-volume pre-culling for point lights.** The current design iterates all dynamic point lights (capped at 32) per raymarch step. If a map has many dynamic point lights and a large fog volume, this may become costly. Pre-culling against the fog volume AABB at CPU side (before upload) would reduce the per-step iteration, but adds complexity. Leave at "iterate all" until profiling on a representative scene indicates otherwise.

- **`fog_pixel_scale` in existing maps.** Maps compiled before this plan have no FogVolumes section. The engine must default `pixel_scale` to 4 silently. Confirm this is the correct default or adjust.

- **FogVolumes section always written vs. conditional.** Should prl-build always emit the FogVolumes section (to carry `fog_pixel_scale` even in maps with no fog volumes), or only when at least one `env_fog_volume` exists? If always, the engine always picks up the pixel scale. If conditional, absence means default-4. Either works — decide at implementation time and make it explicit in the section writer.
