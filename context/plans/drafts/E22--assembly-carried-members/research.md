# E22 Spec 1 — Assembly carried members — research

> Source-grounded seam map, lifecycle, and enumerations for `index.md`. Findings, not decisions. Identifiers read from source this session; cite by symbol — line numbers drift.

## Grounded seam map

### Compile — light path
- `is_light_classname` / `LIGHT_CLASSNAMES` (`format/quake_map.rs`) includes `light_dynamic`, `light_dynamic_spot`; `DYNAMIC_LIGHT_CLASSNAMES` is the dynamic subset; `is_dynamic` is classname-derived (`quake_map.rs`, `translate_light`).
- Light entities collected in the `parse_map_file` loop: `translate_light(&props, origin, &classname)` → pushed to `lights: Vec<MapLight>` (`parse.rs`, `is_light_classname` branch). `MapLight` (`map_data.rs`) is **typed fields only — no generic KVP bag**; a new KVP is read in `translate_light` via `props.get(..)` and stored as a new typed `MapLight` field, set at the constructor site in `quake_map.rs`.
- FGD home: the `@BaseClass = DynamicLight` block (`sdk/TrenchBroom/postretro.fgd`) — its comment already reserves this: *"Dynamic-only KVPs for light movement are deferred until that feature lands. The parser sets is_dynamic = true from the CLASSNAME, not a KVP."* `light_dynamic` / `light_dynamic_spot` derive from it.

### Compile — light → PRL
- Dynamic and baked lights **share** `AlphaLights` (id 18); `AlphaLightsNs::from_lights` keeps all `!bake_only` lights; dynamic distinguished only by per-record `is_dynamic` (`light_namespaces.rs`, `pack.rs::encode_alpha_lights`). `LightInfluence` (21) and `LightTags` (26) are index-aligned to `AlphaLights`. Dynamic lights are excluded from the baked namespaces (`!is_dynamic` filter) — they bake into nothing.
- `AlphaLightRecord` (`level-format/src/alpha_lights.rs`) carries `origin`, `light_type`, `intensity`, `color`, `falloff_*`, `cone_angle_inner/outer`, `cone_direction`, `is_dynamic`, `casts_entity_shadows`, `leaf_index`, `shadow_type`.

### Compile — mover path + wire
- `MapKinematicMover` (`map_data.rs`) already holds `brush_volumes: Vec<BrushVolume>`, `name`, `tags`, `origin`. Movers peeled in `parse.rs`, resolved post-loop by `resolve_kinematic_movers`.
- `KinematicGeometry` id 43, `KINEMATIC_GEOMETRY_VERSION = 4` (`level-format/src/kinematic_geometry.rs`). `write_mover` / `read_mover` gate trailing field groups on `version` (v2 spin, v3 blocking/events, v4 presence-tagged `auto_close_ms`); loader `from_bytes` rejects versions outside {1..4}. Extend = bump to 5, append a `write_mover` block guarded `if version >= V5`, default empty for v1–4 in `read_mover` — the established append pattern.
- **Two consumers not to break:** `KinematicBrushPass::install_geometry` (reads only geometry/material fields) and `level_content_digest` (`runtime_movers.rs`) which hashes a *subset* and already omits v3 event fields. Member-light linkage is presentation-only → **exclude from the digest** (mirrors the v3-event omission; matters for the multiplayer static-content gate).

### Runtime — mover seam
- Host: `snapshot_transforms` (`sim/mod.rs`, start of tick) sets mover `previous`; `run_kinematic_mover_tick` (`sim/mod.rs`, `kinematic_mover.rs`) writes each mover `Transform` in place.
- Render: `KinematicMoverRenderCollector::collect` (`runtime_movers.rs`) draws movers at `registry.interpolated_transform(id, alpha)` (`registry.rs`; lerp position, slerp rotation), `alpha = frame_result.alpha` (`main.rs`).
- Spawn: `spawn_loaded_kinematic_movers` → `spawn_from_geometry_with_auto_close_default` (`runtime_movers.rs`): one entity per mover (`try_spawn(transform, &mover.tags)`, `KinematicMoverComponent`), **no child entities**. Caller `startup/lifecycle*`.
- Client: mover `Transform` is **not** replicated; client re-simulates from replicated *phase* (`WireKinematicMoverState`) via the same `run_kinematic_mover_tick` (`main.rs::client_predict_loaded_movers_tick`), binds by `mover_id` (`netcode/client.rs`). So the interpolated mover pose exists on the client with no new wire.

### Runtime — light seam (the generalization target)
- Spawn: `LightBridge::populate_from_level` (`scripting/systems/light_bridge.rs`) spawns one `LightComponent` entity per `MapLight` via `map_light_to_component`; caller `startup/lifecycle*`.
- `LightComponent` (`entities/src/components/light.rs`) carries `origin`, `light_type`, `intensity`, `color`, `falloff_*`, `cone_*`, `is_dynamic`, `animated_slot`, **`follow_transform: bool`**, `animation`.
- Per-frame upload: `LightBridge::update(registry, current_time, alpha)` (`light_bridge.rs`, driven from `main.rs`). For each light it calls `follow_transform_position(registry, id, component, alpha)`; when `Some`, that position is written into the `GpuLight` origin **and** the culling `influence.center` (`light_bridge.rs`).
- `follow_transform_position` (`light_bridge.rs`): today reads the **same entity's** Transform — `SpriteVisual` → raw tick pose; `Mesh` → `interpolated_transform(id, alpha)`; else live Transform. Set `true` only at projectile spawn (`weapon_stage/commands.rs`, `netcode/projectile_presentation.rs`). **This is the exact hook to generalize** to "read the *carrier mover's* interpolated pose ∘ local offset."

