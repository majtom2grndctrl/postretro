# E22 Spec 1 — Assembly carried members — research

> Source-grounded seam map, lifecycle, and enumerations for `index.md`. Findings, not decisions. Identifiers read from source this session; cite by symbol — line numbers drift.

## Grounded seam map

### Compile — light path
- `is_light_classname` / `LIGHT_CLASSNAMES` (`format/quake_map.rs`) includes `light_dynamic`, `light_dynamic_spot`; `DYNAMIC_LIGHT_CLASSNAMES` is the dynamic subset; `is_dynamic` is classname-derived (`quake_map.rs`, `translate_light`).
- Light entities collected in the `parse_map_file` loop: `translate_light(&props, origin, &classname)` → pushed to `lights: Vec<MapLight>` (`parse.rs`, `is_light_classname` branch). `MapLight` (`map_data.rs`) is **typed fields only — no generic KVP bag**; a new KVP is read in `translate_light` via `props.get(..)` and stored as a new typed `MapLight` field, set at the constructor site in `quake_map.rs`.
- FGD home: the `@BaseClass = DynamicLight` block (`sdk/TrenchBroom/postretro.fgd`) — its comment already reserves this: *"Dynamic-only KVPs for light movement are deferred until that feature lands. The parser sets is_dynamic = true from the CLASSNAME, not a KVP."* `light_dynamic` / `light_dynamic_spot` derive from it.

### Compile — light → PRL
- Dynamic and baked lights **share** `AlphaLights` (id 18); `AlphaLightsNs::from_lights` keeps all `!bake_only` lights; dynamic distinguished only by per-record `is_dynamic` (`light_namespaces.rs`, `pack.rs::encode_alpha_lights`). `LightInfluence` (21) and `LightTags` (26) are index-aligned to `AlphaLights`. Dynamic lights are excluded from the baked namespaces (`!is_dynamic` filter) — they bake into nothing.
- **Pack seam (verified):** `AlphaLightEntry.source_index` records each kept record's source-`MapLight` index — the inversion the linkage needs to map `source_light_index → AlphaLights` position while accounting for the `!bake_only` drop. In `pipeline.rs`, `alpha_lights_ns` is built (from `map_data.lights`) **before** `encode_kinematic_geometry_section(&map_data.kinematic_movers, …)`, so both are in scope at one seam. Carried lights are `is_dynamic` (hence `!bake_only`), so they are always present in this space.
- `AlphaLightRecord` (`level-format/src/alpha_lights.rs`) carries `origin`, `light_type`, `intensity`, `color`, `falloff_*`, `cone_angle_inner/outer`, `cone_direction`, `is_dynamic`, `casts_entity_shadows`, `leaf_index`, `shadow_type`.

### Compile — mover path + wire
- `MapKinematicMover` (`map_data.rs`) already holds `brush_volumes: Vec<BrushVolume>`, `name`, `tags`, `origin`. Movers peeled in `parse.rs`, resolved post-loop by `resolve_kinematic_movers`.
- `KinematicGeometry` id 43, `KINEMATIC_GEOMETRY_VERSION = 4` (`level-format/src/kinematic_geometry.rs`). `write_mover` / `read_mover` gate trailing field groups on `version` (v2 spin, v3 blocking/events, v4 presence-tagged `auto_close_ms`); loader `from_bytes` rejects versions outside {1..4}. Extend = bump to 5, append a `write_mover` block guarded `if version >= V5`, default empty for v1–4 in `read_mover` — the established append pattern.
- **Two consumers not to break:** `KinematicBrushPass::install_geometry` (reads only geometry/material fields) and `level_content_digest` (`runtime_movers.rs`). **Digest mechanism (verified):** `level_content_digest` iterates `geometry.movers` — the runtime `LoadedKinematicMover`, **not** the wire `KinematicMoverRecord` — and hashes named fields by **field access** (`mover_id`, `name`, `origin`, `path`, spin, `carry_yaw`, vertices, indices); block/event/timer fields are simply never read. This is an **allowlist**, not the doctrine denylist (`context/lib/networking.md`) — so a new `carried_lights` field is auto-excluded by *not adding a read*, with no compile error. Exclusion is by inaction. Presentation-only → correct to exclude; matters for the multiplayer static-content gate. (Do not cite "like the v3 event omission" as the warrant — the mechanism is the allowlist, not a named skip.)

