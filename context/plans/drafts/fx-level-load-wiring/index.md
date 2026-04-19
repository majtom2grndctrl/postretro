# FX — Level-Load Wiring for Smoke Emitters and Fog Volumes

> **Status:** draft.
> **Depends on:** `fx-volumetric-smoke` (must merge first — this plan consumes its runtime APIs: `App.smoke_emitters`, `Renderer::register_smoke_collection`, `Renderer::set_fog_volumes`, `Renderer::set_fog_pixel_scale`, `Renderer::set_fog_step_size`).
> **Related:** `context/lib/build_pipeline.md` §Custom FGD / §PRL Compilation · `context/lib/rendering_pipeline.md` §7.4 (billboard smoke) §7.5 (volumetric fog) · `context/lib/development_guide.md` §1.6 (breaking API changes — no compat shims) · `context/plans/in-progress/fx-volumetric-smoke/index.md`.

---

## Context

`fx-volumetric-smoke/` shipped two FX systems but the level-load wire-up was deferred to unblock the renderer work. Both systems now exist in the runtime but are exercised only via demo CLI flags:

- **Smoke emitters (Task A in `fx-volumetric-smoke`).** `App.smoke_emitters: Vec<fx::smoke::SmokeEmitter>` is seeded by `build_demo_emitters(...)` gated on `--demo-smoke` (see `postretro/src/main.rs` ~lines 98, 149, 203–226 on `worktree-agent-a11c0602`). When present, the renderer calls `Renderer::register_smoke_collection` per collection name (see `postretro/src/render/mod.rs` ~line 1535) and drains packed instance uploads each frame (`postretro/src/render/mod.rs` ~line 1743). The compiler never emits `env_smoke_emitter` entity data.
- **Fog volumes (Task B in `fx-volumetric-smoke`).** The fog pass owns GPU state through `crate::fx::fog_volume::{FogVolume, FogSpotLight, FogParams, MAX_FOG_VOLUMES=16, clamp_fog_pixel_scale}` and `render::fog_pass::FogPass::{upload_volumes, set_pixel_scale, upload_params}` (see `postretro/src/render/fog_pass.rs` ~lines 16–19, 451–495). Nothing feeds it. No AABB buffer is ever uploaded; `fog_pixel_scale` and `fog_step_size` never move off their defaults.

The compiler side is also empty. `postretro-level-compiler/src/parse.rs` recognises `worldspawn` and light entities only; `MapData.entity_brushes` stores a `(classname, count)` diagnostic summary (`map_data.rs:248`) and drops all non-worldspawn brush geometry. `env_fog_volume` and `env_smoke_emitter` are named in `context/lib/build_pipeline.md` §Custom FGD but neither has a parser hook, a translator (analogous to `format/quake_map::translate_light`), a PRL section, a loader path, or any plumbing to the runtime.

"Done" means: a TrenchBroom-authored `.map` that places an `env_smoke_emitter` point entity and an `env_fog_volume` brush entity compiles to a `.prl` carrying that data; loading the `.prl` populates `App.smoke_emitters` and calls `Renderer::set_fog_volumes` / `set_fog_pixel_scale` / `set_fog_step_size`; the `--demo-smoke` / demo-fog paths introduced by `fx-volumetric-smoke` are removed (they stood in for PRL data that now actually flows); all `fx-volumetric-smoke` acceptance gates can be re-run against a real level.

---

## Goal

Close the compiler → PRL → runtime gap for both `env_smoke_emitter` and `env_fog_volume` so the FX runtime APIs added by `fx-volumetric-smoke` are fed from level data, not demo seeds. Two deliverables, split by entity kind:

- **Task A:** `env_smoke_emitter` point-entity pipeline — translator, `SmokeEmitters` PRL section, loader wire-up into `App.smoke_emitters`, remove `--demo-smoke` seed.
- **Task B:** `env_fog_volume` brush-entity pipeline — brush AABB retention in the parser, translator for fog params, `FogVolumes` PRL section, `worldspawn.fog_pixel_scale` propagation, loader wire-up into `Renderer::set_fog_volumes` / `set_fog_pixel_scale`, remove demo-fog seed.

