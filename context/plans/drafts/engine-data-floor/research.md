# Research — engine-data floor extraction

Source-grounded findings (file:line confirmed against `crates/postretro/src/`). Feeds `index.md`; not the spec contract. Supersedes the floor assumptions in `context/plans/ready/scripting-core-extraction/research.md` (which assumed `components/` was a clean VM-free leaf — false).

## Why this plan exists

The `scripting-core-extraction` spec assumed `scripting/components/` was pure VM-free data movable wholesale into one `entity-core` crate. Implementation (Phase 2) blocked: several components couple to engine subsystems and to the IR substrate, and `ComponentValue` embeds every component **by value**, so the registry transitively pulls those couplings. The floor is bigger and layered. This plan re-scopes it as a deliberate foundation.

## The hard structural facts

1. **`ComponentValue` embeds by value, no indirection.** `registry.rs:167` — e.g. `PlayerMovement(Box<PlayerMovementComponent>)` (`:179`), `Health(HealthComponent)` (`:183`), `Agent(AgentComponent)` (`:184`). No `Box<dyn>` / handle that would break the transitive pull. So whatever a component pulls, the registry (and any crate owning it) pulls.

2. **The movement/IR cluster is a mutual knot.** `components/player_movement.rs:18` → `movement::MovementScope`; `movement/scope.rs:17` → `components::player_movement::PlayerMovementComponent`. Bidirectional. `MovementScope` is **behavior** (`impl BindingScope`), not POD. Both also depend on the IR substrate core. The three must be co-located.

3. **`player_movement` / `MovementScope` / `movement/` do NOT import `EntityId`/`EntityRegistry`/`ComponentValue`** (grep: zero hits across `movement/` and `player_movement.rs`). So the cluster has a clean **one-way** edge: `registry` → cluster, never back. This is what makes a layered floor possible.

4. **IR substrate core is VM-free and noun-free.** `ir/{mod,bind,eval,scope,load}.rs` import only intra-`ir` `super::*` + serde/thiserror/log; mlua appears only in `#[cfg(test)]`. They do **not** import `registry`/`components`/`ComponentValue`. `ir/scopes.rs` (`StoreScope`) is the exception — it pulls `ScriptCtx`/`slot_table`/`primitives::store` (`ir/scopes.rs:16-18`) and **stays in the runtime crate**.

5. **`SystemCommandQueue` is clean.** Every `SystemReactionCommand` variant carries only `String`/`f32`/`Option`/`[f32;N]`/`serde_json::Value` (`reactions/system_commands.rs:20-100`). No audio/input/UI/lifecycle subsystem type embedded — the app-side drain maps the scalar payload to subsystems. No inversion needed.

6. **No `ComponentValue` variant needs `render::ui`.** No file in `scripting/components/` imports `render::ui` or any UI descriptor. The UI types reach `data_descriptors` only through the *manifest* path (`RegisteredUiTree`/`LevelManifest`), never through a component. So the floor can exclude UI descriptors without breaking `ComponentValue`.

## The cycle sweep (7 cycles, each with a fix)