### Runtime — mover seam
- Host: `snapshot_transforms` (`sim/mod.rs`, start of tick) sets mover `previous`; `run_kinematic_mover_tick` (`sim/mod.rs`, `kinematic_mover.rs`) writes each mover `Transform` in place.
- Render: `KinematicMoverRenderCollector::collect` (`runtime_movers.rs`) draws movers at `registry.interpolated_transform(id, alpha)` (`registry.rs`; lerp position, slerp rotation), `alpha = frame_result.alpha` (`main.rs`).
- Spawn: `spawn_loaded_kinematic_movers` → `spawn_from_geometry_with_auto_close_default` (`runtime_movers.rs`): one entity per mover (`try_spawn(transform, &mover.tags)`, `KinematicMoverComponent`, `rotation: Quat::IDENTITY, scale: Vec3::ONE`), **no child entities**. Returns `Vec<EntityId>` **order-aligned with `geometry.movers`** (not keyed by `mover_id`; zip to recover it). On registry exhaustion returns `Err`; the caller logs and continues with **zero** movers spawned. Caller: **Segment B of the CPU world install** (`startup/lifecycle_world_cpu.rs`).
- **Spawn ordering + resolution seam (verified — load-bearing):** lights spawn in the renderer install (`LightBridge::populate_from_level_with_influences`, `startup/lifecycle.rs`, inside `install_level_payload`) **before** movers spawn (`spawn_loaded_kinematic_movers`, Segment B, `lifecycle_world_cpu.rs::install_world_cpu`). So at the light-populate call site the movers do **not** exist. And at the mover spawn site the light bridge is **not** in scope (it lives on `session.light_bridge`, one scope up) and the spawned `Vec<EntityId>` is **logged and dropped** — the `Ok(spawned)` arm only records `spawned.len()`; `WorldInstallProducts` (fields: `mover_colliders`, `trigger_bindings`, `trigger_pool_report`, `mover_tick_states`, `first_spawn`, `spawn_points`) does not carry it. So the resolution pass cannot live at the spawn site. It must: (1) `install_world_cpu` surfaces the spawned `Vec<EntityId>` (order-aligned with `geometry.movers`) on `WorldInstallProducts`; (2) the pass runs in `install_level_payload` **after** `install_world_cpu` returns, where `session.light_bridge` is in scope, and **before** the event loop's first `LightBridge::update` (`main.rs`). `install_level_payload` is the single synchronous funnel for all level loads (initial, reload, dev-cycle, host relevel, connected client via `suppress_*` flags); unload clears bridge+registry (`lifecycle_net.rs`). So one pass in that funnel covers every mover-spawn path — no per-install-site attachment. **Headless caveat:** the headless driver (`observability::driver`) calls `install_world_cpu` directly and never populates a light bridge, so carrier *binding* is windowed-only (test implication).
- Client: mover `Transform` is **not** replicated; client re-simulates from replicated *phase* (`WireKinematicMoverState`) via the same `run_kinematic_mover_tick` (`main.rs::client_predict_loaded_movers_tick`), binds by `mover_id` (`netcode/client.rs`). So the interpolated mover pose exists on the client with no new wire.

