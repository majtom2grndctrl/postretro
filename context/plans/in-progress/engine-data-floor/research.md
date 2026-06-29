# Research — engine-data floor extraction

Source-grounded findings (file:line confirmed against `crates/postretro/src/`). Feeds `index.md`; not the spec contract. Supersedes the floor assumptions in `context/plans/drafts/scripting-core-extraction/research.md` (which assumed `components/` was a clean VM-free leaf — false).

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
| 1 | `player_movement` ↔ `movement::MovementScope` | behavior (mutual) | Co-locate the cluster in `postretro-foundation` crate (no inversion). |
| 2 | `DataRegistry` → `data_descriptors` → `render::ui::{descriptor,layout,style_ranges}` (`data_descriptors/mod.rs:31-39`) | module-coupling (DataRegistry drops `ui_trees` at `data_registry.rs:88`; doesn't store them) | Split `data_descriptors`: POD descriptor **types** → `postretro-foundation`; VM converters + UI-manifest types stay in runtime crate. |
| 3 | `DataRegistry` → `data_descriptors` → `mlua`/`rquickjs` (`data_descriptors/mod.rs` top, for `js/`+`lua/` converters) | behavior (converters) | Same split as #2 — types are POD; converters stay up. |
| 4 | `DataRegistry` → `runtime::ModMapEntry` → VMs (`data_registry.rs:13`; `runtime/types.rs:19-20` imports luau/quickjs) | POD dragged through a VM-coupled module | Sink `ModMapEntry` POD into `postretro-foundation`. `MenuCamera`/`Frontend` are NOT stored by `DataRegistry` and stay in `postretro`. |
| 5 | `data_descriptors/validate.rs:135` → `movement::MovementScope` (binds dash IR at declaration) | behavior (validation) | Resolved by the movement cluster living in `postretro-foundation`. **`validate.rs` is a mixed file** — split it: pure numeric/crossing/IR validators descend; `mlua::Table`-coupled validators (e.g. `validate_dense_lua_array`, `validate.rs:22-30`) stay runtime-side. |
| 6 | `components/health.rs:14` → `weapon::DamagePayload` (param of `apply_damage`, `health.rs:121`; not a stored field) | POD + behavior-only edge | Sink `DamagePayload` (`{amount:f32}`) POD into `postretro-foundation`; `apply_damage` stays with the health component. |
| 7 | `components/agent.rs:20` → `nav::NavAgentParams` (ctor `from_nav_params`, `:126`; component stores unpacked scalars) | POD + behavior-only edge | Sink `NavAgentParams` (4 `f32`) POD into `postretro-foundation`. |
| 8 | `SequenceStep.id: EntityId` (`data_descriptors/types/reactions.rs:21`); `NamedReaction`/`ReactionDescriptor::Sequence` transitively; `DataRegistry` stores `Vec<NamedReaction>` | by-value UP edge (`postretro-foundation`→`postretro-entities`) — an 8th cycle the 7 in-place fixes don't break | **Placement, not an in-place move:** the reaction/crossing descriptor types reference `EntityId`, so they live in `postretro-entities` with the registry, not `postretro-foundation`. `EntityId` stays the entities crate's opaque handle. |
| 9 | `EntityTypeDescriptor.emitter: Option<BillboardEmitterComponent>` (`data_descriptors/types/entity.rs:216`); `BillboardEmitterComponent` (entities) imports `ScriptError`→`EntityId` | `EntityId`-free descriptor embedding an entities component → UP edge | **Placement:** `EntityTypeDescriptor` → `postretro-entities`. The naïve "`EntityId`-free ⇒ foundation" rule misses this; the rule is "references *only* foundation types ⇒ foundation." |
| 10 | `MeshDescriptor` references `AnimationState`/`InterruptPolicy` (`components/mesh.rs:28,44`, an entities module) | `EntityId`-free descriptor referencing entities state enums → UP edge | **Placement:** `MeshDescriptor` → `postretro-entities` (the mesh state enums stay in `components/mesh.rs`). |

Round-2 correction: cycles 9–10 are the same class as 8 — a descriptor that is `EntityId`-free but aggregates an entities-resident type. The partition rule is therefore "foundation only if it references *only* foundation-resident types," not "`EntityId`-free." Separately, `data_descriptors/validate.rs` is **three-way** coupled, not two-way: pure numeric/IR validators → foundation; `build_crossing` (constructs `CrossingDescriptor`) → entities; `validate_dense_lua_array` (`mlua`) + `parse_*` (`render::ui`) → runtime. And `data_descriptors/error.rs` keeps its `js_err`/`lua_err` VM adapters runtime-side while only `DescriptorError` descends.

`Vec3Lit`/`EulerDegrees` (`conv.rs:24-31,99-128`) are POD but live in VM-coupled `conv.rs` (imports mlua/rquickjs at `conv.rs:5-6`); stored by value in `light`/`billboard_emitter`. **Relocate the types** to `postretro-foundation`; their FFI impls feature-gate (see orphan-rule contract).

## Layering verdict and membership

Two crates. Dependency flows one way: `postretro-entities` → `postretro-foundation` → (leaf deps).

### Lower: `postretro-foundation`
Pure data + the IR evaluator; no registry, no VM, no hardware subsystem.
- IR core: `ir/{mod,bind,eval,scope,load}.rs` (`IrNode`/`IrValue`/`BakedIr`/`CURRENT_IR_VERSION`/`BindingScope`/`bind`/`eval`). Excludes `ir/scopes.rs`.
- Movement cluster: `MovementScope` (`movement/scope.rs`) + `PlayerMovementComponent`/`MovementState`/`DashPrograms` (`components/player_movement.rs`).
- **Foundation-clean** POD descriptor **types** (the structs in `data_descriptors/types/*.rs` that reference *only* foundation-resident types): movement params + `NumberOrIr`/`BoolOrIr`/`DashParams`, `WeaponDescriptor`/`FireMode`/`ResolutionMode`, `HealthDescriptor`/`HitboxDescriptor`, `AiDescriptor`/`AiStateNames`, `LightDescriptor`, POD manifest types `ModThemeTokens`/`ModFontAssets`. **NOT** (round-2 finding): the reaction/crossing descriptors (reference `EntityId`); `MeshDescriptor` (references `AnimationState`/`InterruptPolicy` in `components/mesh.rs`); `EntityTypeDescriptor` (aggregates `BillboardEmitterComponent` at `entity.rs:216` + the mesh state enums). Those go to `postretro-entities`. Plus the *pure numeric/IR* validators only (`DescriptorError`; `validate_*`/`validate_dash_expr`/`ir_node_from_json`/`ir_type_label` carved from `validate.rs` — `build_crossing` → entities, and the `mlua`/`render::ui` validators stay runtime-side).
- POD value types: `Vec3Lit`/`EulerDegrees` (relocated out of `conv.rs`). `Vec3Lit` is stored by value in `light`/`billboard_emitter`; `EulerDegrees` is FFI-boundary-only (not stored in any component).
- Sunk subsystem PODs: `DamagePayload`, `NavAgentParams`, `ModMapEntry`. (`MenuCamera`/`Frontend` are not consumed by the floor and stay in `postretro`.)

### Upper: `postretro-entities`
VM-free; depends on `postretro-foundation`.
- `registry.rs` (`EntityId`, `ComponentKind`, `ComponentValue`, `EntityRegistry`, `RegistryError`, `Transform`, `FogVolumeComponent`).
- Remaining `components/*` data structs (light, billboard_emitter, mesh, health, agent, brain, particle, sprite_visual, weapon, fog_volume) and their behavior fns that take `&EntityRegistry` (`apply_damage`, `attach_agent`, mesh/brain fns).
- The **reaction/crossing descriptor types** (`SequenceStep`/`NamedReaction`/`ReactionDescriptor`/`PrimitiveDescriptor`/`ProgressDescriptor`/`CrossingCondition`/`CrossingDescriptor`) + the `build_crossing` validator — they embed/construct `EntityId`, so they sit with the registry.
- **`MeshDescriptor`** (references `AnimationState`/`InterruptPolicy`) and **`EntityTypeDescriptor`** (`entity.rs:216` embeds `BillboardEmitterComponent`; also references the mesh state enums) — round-2 finding: `EntityId`-free but they reference entities-resident types, so they belong here. `AnimationState`/`InterruptPolicy`/`DEFAULT_CROSSFADE_MS` stay in `components/mesh.rs` (entities); `MeshDescriptor` joins them.
- `ctx.rs` (`ScriptCtx`), `slot_table.rs` + `engine_state_catalog.rs`, `provenance.rs`, `scripting/error.rs` (`ScriptError` — distinct from `data_descriptors/error.rs` `DescriptorError`, which descends), `reactions/system_commands.rs` (`SystemCommandQueue` + command enum), and `DataRegistry` (already slim — it stores only POD descriptors + `ModMapEntry` and destructures `ui_trees` away at `data_registry.rs:88`; `LevelManifest` itself stays runtime-side and the population destructuring moves there).

### Stays in the VM-coupled runtime crate (`postretro` today; later `scripting-core`)
`ir/scopes.rs` (`StoreScope`); `conv.rs` FFI impls + the json bridges; `data_descriptors/{js,lua}/` converters + `mod.rs` VM/`render::ui` glob; `RegisteredUiTree`/`LevelManifest` (UI-embedding); `runtime/*`; `luau.rs`/`quickjs.rs`/`primitives/*`/`primitives_registry.rs`/`reaction_dispatch.rs`/typedef generator.

## Orphan-rule contract, per crate

Each floor crate owns the FFI marshalling impls (`FromJs`/`IntoJs`/`FromLua`/`IntoLua`) for the types it defines, behind an optional `script-ffi` feature that pulls `rquickjs`/`mlua`. Only the runtime crate enables the feature. `postretro-foundation`: FFI for `Vec3Lit`/`EulerDegrees` + the `EntityId`-free descriptor types. `postretro-entities`: FFI for `EntityId`/`Transform`/`ComponentKind`/`ComponentValue` + component types + reaction/crossing descriptors. `postretro-entities` depends on `postretro-foundation` with `default-features = false` and forwards its own `script-ffi` to `postretro-foundation/script-ffi` (never enabling it unconditionally), so a default `cargo tree` of `postretro-entities` stays VM-free. Default builds of both crates have no VM deps — that is the firewall. Precedent: `crates/level-format`'s optional `serde`/`gltf-resolve` features.

## Inversion call-site map (consumers flip to the floor — all cycle-free)

- **movement** (`mod/dispatch/intents/substrate/carry.rs`): become floor-consumers; `scope.rs` moves into `postretro-foundation`. No remaining movement file is pulled back into the floor. Imports of `MovementScope`/`PlayerMovementComponent`/movement descriptors re-point to the floor crates.
- **nav** (`mod.rs`/`path.rs`): pure consumer once `NavAgentParams` sinks. nav imports no components.
- **weapon** (`damage/impact/mod.rs`): pure consumer once `DamagePayload` sinks; consumes `WeaponDescriptor`/`FireMode`/`ResolutionMode` from the floor.
- **ai** (`scripting/systems/ai.rs`): pure consumer; imports `AiDescriptor`/registry/components/`DamagePayload` from the floor.
- **opaque-handle consumers** — `render` (only `#[cfg(test)]` refs), `audio` (none), `netcode` (`EntityId`/`EntityTypeDescriptor`/`NavAgentParams` as data), `agent_steering` (handles), `collision` (none): all depend up cleanly. None defines a type the floor needs back.

## Naming

Names decided: `postretro-foundation` (lower) / `postretro-entities` (upper). "Foundation" (not "core") names the base layer in the Apple-Foundation / Unreal-`Core` sense without competing with "the engine is the core"; "entities" names the closed entity/component model without implying an extensible ECS (a non-goal). The lower crate is wider than "movement" or "IR" alone (it's the leaf-data + evaluation substrate); the upper is the entity/scripting data model.
