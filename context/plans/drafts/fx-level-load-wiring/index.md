# FX — Level-Load Wiring for Fog Volumes

> **Status:** draft.
> **Depends on:** `fx-volumetric-smoke` merged to main (done — `render/fog_pass.rs`, `shaders/fog_volume.wgsl`, `shaders/fog_composite.wgsl` are present).
> **Related:** `context/lib/build_pipeline.md` §Custom FGD · `context/lib/rendering_pipeline.md` §7.5 (volumetric fog) · `context/lib/development_guide.md` §1.6 (breaking API changes — no compat shims).

---

## Context

`fx-volumetric-smoke` landed the fog volume render pass as an inert skeleton. Current state:

- `postretro/src/render/fog_pass.rs` — GPU pass exists. `FogPass::upload_volumes`, `set_pixel_scale`, `upload_params` are implemented.
- `postretro/src/shaders/fog_volume.wgsl`, `fog_composite.wgsl` — shaders exist.
- `postretro/src/fx/fog_volume.rs` — **does not exist.** `FogVolume`, `FogSpotLight`, `FogParams`, and `MAX_FOG_VOLUMES` are missing. This is the data module the pass needs.
- Nothing in `fx/mod.rs` or `render/mod.rs` wires the fog pass. No AABB buffer is ever uploaded; `fog_pixel_scale` and `fog_step_size` never move off their defaults.

The compiler side is also empty. `postretro-level-compiler/src/parse.rs` recognises `worldspawn` and light entities only. `env_fog_volume` is named in `context/lib/build_pipeline.md` §Custom FGD but has no parser hook, translator, PRL section, loader path, or runtime wiring.

Done means: a TrenchBroom-authored `.map` placing an `env_fog_volume` brush entity compiles to a `.prl` carrying fog data; loading the `.prl` calls `Renderer::set_fog_volumes` / `set_fog_pixel_scale` / `set_fog_step_size`; the fog pass renders visibly inside the brush volume.

---

## Goal

Close the compiler → PRL → runtime gap for `env_fog_volume` so the fog render pass added by `fx-volumetric-smoke` is fed from level data.

**Crates:** `postretro-level-format` (new section modules), `postretro-level-compiler` (parser brush retention + translator + pack), `postretro` (new data module + loader + renderer wiring).

---

## Upstream gap: entity brushes are discarded

The current parser (`postretro-level-compiler/src/parse.rs`) records only `(classname, brush_count)` for non-worldspawn entities. Brush planes, sides, and vertices are never retained. Fog-volume AABB extraction needs per-brush bounds for each `env_fog_volume` brush entity. Fix this in the parser rather than routing fog geometry through the worldspawn BSP.

Extend `EntityInfo` (`map_data.rs`) with `brush_aabbs: Vec<Aabb>` populated for non-worldspawn entities from the same face-vertex data the worldspawn loop consumes, already swizzled to engine space. The existing `entity_brushes: Vec<(String, usize)>` diagnostic field is superseded — delete it in the same pass (pre-release policy; call sites are only in `parse.rs`).

Each `EntityInfo` with `classname == "env_fog_volume"` and `!brush_aabbs.is_empty()` becomes one or more `FogVolume` records. Emit one record per brush — the runtime's point-in-AABB check is per-sample with no dedup semantics; disjoint brushes under one entity should each be their own AABB. Warn if the entity has no brushes.

---

## New / modified files

- `postretro/src/fx/fog_volume.rs` *(new)* — `FogVolume`, `FogSpotLight`, `FogParams`, `MAX_FOG_VOLUMES = 16`, `clamp_fog_pixel_scale`. Referenced by `fog_pass.rs`; create it here and declare it in `fx/mod.rs`.
- `postretro-level-format/src/lib.rs` — add `FogVolumes = 25` and `WorldspawnProps = 26` to `SectionId` enum and `from_u32`.
- `postretro-level-format/src/fog_volumes.rs` *(new)* — `FogVolumesSection` with `FogVolumeRecord` and `to_bytes` / `from_bytes`. Pattern: `alpha_lights.rs`.
- `postretro-level-format/src/worldspawn.rs` *(new)* — `WorldspawnSection` carrying `ambient_color`, `fog_pixel_scale`, `fog_step_size`.
- `postretro-level-compiler/src/map_data.rs` — add `MapFogVolume`, `MapWorldspawn`, and corresponding `Vec` fields on `MapData`. Delete `entity_brushes`.
- `postretro-level-compiler/src/format/quake_map.rs` — add `translate_fog_volume(props, brush_aabbs)` and `translate_worldspawn(props)`.
- `postretro-level-compiler/src/parse.rs` — retain entity brush AABBs, route `env_fog_volume` through the translator, extract worldspawn props.
- `postretro-level-compiler/src/pack.rs` — `encode_fog_volumes` and `encode_worldspawn`; append both section blobs to the pack call.
- `postretro/src/prl.rs` — add `fog_volumes: Vec<FogVolumeData>` and `worldspawn: WorldspawnData` to `LevelWorld`, decode both sections. Absent `WorldspawnSection` → engine defaults (`fog_pixel_scale = 4`, `fog_step_size = 0.5`, `ambient_color = [0,0,0]`).
- `postretro/src/render/mod.rs` — wire `FogPass` into the render pipeline; call `set_fog_pixel_scale`, `set_fog_step_size`, `set_fog_volumes` at level load.

---

## Byte layout — `FogVolumes` (ID 25), version 1