Tasks A and B are fully independent and can be run as concurrent workstreams. See the "Shared plumbing?" note below for the one judgement call that spans both.

---

## Shared plumbing? (design question)

Both tasks add a new PRL section of the general shape "FX entity of kind X, list of records, fixed-size per record." Reasonable options:

1. **Two dedicated sections.** New `SmokeEmitters` (ID 24) and `FogVolumes` (ID 25). Encoders live in `postretro-level-format::smoke_emitters` and `postretro-level-format::fog_volumes`. Each has its own `to_bytes` / `from_bytes` using the same hand-rolled LE byte pattern as `alpha_lights.rs` / `light_influence.rs`.
2. **One generic `FxEntities` section** with a kind tag per record. Saves one section ID. Forces a discriminated-union layout and a string collection field for smoke's `collection` name — the kind tag makes per-kind fields variable-length, which the format crate does not currently do.
3. **One section per kind plus a shared helper crate** for common encode/decode primitives (u32 length prefix, Vec3 LE readers, etc.).

**Recommendation: option 1, two dedicated sections.** The existing format crate's pattern is one module per section, flat array, fixed-size per record (see `alpha_lights.rs`, `light_influence.rs`, `chunk_light_list.rs`). Smoke records carry a variable-length UTF-8 `collection` string; fog records do not. Fusing them forces either string fields into fog records (wasted bytes on every map) or a variant layout the format crate has no precedent for. Option 3 is worth considering only if a third FX entity lands; deferring that call is cheaper than pre-abstracting. The plan proceeds on option 1 unless the implementer finds a reason to revisit.

---

## Task A — `env_smoke_emitter` wire-up

**Crates:** `postretro-level-format` (new section module), `postretro-level-compiler` (translator + pack), `postretro` (loader + App glue).

### New / modified files

- `postretro-level-format/src/lib.rs` — add `SmokeEmitters = 24` to `SectionId` enum (~line 63) and its `from_u32` arm (~line 114).
- `postretro-level-format/src/smoke_emitters.rs` *(new)* — `SmokeEmittersSection` with `SmokeEmitterRecord` and `to_bytes` / `from_bytes`. Pattern-match `alpha_lights.rs`.
- `postretro-level-compiler/src/format/quake_map.rs` — add `SMOKE_EMITTER_CLASSNAME = "env_smoke_emitter"`, `is_smoke_emitter_classname`, and `translate_smoke_emitter(props: &HashMap<String, String>, origin: DVec3) -> Result<MapSmokeEmitter, TranslateError>` mirroring `translate_light`'s shape.
- `postretro-level-compiler/src/map_data.rs` — add `MapSmokeEmitter` struct and `MapData.smoke_emitters: Vec<MapSmokeEmitter>`.
- `postretro-level-compiler/src/parse.rs` — route `env_smoke_emitter` classnames through `quake_map::translate_smoke_emitter` alongside the existing light dispatch (~line 148). Same `origin` precondition as lights.
- `postretro-level-compiler/src/pack.rs` — add `encode_smoke_emitters(&[MapSmokeEmitter]) -> SmokeEmittersSection`; extend `pack_and_write_pvs` / `pack_and_write_portals` section lists to include it.
- `postretro-level-compiler/src/main.rs` — thread `map_data.smoke_emitters` into the pack call.
- `postretro/src/prl.rs` — add `smoke_emitters: Vec<SmokeEmitterData>` to `LevelWorld` (~line 153) and decode it in the existing PRL read path (search the file for `AlphaLights` / `read_section_data` call sites — it decodes sections lazily after `read_container`).
- `postretro/src/main.rs` — replace `build_demo_emitters(...)` with `build_emitters_from_level(&level)`. Delete `--demo-smoke` arg parsing (~line 98) and the `build_demo_emitters` helper. `App.smoke_emitters` is now populated from `level.smoke_emitters`.

### Byte layout — `SmokeEmitters` (ID 24), version 1

On-disk, little-endian throughout.

```
u32  emitter_count
SmokeEmitterRecord[emitter_count]
```

`SmokeEmitterRecord` is **not** fixed-size because `collection` is variable-length UTF-8. Concrete record layout:

```
f32[3]  origin                 (engine meters, Y-up)
f32     rate                   (sprites/sec)
f32     lifetime               (seconds)
f32     size                   (world units)
f32     speed                  (drift velocity, m/s)
f32     spec_intensity         (Blinn-Phong specular scale)
u16     collection_len         (bytes)
u8[collection_len]  collection (UTF-8, no trailing null)
```

Per-record size: `12 + 4*5 + 2 + collection_len` = `34 + collection_len` bytes. Decoder validates `collection_len` against the remaining buffer before slicing, rejects non-UTF-8 as `FormatError::Io(InvalidData)`.

Origin is `f32[3]` (not `f64[3]` as `AlphaLightRecord` uses). `AlphaLightRecord`'s `f64` origin predates the precision review; smoke emitter positions don't need it, and the runtime consumes `glam::Vec3` immediately. If uniformity with `alpha_lights` matters more than economy, the implementer can widen to `f64[3]` in review; flag as a decision point.

### Translator — field mapping

FGD properties (from `context/lib/build_pipeline.md` §Custom FGD) → `MapSmokeEmitter` / `SmokeEmitterRecord`:

| FGD property | Default | Validation | Notes |
|---|---|---|---|
| `origin` | required | parse as vec3; error if missing | already converted to engine-space by parser before `translate_smoke_emitter` is called (same contract as `translate_light`) |
| `rate` | 4.0 | must be `> 0` | sprites/sec |
| `lifetime` | 3.0 | must be `> 0` | seconds |
| `size` | 0.5 | must be `> 0` | world units |
| `speed` | 0.3 | finite | drift velocity |
| `collection` | required | non-empty UTF-8 | sprite-sheet subdirectory under `textures/` |
| `spec_intensity` | 0.3 | must be `>= 0` | Blinn-Phong scale |

Validation errors block compilation via `TranslateError`; warnings log and proceed with defaults. Same policy as `translate_light`.

### Loader → runtime wiring

After PRL decode, the main.rs startup path builds `App.smoke_emitters` from `level.smoke_emitters`:

```
// Proposed design
fn build_emitters_from_level(level: Option<&prl::LevelWorld>) -> Vec<fx::smoke::SmokeEmitter> {
    level
        .map(|w| w.smoke_emitters.iter().map(|e| fx::smoke::SmokeEmitter::new(
            fx::smoke::SmokeEmitterParams {
                origin: e.origin,
                rate: e.rate,
                lifetime: e.lifetime,
                size: e.size,
                speed: e.speed,
                collection: e.collection.clone(),
                spec_intensity: e.spec_intensity,
            }
        )).collect())
        .unwrap_or_default()
}
```

The renderer's existing `register_smoke_collection` loop in `App::resumed` (`main.rs` ~line 356 on `worktree-agent-a11c0602`) already iterates `self.smoke_emitters` and registers one collection per unique `collection` name. No renderer-side changes needed.

### Task A acceptance gates

- `env_smoke_emitter` placed in `assets/maps/test.map` (or a dedicated test map) compiles to a `.prl` whose `SmokeEmitters` section round-trips through `SmokeEmittersSection::{to_bytes, from_bytes}` (unit test in `smoke_emitters.rs`).
- Loading that `.prl` populates `App.smoke_emitters` with the matching origins, rates, and collections. `register_smoke_collection` is called once per unique collection name at startup.
- With `env_smoke_emitter` in the map, running `cargo run -p postretro -- assets/maps/test.prl` (no `--demo-smoke`) reproduces the Task A acceptance gates from `fx-volumetric-smoke` (camera-facing sprites, SH tint, chunk-list specular, fade in/out).
- `--demo-smoke` and `build_demo_emitters` are deleted. Missing `env_smoke_emitter` entities in a level produce an empty emitter list (no panic, no log noise beyond a single info line).
- Compiler errors out cleanly on `env_smoke_emitter` with missing `origin`, empty `collection`, or non-finite numeric props.

---

## Task B — `env_fog_volume` wire-up

**Crates:** `postretro-level-format` (new section module), `postretro-level-compiler` (parser brush retention + translator + pack), `postretro` (loader + App glue).