### Runtime — light seam (the generalization target)
- Spawn: `LightBridge::populate_from_level` (`scripting/systems/light_bridge.rs`) spawns one `LightComponent` entity per `MapLight` via `map_light_to_component`; caller `startup/lifecycle*`.
- `LightComponent` (`entities/src/components/light.rs`) carries `origin`, `light_type`, `intensity`, `color`, `falloff_*`, `cone_*`, `is_dynamic`, `animated_slot`, **`follow_transform: bool`**, `animation`.
- Per-frame upload: `LightBridge::update(registry, current_time, alpha)` (`light_bridge.rs`, driven from `main.rs`). For each light it calls `follow_transform_position(registry, id, component, alpha)`; when `Some`, that position is written into the `GpuLight` origin **and** the culling `influence.center` (`light_bridge.rs`).
- `follow_transform_position` (`light_bridge.rs`): today gated on `component.follow_transform` (`bool`, `#[serde(default)]` on `LightComponent`), reads the **same entity's** Transform — `SpriteVisual` → raw tick pose; `Mesh` → `interpolated_transform(id, alpha)` (returns `Result`, consumed via `.ok().map(..)`); else live Transform. Set `true` only at projectile spawn. **This is the exact hook to generalize** to a **carrier-first** branch (mutually exclusive with the `follow_transform` path) reading the *carrier mover's* interpolated pose ∘ local offset.
- **Compose expression (verified):** the engine `Transform` (`registry.rs`) is a plain `{ position: Vec3, rotation: Quat, scale: Vec3 }` with **no `transform_point` method** (that method is glam `Mat4::transform_point3` / nalgebra `Isometry`, not `Transform`). Compose is `t.position + t.rotation * Vec3::from(local_offset)` behind `.ok().map(..)`; movers never scale. `rot·offset` gives the spinning-mover orbit.
- **Dirty gate (verified — bounds the spot scope):** the bridge marks itself dirty only when the followed **position** changes (`cached_follow_positions[map_idx] != followed_position`), and the renderer skips the upload when `!self.dirty`. There is no cache/compare for rotation or direction. A carried spot with a constant position but rotating aim (axial offset on a spinner) would never re-upload its cone. Spec 1 therefore scopes carried spots to translating movers (position always changes → as-authored aim rides along) and warns on a spinner-capable carrier; dirty-on-aim machinery defers.
- **Spinner capability = non-zero `spin_axis` (verified — bounds the warn key):** the compiler's `parse_kinematic_spin_axis` (`parse.rs`) returns the authored `spin_axis` even when `spin_speed == 0`, and the axis rides the section. At runtime a `moverSetSpinRate` trigger reaction (`kinematic_mover/commands.rs`, bound via `trigger_bindings.rs`) sets the spin rate **gated only on a non-zero `spin_axis`**. So a mover authored `spin_axis "0 0 1"` / `spin_speed 0` passes a `spin_speed_deg_s != 0` check yet can spin at runtime — silently freezing a carried spot's aim. The complete capability test is `spin_axis != 0` (a zero-axis mover can never rotate, by authoring or by command). `MapKinematicMover` carries `spin_axis` and `spin_speed_deg_s` readable at the compile-time resolution pass; spot-ness is knowable via `MapLight.light_type == LightType::Spot`.
- `LightComponent.cone_direction: Option<[f32;3]>` is authored world-space; for a carried spot on a non-spinning mover (identity rotation) the authored aim stays correct with no runtime rotation, so spec 1 packs it as-authored.
- `LightBridge::entity_for_map_index(map_index) -> Option<EntityId>` (`light_bridge.rs`) resolves an `AlphaLights`-positional index to the spawned light entity — the accessor the post-mover-spawn resolution pass uses.

## Design resolution (why the compose lives at upload, not a tick system)

The carried light's *visible* position must match the *drawn* mover geometry, which uses `interpolated_transform(mover, alpha)`. The light bridge already resolves a followed light's position per render frame at that same `alpha`. So composing `interpolated_transform(carrier_mover, alpha).ok().map(|t| t.position + t.rotation * local_offset)` inside the bridge's follow hook is exact by construction and needs **no per-tick compose system** and no per-tick child-Transform bookkeeping. `local_offset = light.origin − mover.origin` (mover **spawn** rotation is identity — no authored rotation field), so the compose handles a spinning mover (E17-D) automatically: `world = mover.pos + mover.rot · local_offset`. Spot `cone_direction` is packed **as-authored** in spec 1: for a carried spot on a non-spinning mover the authored world aim is correct with no rotation, and carrying aim under a spinning mover is deferred (dirty-gate limit above). Nothing reads a dynamic light's position at tick time (lights are presentation), so upload-time compose is sufficient.