## Design resolution (why the compose lives at upload, not a tick system)

The carried light's *visible* position must match the *drawn* mover geometry, which uses `interpolated_transform(mover, alpha)`. The light bridge already resolves a followed light's position per render frame at that same `alpha`. So composing `interpolated_transform(carrier_mover, alpha).transform_point(local_offset)` inside the bridge's follow hook is exact by construction and needs **no per-tick compose system** and no per-tick child-Transform bookkeeping. `local_offset = light.origin − mover.origin` (mover authored rotation is identity), so `transform_point` handles a spinning mover (E17-D) automatically: `world = mover.pos + mover.rot * local_offset`. Spot `cone_direction` is stored mover-local (authored == local at identity) and rotated by the mover's interpolated rotation in the same pass. Nothing reads a dynamic light's position at tick time (lights are presentation), so upload-time compose is sufficient.

## Lifecycle (host + client → one render compose)

```mermaid
sequenceDiagram
    participant Tick as Fixed tick
    participant Mover as Mover Transform (registry)
    participant Bridge as LightBridge::update (render, alpha)
    participant GPU as Renderer

    Note over Tick: HOST — sim/mod.rs
    Tick->>Mover: snapshot_transforms (previous := current)
    Tick->>Mover: run_kinematic_mover_tick (current advances)
    Note over Tick: CLIENT — main.rs (no new wire)
    Tick->>Mover: snapshot_transform + run_kinematic_mover_tick<br/>(re-derived from replicated phase)

    Note over Bridge: RENDER FRAME (both host & client)
    Bridge->>Mover: interpolated_transform(carrier_mover, alpha)
    Bridge->>Bridge: world_pos = mover_pose.transform_point(local_offset)<br/>world_dir = mover_rot * cone_direction (spot)
    Bridge->>GPU: GpuLight.origin = world_pos; influence.center = world_pos
```

No arrow without a read call site: `snapshot_transforms`/`snapshot_transform`, `run_kinematic_mover_tick`, `interpolated_transform`, `follow_transform_position`, `upload_bridge_lights` are all quoted in the seam map above.

## Observers × lifecycle

| Vantage | Behavior | Same as / differs |
|---|---|---|
| Host | Bridge composes `interpolated_transform(mover, alpha) ∘ offset` at render. | baseline |
| Connected client | Identical bridge compose; the mover Transform it reads was re-derived locally from replicated phase, not from the host pose. **No new wire.** Warrant: `client_predict_loaded_movers_tick` runs the same integrator; the linkage + light params ride the static PRL. |
| Renderer (interpolated) | Reads the *same* `alpha` and *same* mover entity the geometry draw uses → light and geometry never desync by a tick. | must be identical to the geometry draw's `interpolated_transform(id, alpha)` |
| Baked vs dynamic tier | Only `is_dynamic` lights carry. A baked light's contribution is baked static at its authored origin — it cannot move; binding one is warn+ignored at compile. | dynamic-only, enforced |

## Orderings

| Scenario | Ordering | Expected |
|---|---|---|
| Mover reverses (`ping_pong`, block reverse) | offset unchanged; mover pose reverses | Light tracks through reversal, no snap. |
| Mover completes (`once`) | mover holds at terminus | Light holds at composed terminus pose, not authored origin. |
| Render-only frame (0 fixed ticks) | `interpolated_transform` blends previous→current at `alpha` | Light composes from the interpolated mover pose, same as geometry. |
| Two fixed ticks in one frame | mover advanced twice before render | Light composes from post-second-tick current, interpolated to `alpha`. |
| Rotating mover (E17-D spin) | mover rotation non-identity | `transform_point` rotates the offset; spot direction rotates too. Test on a spinner. |
| `carrier` names no mover | resolved at compile | Warn (name the light + the missing name); light stays an unbound top-level dynamic light at authored origin. |
| `carrier` matches >1 mover (duplicate mover `name`s) | resolved at compile | Warn (name the duplicate-named movers); light unbound (a light cannot ride two movers). |
| `carrier` on a baked light (`light`/`light_spot`/`light_sun`) | resolved at compile | Warn + ignore binding; bakes as a normal static light (mirrors `_cast_entity_shadows` warn-clear on baked lights). |
| Blank/cleared `carrier` | `authored` helper returns default | Unbound normal dynamic light — never a parse error (mirrors trigger optional-KVP posture). |

## Binding precedent (mirror the KVP pattern; diverge on the referent)
- Triggers bind movers by **`_tags`** via `query_by_component_and_tag(ComponentKind::KinematicMover, Some(tag))` (`trigger_system.rs`, `registry.rs`) — a dedicated `target_tag(string)` KVP, not shared-tag inference. Mover `name` (FGD `name(string) : "Stable mover name"`) is authored but diagnostics-only at runtime (`runtime_movers.rs` uses it only in log strings).
- Blank-is-default helper `authored` (`trigger_volumes.rs`) — cleared KVP → declared default, never a compile error. Use it for `carrier`.
- **Divergence from triggers — bind by `name`, not `_tag`.** Triggers command *all* tag matches (fan-out is correct for a command); a carried light rides *exactly one* parent. Binding by the mover's unique `name` fits the 1:1 relation without importing the trigger fan-out vocabulary and then suppressing it, and without overloading `_tags` (a light sharing a script-query tag with a mover would otherwise be silently carried). Because carried-light resolution is **compile-time**, `name`'s runtime non-indexing is irrelevant — the compiler has every mover's `name` in scope. Resolution: `carrier` → the mover of that name; 0 matches or a duplicate-`name` collision → warn + unbound.