### Upstream gap: entity brushes are discarded

The current parser (`postretro-level-compiler/src/parse.rs:137–143`) records only `(classname, brush_count)` for non-worldspawn entities. Brush planes, sides, and vertices are never retained. The brush volumes themselves are computed from worldspawn only (parse.rs:105–110 and the downstream `world_brush_ids` loop at 203–336). Fog-volume AABB extraction needs per-brush bounds for each `env_fog_volume` brush entity. Fix this in the parser rather than routing fog geometry through the worldspawn BSP.

**Parser change.** Extend `EntityInfo` (`map_data.rs:122`) with `brush_aabbs: Vec<Aabb>` populated for non-worldspawn entities from `geo_map.entity_brushes` + `face_verts` (the same data the worldspawn loop consumes — already swizzled to engine space at `parse.rs:236`). Keep `entity_brushes: Vec<(String, usize)>` as-is for diagnostics or drop it as a follow-up; the new field supersedes it. (Pre-release: per `development_guide.md` §1.6, feel free to delete `entity_brushes` in the same pass — call sites are only `parse.rs:143` and `parse.rs:454`.)

Each `EntityInfo` that is `classname == "env_fog_volume"` with `!brush_aabbs.is_empty()` becomes one `FogVolume` record. Multiple brushes under one entity are typical in TrenchBroom; either merge them into a single union AABB or emit one record per brush. Recommend one record per brush — the runtime's point-in-AABB check is per-sample and has no dedup semantics; multiple disjoint brushes under one fog entity should each be its own AABB. Warn if the entity has no brushes.

### New / modified files