**Verified frame ordering (the spine):** `LightBridge::update` and `KinematicMoverRenderCollector::collect` both run **after** the fixed-tick loop closes (`main.rs`), and nothing between them calls `snapshot_transforms` or mutates a mover `Transform`. So the light compose and the geometry draw read the **same** `previous`/`current` pair and the **same** `frame_result.alpha` for the same mover entity — they cannot desync by a tick. Spawn seeds `previous == current` and `interpolated_transform` does `unwrap_or(current)` for a never-snapshotted slot, so on a 0-tick first frame the composed light sits at `mover.origin + I·offset = light.origin` (at-rest AC). On the client, `client_predict_loaded_movers_tick` snapshots then advances each mover, so both the carried light and the mover geometry read the client's **own** `interpolated_transform` — they may differ from the host's absolute pose but never desync from each other intra-client.

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
    Bridge->>Bridge: world_pos = t.position + t.rotation * local_offset<br/>(spot cone_direction packed as-authored; no rotation in spec 1)
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

Pin table — each row concrete enough to write a test from; Task 4 references these.

| Scenario | Ordering (concrete sequence) | Expected |
|---|---|---|
| Load order: lights before movers | `populate_from_level` spawns lights (`lifecycle.rs`); **then** `spawn_loaded_kinematic_movers` (Segment B, `lifecycle_world_cpu.rs`); **then** carrier-resolution pass | Every carried light's `carrier.mover_entity` == the `EntityId` the collector iterates for that `mover_id`; resolution never runs before mover spawn. |
| First render frame, 0 ticks, never snapshotted | level loads; first redraw with `ticks == 0`, before any `snapshot_transforms` | `interpolated_transform(mover)` returns `current` (spawn pose); composed light position == authored `light.origin`, byte-identical with the mover pose at same alpha. |
| Mover reverses (`ping_pong`, block reverse) | offset unchanged; mover pose reverses across a tick, render straddles at `alpha` | Light tracks through reversal, no snap to authored origin; light == geometry at every alpha. |
| Mover completes (`once`) / block-stop hold | mover holds at terminus (`previous == current == terminus`) | Light holds at composed terminus pose (bridge not dirty, last upload is terminus), not authored origin. |
| Render-only frame (0 fixed ticks) | `interpolated_transform` blends previous→current at `alpha` | Light composes from the interpolated mover pose, same as geometry. |
| Two fixed ticks in one frame | mover snapshot then `run_kinematic_mover_tick` twice before render | Light composes from post-second-tick current slerped from post-first-tick previous at `alpha` — identical to the geometry draw. |
| Spinning mover (E17-D), carried **omni** | mover rotation non-identity | `pos + rot·offset` orbits the offset around the spin axis at authored radius. Test on a spinner. |
| Carried **spot** on a spinner-capable mover | resolved at compile (mover `spin_axis != 0`, any `spin_speed`) | Warn (spot + spinner-capable carrier); light binds and carries position (orbit), cone aim **not** re-rotated (deferred). |
| Carried **spot** on a translating mover (zero `spin_axis`) | position changes each frame → dirty fires; aim as-authored | Cone tracks position and holds authored aim; re-uploads because position moves. |
| Runtime-commanded spin on a compile-time non-spinner carrying a spot | compile: `spin_axis "0 0 1"`, `spin_speed 0` — but `spin_axis != 0` so the spinner-capable warn fires; load; a `moverSetSpinRate` trigger takes (axis non-zero) → mover rotates | Guard is at compile via the `spin_axis` capability key, so the mapper is already warned; no silent aim freeze. (A `spin_speed`-keyed warn would have missed this.) |
| Reload / host level change | load → carrier set; unload (`lifecycle_net.rs` clears bridge+registry) → reload through `install_level_payload` → resolution re-runs | Each carried light re-resolves against the freshly spawned mover; no carrier retains a pre-reload `EntityId`. |
| Failed (zero) mover spawn with carried linkages present | `geometry.movers` non-empty with `carried_lights`; `spawn_loaded_kinematic_movers` returns `Err` (registry exhausted **or any per-mover load fault** — spin/accel rounding to zero rad/s, initial spin with zero axis, bad waypoint chain/mode) → **zero** entities (all-or-nothing, never partial); pass runs over an empty `mover_id→EntityId` map | Every carrier `None` (authored origin); the resolution pass logs one runtime warning per unbound carried linkage. Never bound to a misaligned entity. |
| Resolution-pass placement / co-scope | pass needs both the spawned `Vec<EntityId>` and `session.light_bridge`; `install_world_cpu` exposes neither at the spawn site | Pass runs in `install_level_payload` after `install_world_cpu` returns (spawned `Vec` surfaced on `WorldInstallProducts`), synchronously **before** the first `LightBridge::update`; no update serviced between light spawn and carrier set → no one-frame authored-origin pop. |
| Headless install of a carried-light map | `observability::driver` calls `install_world_cpu` directly; light bridge never populated | Resolution no-ops (no light entities); no panic; carrier binding is documented windowed-only (parity test uses a windowed/loopback harness for the binding half). |
| Mover geometry culled, light in view | mover AABB outside `visible_cells` so its draw is skipped | Light still composes: bridge iterates its own list; for a bound carrier `interpolated_transform(mover)` is always available (mover entity spawned). |
| Host block vs client unreconciled window | host stops mover on tick N; client phase not yet reconciled | Carried light and mover geometry both read the client's own `interpolated_transform(mover, alpha)` → match each other exactly (host/client absolute pose may differ; intra-client never desyncs). |
| `carrier` names no mover | resolved at compile | Warn (name the light + the missing name); light stays an unbound top-level dynamic light at authored origin. |
| `carrier` matches >1 mover (duplicate mover `name`s) | resolved at compile | Warn (name the duplicate-named movers); light unbound (a light cannot ride two movers). |
| `carrier` on a baked light (`light`/`light_spot`/`light_sun`) | resolved at compile | Warn + ignore binding; bakes as a normal static light (mirrors `_cast_entity_shadows` warn-clear on baked lights). |
| Blank/cleared `carrier` | `authored` helper returns default | Unbound normal dynamic light — never a parse error (mirrors trigger optional-KVP posture). |

## Binding precedent (mirror the KVP pattern; diverge on the referent)
- Triggers bind movers by **`_tags`** via `query_by_component_and_tag(ComponentKind::KinematicMover, Some(tag))` (`trigger_system.rs`, `registry.rs`) — a dedicated `target_tag(string)` KVP, not shared-tag inference. Mover `name` (FGD `name(string) : "Stable mover name"`) is authored but diagnostics-only at runtime (`runtime_movers.rs` uses it only in log strings).
- Blank-is-default helper `authored` (`trigger_volumes.rs`) — cleared KVP → declared default, never a compile error. Use it for `carrier`.
- **Divergence from triggers — bind by `name`, not `_tag`.** Triggers command *all* tag matches (fan-out is correct for a command); a carried light rides *exactly one* parent. Binding by the mover's unique `name` fits the 1:1 relation without importing the trigger fan-out vocabulary and then suppressing it, and without overloading `_tags` (a light sharing a script-query tag with a mover would otherwise be silently carried). Because carried-light resolution is **compile-time**, `name`'s runtime non-indexing is irrelevant — the compiler has every mover's `name` in scope. Resolution: `carrier` → the mover of that name; 0 matches or a duplicate-`name` collision → warn + unbound.