Fixed header + fixed-size records, little-endian throughout.

```
u32  volume_count
FogVolumeRecord[volume_count]         (fixed size: 40 bytes each)
```

`FogVolumeRecord`:

```
f32[3]  aabb_min        (engine meters)
f32[3]  aabb_max        (engine meters)
f32[3]  color           (linear RGB, 0–1)
f32     density         (per-meter extinction coefficient)
f32     falloff         (volume edge falloff, forwarded as-authored)
f32     scatter         (0–1 scatter fraction toward camera, default 0.6)
```

The format crate owns the wire layout only. The loader converts `FogVolumeRecord` → runtime `FogVolume` struct at load time.

## Byte layout — `WorldspawnSection` (ID 26), version 1

Fixed size: 20 bytes, little-endian.

```
f32[3]  ambient_color       (linear RGB)
u32     fog_pixel_scale     (wire value; loader clamps to 1..=8)
f32     fog_step_size       (meters)
```

Verify no existing `ambient_color` wire path exists (`grep -r ambient_color postretro-level-format/src`) before adding this section. If one exists, extend it instead of adding ID 26.

---

## Translator — field mapping

`env_fog_volume` (brush entity) → `MapFogVolume`:

| FGD property | Default | Validation |
|---|---|---|
| brush AABBs | required | at least one brush; warn if none |
| `color` | `[1, 1, 1]` | `"r g b"` floats in 0–1 range, matching `fog_volume.wgsl` expectations |
| `density` | `0.5` | `>= 0`; error if missing and no default applies |
| `falloff` | `1.0` | `>= 0` |
| `scatter` | `0.6` | clamp to `[0, 1]` |

`worldspawn` → `MapWorldspawn`:

| FGD property | Default | Validation |
|---|---|---|
| `ambient_color` | `[0, 0, 0]` | parse as RGB |
| `fog_pixel_scale` | `4` | integer; clamp `[1, 8]` with warning if out of range |
| `fog_step_size` | `0.5` | `f32`; must be `> 0`; add to §Custom FGD in `build_pipeline.md` |

Validation errors block compilation. Warnings log and proceed with defaults. Same policy as `translate_light`.

---

## Loader → runtime wiring

After renderer construction, at level load:

```rust
// Proposed design
if let Some(level) = &self.level {
    renderer.set_fog_pixel_scale(level.worldspawn.fog_pixel_scale);
    renderer.set_fog_step_size(level.worldspawn.fog_step_size);
    let volumes: Vec<fx::fog_volume::FogVolume> =
        level.fog_volumes.iter().map(fog_volume_data_to_gpu).collect();
    renderer.set_fog_volumes(&volumes);
}
```

`fog_volume_data_to_gpu` converts `FogVolumeData` (PRL) → `FogVolume` (GPU packed). Add it to `postretro/src/fx/fog_volume.rs`.

---

## Acceptance gates

- `env_fog_volume` brush in a test map compiles; the resulting `.prl` has a `FogVolumes` section with one record per brush. Unit test on `FogVolumesSection` round-trip passes.
- `worldspawn` with `fog_pixel_scale 2` in the `.map` produces `LevelWorld.worldspawn.fog_pixel_scale == 2`; `renderer.set_fog_pixel_scale(2)` is called at startup.
- `cargo run -p postretro -- content/base/maps/test.prl` shows the fog pass active inside the brush volume: visible pixelated haze, spot-beam shafts, SH tint.
- Maps with no `env_fog_volume` entities produce no fog pass work — `renderer.set_fog_volumes(&[])` is a no-op; `FogPass::active()` returns false.
- Compiler warns on `env_fog_volume` with no brushes; errors on out-of-range `scatter`.

---

## Acceptance criteria

1. `cargo test --workspace` passes. Unit tests: `FogVolumesSection` and `WorldspawnSection` round-trips; fog-absent map produces no fog work.
2. `cargo clippy --workspace -- -D warnings` clean.
3. No new `unsafe`.
4. Acceptance gates above.
5. `context/lib/build_pipeline.md` §Custom FGD: fog-volume row updated to "resolved at compile time to world-space AABBs; uploaded as a compact storage buffer" (drop BSP-leaf language); `fog_step_size` added to worldspawn row.
6. `context/lib/build_pipeline.md` §PRL section IDs: add `FogVolumes (25)` (when present) and `WorldspawnProps (26)` (always).

---

## Out of scope

- `env_smoke_emitter` wiring. Handled by the scripting foundation (Plan 3 sub-plan 8) via the entity registry, not a PRL section.
- Per-volume `fog_pixel_scale` — global render-target property, one divisor per map on `worldspawn`.
- BSP-leaf membership for fog volumes. Runtime uses point-in-AABB per raymarch sample.
- Compile-time validation that fog volumes sit inside the map hull.
- Ambient-color rendering semantics beyond wiring (don't change shader evaluation; just give `ambient_color` a PRL home).
- Kinematic or destructible fog volumes. Baked-once.
- Extending `MAX_FOG_VOLUMES` beyond 16.

---

## Open design questions

1. **One record per fog brush vs. union AABB per entity.** Recommendation: one record per brush.
2. **Does `ambient_color` already have a wire home?** Grep `postretro-level-format/src` before adding `WorldspawnSection`. If yes, extend the existing section.
3. **`fog_step_size` FGD property.** Not in `context/lib/build_pipeline.md` §Custom FGD today. Add it as a `worldspawn` property alongside `fog_pixel_scale`.