- `postretro-level-format/src/lib.rs` — add `FogVolumes = 25` to `SectionId` enum and `from_u32`. Add `WorldspawnProps = 26` (see worldspawn section below) in the same edit.
- `postretro-level-format/src/fog_volumes.rs` *(new)* — `FogVolumesSection` with `FogVolumeRecord` and `to_bytes` / `from_bytes`.
- `postretro-level-format/src/worldspawn.rs` *(new)* — `WorldspawnSection` carrying `ambient_color`, `fog_pixel_scale`, and `fog_step_size`. See rationale below.
- `postretro-level-compiler/src/map_data.rs` — add `MapFogVolume { aabb_min: DVec3, aabb_max: DVec3, color: [f32;3], density: f32, falloff: f32, scatter: f32 }`. Add `MapWorldspawn { ambient_color: [f32;3], fog_pixel_scale: u32, fog_step_size: f32 }`. Add `MapData.fog_volumes: Vec<MapFogVolume>` and `MapData.worldspawn: MapWorldspawn`.
- `postretro-level-compiler/src/format/quake_map.rs` — `translate_fog_volume(props, brush_aabbs) -> Result<Vec<MapFogVolume>, TranslateError>` and `translate_worldspawn(props) -> MapWorldspawn`.
- `postretro-level-compiler/src/parse.rs` — retain entity brush AABBs, route `env_fog_volume` through the translator, extract worldspawn props through `translate_worldspawn`.
- `postretro-level-compiler/src/pack.rs` — `encode_fog_volumes(&[MapFogVolume])` and `encode_worldspawn(&MapWorldspawn)`; append both section blobs to the pack calls.
- `postretro/src/prl.rs` — add `fog_volumes: Vec<FogVolumeData>` and `worldspawn: Worldspawn` to `LevelWorld`, decode both sections. Absent `WorldspawnSection` → engine defaults (`fog_pixel_scale = 4`, `fog_step_size = 0.5`, `ambient_color = [0,0,0]`).
- `postretro/src/main.rs` — after renderer construction, call `renderer.set_fog_pixel_scale(level.worldspawn.fog_pixel_scale)`, `renderer.set_fog_step_size(level.worldspawn.fog_step_size)`, and `renderer.set_fog_volumes(&level.fog_volumes)`. Remove any demo-fog seed if `fx-volumetric-smoke` introduced one (confirm with that plan's final implementation — it may have only a code-side const and no CLI flag).

### Byte layout — `FogVolumes` (ID 25), version 1

```
u32  volume_count
FogVolumeRecord[volume_count]         (fixed size: 40 bytes)
```

`FogVolumeRecord`:

```
f32[3]  aabb_min        (engine meters)
f32[3]  aabb_max        (engine meters)
f32[3]  color           (linear RGB, 0-1)
f32     density         (per-meter extinction coefficient)
f32     falloff         (volume edge falloff, units TBD; forwarded as-authored)
f32     scatter         (0-1 scatter fraction toward camera, default 0.6)
```

Per-record size: 40 bytes. Matches the packed `FogVolume` struct consumed by `render::fog_pass::FogPass::upload_volumes` in shape (not necessarily in memory layout — the loader converts). Format crate does not know the runtime packed layout; it owns the wire format only.

### Byte layout — `WorldspawnSection` (ID 26), version 1

Fixed-size 20 bytes:

```
f32[3]  ambient_color       (linear RGB)
u32     fog_pixel_scale     (clamped at load to 1..=8; raw u32 wire)
f32     fog_step_size       (meters; 0.5 default)
```

`ambient_color` is included because it is already documented in §Custom FGD as a worldspawn property and deserves a home. If another plan already added an ambient-color PRL pathway (grep for `ambient_color` in `postretro-level-format/src` to confirm), drop the field from this section and point at the existing one. Current grep (2026-04-19) shows no existing wire home.

### Translator — field mapping

`env_fog_volume` (brush entity) properties → `MapFogVolume`:

| FGD property | Default | Validation |
|---|---|---|
| brush AABBs | required | extracted from shambler; at least one brush |
| `color` | `[1,1,1]` | parse `_color`-style `"r g b"` (0-255 ints) or `"r g b"` 0-1 floats — pick one. Recommend 0-1 floats to match `fog_volume.wgsl` expectations. |
| `density` | 0.5 | must be `>= 0` |
| `falloff` | 1.0 | must be `>= 0` |
| `scatter` | 0.6 | clamp to `[0, 1]` |

`worldspawn` → `MapWorldspawn`:

| FGD property | Default | Validation |
|---|---|---|
| `ambient_color` | `[0,0,0]` | parse as RGB |
| `fog_pixel_scale` | 4 | integer; clamp `[1, 8]` with warning if out of range |
| `fog_step_size` | 0.5 | f32, must be `> 0`; not yet documented in FGD — add to §Custom FGD |

### Loader → runtime wiring

At level load, after the renderer is constructed (`App::resumed`):

```
// Proposed design
if let Some(level) = &self.level {
    renderer.set_fog_pixel_scale(level.worldspawn.fog_pixel_scale);
    renderer.set_fog_step_size(level.worldspawn.fog_step_size);
    let volumes: Vec<render::fog_pass::FogVolume> =
        level.fog_volumes.iter().map(fog_volume_data_to_gpu).collect();
    renderer.set_fog_volumes(&volumes);
}
```

The mapping helper converts `FogVolumeData` (PRL) → `FogVolume` (GPU packed) and lives in `postretro/src/fx/fog_volume.rs` (already present per the fog pass source). If that helper doesn't exist yet on the merged branch, add it there.

### Task B acceptance gates

- `env_fog_volume` brush in `assets/maps/test.map` compiles; the resulting `.prl` has a `FogVolumes` section with one record per brush (verified by unit test on `FogVolumesSection` round-trip + a compile-and-inspect test).
- `worldspawn` with `fog_pixel_scale 2` in the `.map` lands in `LevelWorld.worldspawn.fog_pixel_scale == 2`, and `renderer.set_fog_pixel_scale(2)` is called at startup — confirmed by log line or test.
- Running `cargo run -p postretro -- assets/maps/test.prl` (no demo flags) shows the fog pass active inside the brush region, reproducing the Task B acceptance gates from `fx-volumetric-smoke` (visible pixelated haze, spot-beam shafts, SH tint).
- Maps with no `env_fog_volume` entities produce no fog pass work — `renderer.set_fog_volumes(&[])` is a no-op; `FogPass::active()` returns false.
- Demo-fog seed (if any was introduced) is removed. Remaining CLI flags related to fog are only debug toggles, if any.
- Compiler emits a warning on `env_fog_volume` with no brushes, and an error on missing `density` / out-of-range `scatter`.

---

## Out of scope

- GPU-side smoke simulation. CPU ring buffer is sufficient (see `fx-volumetric-smoke` Task A).
- Per-volume `fog_pixel_scale` — this is a global render-target property. One render target, one divisor, on `worldspawn` by design.
- BSP-leaf membership for fog volumes. The runtime uses point-in-AABB per raymarch sample; BSP-leaf resolution is a stale idea from the FGD doc that predates the AABB buffer approach. Update `context/lib/build_pipeline.md` §Custom FGD fog-volume row to reflect AABB resolution as part of this plan.
- Compile-time validation that fog volumes sit inside the map hull.
- Ambient-color rendering semantics beyond wiring (i.e. don't change how `ambient_color` is evaluated in the shader; just give it a PRL home).
- Kinematic / destructible fog volumes. Baked-once is sufficient.
- Extending `MAX_FOG_VOLUMES` beyond 16 — overflow is already logged by `FogPass::upload_volumes`; retro-scale maps don't need more.
- Variable-length FGD properties beyond `env_smoke_emitter.collection`. If a future entity needs variable-length fields, revisit the shared-plumbing question (see above).

---

## Acceptance criteria (both tasks)

1. `cargo test --workspace` passes.
2. `cargo clippy --workspace -- -D warnings` clean.
3. No new `unsafe`.
4. Task A and Task B acceptance gates above.
5. `build_demo_emitters` and the `--demo-smoke` CLI flag are deleted from `postretro/src/main.rs`. Any equivalent demo-fog seed introduced by `fx-volumetric-smoke` is deleted.
6. A single TrenchBroom-authored test map (`assets/maps/test_fx.map` or extension of `assets/maps/test.map`) exercises:
   - at least one `env_smoke_emitter` with non-default `rate` and a `collection` that exists under `textures/`,
   - at least one `env_fog_volume` brush with non-default `density` and `scatter`,
   - a `worldspawn` with explicit `fog_pixel_scale` differing from the default.
   Running the engine on the compiled `.prl` visually reproduces every `fx-volumetric-smoke` acceptance gate that `--demo-smoke` / demo-fog previously stood in for.
7. `context/lib/build_pipeline.md` §Custom FGD updated: fog-volume row reads "resolved at compile time to world-space AABBs and fog parameters; uploaded as a compact storage buffer" (drop the BSP-leaf language); add `fog_step_size` to the `worldspawn` row; add a "Fog volumes are carried by the `FogVolumes` PRL section; `env_smoke_emitter` is carried by the `SmokeEmitters` PRL section; `worldspawn` scene-wide render settings are carried by the `Worldspawn` PRL section" line under §Entity resolution.
8. `context/lib/build_pipeline.md` §PRL section IDs table gets three new rows: `SmokeEmitters (24)`, `FogVolumes (25)`, `Worldspawn (26)`, each with an "always" / "when present" column entry matching behavior (smoke and fog are "when present"; worldspawn is "always").

---

## Open design questions to confirm before implementation

1. **Origin precision in `SmokeEmitterRecord`.** `f32[3]` (smaller, matches runtime consumer) or `f64[3]` (consistent with `AlphaLightRecord`)? Recommendation: `f32[3]`.
2. **One record per fog brush vs. union AABB per entity.** Recommendation: one record per brush.
3. **Does `ambient_color` already have a wire home?** Verify by greping `postretro-level-format/src` for `ambient_color` before adding `WorldspawnSection`. If yes, extend the existing section instead of adding ID 26.
4. **`fog_step_size` FGD property.** Not in `context/lib/build_pipeline.md` §Custom FGD today. Add it as a `worldspawn` property alongside `fog_pixel_scale`, or hardcode at `0.5` and drop the worldspawn field? Recommendation: add it — authors asked for a retunable beam density in `fx-volumetric-smoke` Task B.
5. **Retiring `MapData.entity_brushes`** (the diagnostic `Vec<(String, usize)>`). Pre-release policy says delete it and fix the one test assertion. Implementer should confirm no other consumers exist.