| # | Edge | Type | Fix |
|---|---|---|---|
| 1 | `player_movement` ↔ `movement::MovementScope` | behavior (mutual) | Co-locate the cluster in the substrate crate (no inversion). |
| 2 | `DataRegistry` → `data_descriptors` → `render::ui::{descriptor,layout,style_ranges}` (`data_descriptors/mod.rs:31-39`) | module-coupling (DataRegistry drops `ui_trees` at `data_registry.rs:88`; doesn't store them) | Split `data_descriptors`: POD descriptor **types** → substrate; VM converters + UI-manifest types stay in runtime crate. |
| 3 | `DataRegistry` → `data_descriptors` → `mlua`/`rquickjs` (`data_descriptors/mod.rs` top, for `js/`+`lua/` converters) | behavior (converters) | Same split as #2 — types are POD; converters stay up. |
| 4 | `DataRegistry` → `runtime::ModMapEntry` → VMs (`data_registry.rs:13`; `runtime/types.rs:19-20` imports luau/quickjs) | POD dragged through a VM-coupled module | Sink `ModMapEntry` POD into the substrate. `MenuCamera`/`Frontend` are NOT stored by `DataRegistry` and stay in `postretro`. |
| 5 | `data_descriptors/validate.rs:135` → `movement::MovementScope` (binds dash IR at declaration) | behavior (validation) | Resolved by the movement cluster living in the substrate. **`validate.rs` is a mixed file** — split it: pure numeric/crossing/IR validators descend; `mlua::Table`-coupled validators (e.g. `validate_dense_lua_array`, `validate.rs:22-30`) stay runtime-side. |
| 6 | `components/health.rs:14` → `weapon::DamagePayload` (param of `apply_damage`, `health.rs:121`; not a stored field) | POD + behavior-only edge | Sink `DamagePayload` (`{amount:f32}`) POD into the substrate; `apply_damage` stays with the health component. |
| 7 | `components/agent.rs:20` → `nav::NavAgentParams` (ctor `from_nav_params`, `:126`; component stores unpacked scalars) | POD + behavior-only edge | Sink `NavAgentParams` (4 `f32`) POD into the substrate. |
| 8 | `SequenceStep.id: EntityId` (`data_descriptors/types/reactions.rs:21`); `NamedReaction`/`ReactionDescriptor::Sequence` transitively; `DataRegistry` stores `Vec<NamedReaction>` | by-value UP edge (substrate→data-model) — an 8th cycle the 7 in-place fixes don't break | **Placement, not an in-place move:** the reaction/crossing descriptor types reference `EntityId`, so they live in the **data-model** crate with the registry, not the substrate. `EntityId` stays the data-model's opaque handle. |

`Vec3Lit`/`EulerDegrees` (`conv.rs:24-31,99-128`) are POD but live in VM-coupled `conv.rs` (imports mlua/rquickjs at `conv.rs:5-6`); stored by value in `light`/`billboard_emitter`. **Relocate the types** to the substrate; their FFI impls feature-gate (see orphan-rule contract).

## Layering verdict and membership

Two crates. Dependency flows one way: data-model → substrate → (leaf deps).

### Lower: substrate crate (working name `postretro-sim-substrate`)
Pure data + the IR evaluator; no registry, no VM, no hardware subsystem.
- IR core: `ir/{mod,bind,eval,scope,load}.rs` (`IrNode`/`IrValue`/`BakedIr`/`CURRENT_IR_VERSION`/`BindingScope`/`bind`/`eval`). Excludes `ir/scopes.rs`.
- Movement cluster: `MovementScope` (`movement/scope.rs`) + `PlayerMovementComponent`/`MovementState`/`DashPrograms` (`components/player_movement.rs`).
- **`EntityId`-free** POD descriptor **types** (the structs in `data_descriptors/types/*.rs`): movement params + `NumberOrIr`/`BoolOrIr`/`DashParams`, `WeaponDescriptor`/`FireMode`/`ResolutionMode`, `MeshDescriptor`/`AnimationState`/`InterruptPolicy`, `HealthDescriptor`/`HitboxDescriptor`, `AiDescriptor`/`AiStateNames`, `LightDescriptor`, `EntityTypeDescriptor`, POD manifest types `ModThemeTokens`/`ModFontAssets`. **NOT** the reaction/crossing descriptors (`NamedReaction`/`CrossingCondition`/`CrossingDescriptor`/`PrimitiveDescriptor`/`ProgressDescriptor`/`SequenceStep`) — they reference `EntityId`, so they go to the data-model crate (cycle #8). Plus the *pure* validators only (`DescriptorError`; numeric/crossing/IR validators carved from `validate.rs`, leaving the `mlua`-coupled ones runtime-side).
- POD value types: `Vec3Lit`/`EulerDegrees` (relocated out of `conv.rs`). `Vec3Lit` is stored by value in `light`/`billboard_emitter`; `EulerDegrees` is FFI-boundary-only (not stored in any component).
- Sunk subsystem PODs: `DamagePayload`, `NavAgentParams`, `ModMapEntry`. (`MenuCamera`/`Frontend` are not consumed by the floor and stay in `postretro`.)

### Upper: data-model crate (working name `postretro-entity-core`)
VM-free; depends on the substrate.
- `registry.rs` (`EntityId`, `ComponentKind`, `ComponentValue`, `EntityRegistry`, `RegistryError`, `Transform`, `FogVolumeComponent`).
- Remaining `components/*` data structs (light, billboard_emitter, mesh, health, agent, brain, particle, sprite_visual, weapon, fog_volume) and their behavior fns that take `&EntityRegistry` (`apply_damage`, `attach_agent`, mesh/brain fns).
- The **reaction/crossing descriptor types** (`SequenceStep`/`NamedReaction`/`ReactionDescriptor`/`PrimitiveDescriptor`/`ProgressDescriptor`/`CrossingCondition`/`CrossingDescriptor`) — they embed `EntityId` (cycle #8), so they sit with the registry.
- `ctx.rs` (`ScriptCtx`), `slot_table.rs` + `engine_state_catalog.rs`, `provenance.rs`, `scripting/error.rs` (`ScriptError` — distinct from `data_descriptors/error.rs` `DescriptorError`, which descends), `reactions/system_commands.rs` (`SystemCommandQueue` + command enum), and `DataRegistry` (already slim — it stores only POD descriptors + `ModMapEntry` and destructures `ui_trees` away at `data_registry.rs:88`; `LevelManifest` itself stays runtime-side and the population destructuring moves there).

### Stays in the VM-coupled runtime crate (`postretro` today; later `scripting-core`)
`ir/scopes.rs` (`StoreScope`); `conv.rs` FFI impls + the json bridges; `data_descriptors/{js,lua}/` converters + `mod.rs` VM/`render::ui` glob; `RegisteredUiTree`/`LevelManifest` (UI-embedding); `runtime/*`; `luau.rs`/`quickjs.rs`/`primitives/*`/`primitives_registry.rs`/`reaction_dispatch.rs`/typedef generator.

## Orphan-rule contract, per crate

Each floor crate owns the FFI marshalling impls (`FromJs`/`IntoJs`/`FromLua`/`IntoLua`) for the types it defines, behind an optional `script-ffi` feature that pulls `rquickjs`/`mlua`. Only the runtime crate enables the feature. Substrate: FFI for `Vec3Lit`/`EulerDegrees` + the `EntityId`-free descriptor types. Data-model: FFI for `EntityId`/`Transform`/`ComponentKind`/`ComponentValue` + component types + reaction/crossing descriptors. The data-model depends on the substrate with `default-features = false` and forwards its own `script-ffi` to `postretro-sim-substrate/script-ffi` (never enabling it unconditionally), so a default `cargo tree` of the data-model stays VM-free. Default builds of both crates have no VM deps — that is the firewall. Precedent: `crates/level-format`'s optional `serde`/`gltf-resolve` features.

## Inversion call-site map (consumers flip to the floor — all cycle-free)

- **movement** (`mod/dispatch/intents/substrate/carry.rs`): become floor-consumers; `scope.rs` moves into the substrate. No remaining movement file is pulled back into the floor. Imports of `MovementScope`/`PlayerMovementComponent`/movement descriptors re-point to the floor crates.
- **nav** (`mod.rs`/`path.rs`): pure consumer once `NavAgentParams` sinks. nav imports no components.
- **weapon** (`damage/impact/mod.rs`): pure consumer once `DamagePayload` sinks; consumes `WeaponDescriptor`/`FireMode`/`ResolutionMode` from the floor.
- **ai** (`scripting/systems/ai.rs`): pure consumer; imports `AiDescriptor`/registry/components/`DamagePayload` from the floor.
- **opaque-handle consumers** — `render` (only `#[cfg(test)]` refs), `audio` (none), `netcode` (`EntityId`/`EntityTypeDescriptor`/`NavAgentParams` as data), `agent_steering` (handles), `collision` (none): all depend up cleanly. None defines a type the floor needs back.

## Naming

Working names `postretro-sim-substrate` (lower) / `postretro-entity-core` (upper); final decided at implementation. The lower crate is wider than "movement" or "IR" alone (it's the leaf-data + evaluation substrate); the upper is the entity/scripting data model. Rename either if it reads truer.
