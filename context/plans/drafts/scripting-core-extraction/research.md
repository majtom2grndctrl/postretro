# Research — scripting-core extraction

Source-grounded findings (file:line confirmed). Feeds `index.md`; not part of the spec contract.

## Module sizes and runtime-dependency status

| File | Lines | rquickjs/mlua in non-test code? | Destination |
|---|---|---|---|
| `scripting/registry.rs` | 1462 | No (glam + serde + thiserror) | `entity-core` |
| `scripting/ctx.rs` | 149 | No (Rc/RefCell of registry, data_registry, slot_table, system_commands) | `entity-core` |
| `scripting/conv.rs` | 1255 | **Yes** — `mlua` + `rquickjs` imports; marshalling layer | split: FFI impls feature-gated into `entity-core`; orchestration into `scripting-core` |
| `scripting/primitives_registry.rs` | 832 | Only in the installer **type aliases** (`QuickJsInstaller`/`LuauInstaller`); construction/registration is runtime-agnostic | `scripting-core` |
| `scripting/luau.rs` | 1683 | Yes (runtime subsystem) | `scripting-core` |
| `scripting/quickjs.rs` | 774 | Yes (runtime subsystem) | `scripting-core` |
| `scripting/ir/` (`mod.rs` 416 + bind/eval/load/scope/scopes) | ~ | No in core; tests only | `scripting-core` |
| `scripting/typedef/` (`mod.rs` 205 + common/ts/luau) | ~ | No | `scripting-core` |
| `scripting/primitives/mod.rs` | 799 | — | stays (handlers); near 800-line watch |

No file currently exceeds the ~800-line split-before-extend gate (primitives/mod.rs sits at 799).

## The orphan-rule constraint (central design problem)

`conv.rs` implements foreign FFI traits on the entity types:

- `FromJs`/`IntoJs`/`FromLua`/`IntoLua` for `EntityId` (conv.rs:132–156)
- `FromJs`/`IntoJs`/`FromLua`/`IntoLua` for `Transform` (220–283)
- `FromJs`/`IntoJs`/`FromLua`/`IntoLua` for `ComponentKind` (322–349)
- `FromJs`/`IntoJs`/`FromLua`/`IntoLua` for `ComponentValue` (351–753)
- `FromJs`/`FromLua`/`IntoJs`/`IntoLua` for `LightComponent`, `LightAnimation` (986–1036)

These compile today because the types are local to `postretro`. After extraction, `impl rquickjs::FromJs for entity_core::EntityId` written in `scripting-core` is an orphan violation (both trait and type foreign).

**Resolution (precedent: `crates/level-format` optional features).** The FFI marshalling impls for entity-core types live **in `entity-core`, behind an optional `script-ffi` feature** that pulls `rquickjs` + `mlua`. The impl is then legal — the type is local to the crate. `entity-core` default build has no VM deps; bridges depend on default (`EntityId` without VMs); `scripting-core` depends on `entity-core` with `script-ffi` on. Cargo feature unification means the full `postretro` build compiles `entity-core` once with the feature, but editing a bridge/handler in `postretro` never recompiles `rquickjs`/`mlua` (they are upstream deps of `entity-core`). Existing newtype precedent for the orphan rule already in conv.rs: `Vec3Lit`, `EulerDegrees` wrap glam types to impl FFI traits.

The component-type marshalling (`LightComponent`, `LightAnimation`) moves with the component definitions; same feature-gate mechanism.

## ScriptCtx dependency cluster

`ScriptCtx` (ctx.rs:23–55) holds `Rc<RefCell<_>>` of: `EntityRegistry`, `DataRegistry`, `SlotTable`, `Cell<u64>` frame, `Cell<f32>` gravity, `SystemCommandQueue`. None pull VMs. So `entity-core` must also absorb (or re-export) `DataRegistry`, `SlotTable`, `SystemCommandQueue`, and the `components/` structs that `ComponentValue` wraps. `entity-core` is therefore the scripting **data model**, not just the entity registry. Bridges capture `ScriptCtx` (e.g. `FlashDecay::new(script_ctx.clone())` in session/mod.rs), so `ScriptCtx` must be VM-free.

## Reverse deps of entity types

`scripting/systems/` bridges (ai, emitter_bridge, fog_volume_bridge, health, light_bridge, mesh_render, particle_render, particle_sim, ui_proxy) use only `EntityId`/`EntityRegistry`/`ComponentKind`/`ComponentValue`/`Transform`/`FogVolumeComponent`. Non-scripting modules (agent_steering, movement, netcode, render) pass `EntityId` but do **not** import from `scripting::registry` — they treat it as an opaque handle.

## scripting_systems path alias

`main.rs:57–58` (lines 55–56 are the explanatory comment): `#[path = "scripting/systems/mod.rs"] mod scripting_systems;` — rooted off `scripting/` so `gen_script_types` reuses the tree without engine/GPU deps. The bridges are already a separate module tree from `scripting` proper.

## Handler → subsystem mapping (Phase 2)

Primitives: `entity.rs` → entity, `light.rs` → lighting, `world.rs` → world/entity (worldQuery, worldGetGravity, worldSetGravity), `store.rs` → state store, `mod.rs` → `register_all` entry + shared types.

**Correction (codebase-anchor review): `primitives/*` files are NOT runtime-agnostic.** They import `rquickjs` + `mlua` at module scope for handler-local marshalling newtypes with FFI impls: `entity.rs` (4 refs; `NullableString` `IntoJs`/`IntoLua`, lines 22–38), `light.rs` (25 refs, all inside `#[cfg(test)]` — production code is VM-free), `world.rs` (18 refs; `WorldQueryFilterInput` `FromJs`/`FromLua`, lines 40–71 — note: `WorldQueryFilter` is the *SDK typedef* name, a different symbol), `store.rs` (17 refs), `mod.rs` (9 refs). The newtypes appear in closure *signatures*, not bodies — bodies delegate to VM-free free functions (`apply_light_animation`, `read_store_slot`, `parse_query_filter` + collectors), so the A1 split is clean. By contrast every `reactions/*` handler has **0** `rquickjs`/`mlua` refs in **non-test** code (`set_fog_params.rs` has VM refs only inside its `#[cfg(test)]` cross-runtime parity test, lines 638–671). This drives the A1 split in `index.md` Task 6: `primitives/*` logic relocates but its marshalling newtypes + `register_*` wiring stay in `scripting-core`; `reactions/*` production code relocates whole, with VM-touching `#[cfg(test)]` modules going to `scripting-core/tests/`.

Reactions: `apply_damage.rs` → health; `set_animation_state.rs` → mesh; `set_emitter_rate.rs`/`set_spin_rate.rs` → emitter; `set_fog_*` (density, glow, edge_softness, falloff, params, animation) → fog volume; `system_commands.rs` → cross-engine command queue; `registry.rs` → dispatch root; `log_capture.rs` → test-only.

## Aggregation site

`Session::build` (`crates/postretro/src/session/mod.rs:255`): `ScriptCtx::new()` (line 313) → `register_all(&mut script_registry, script_ctx.clone())` (call at session/mod.rs:315; `register_all` defined at primitives/mod.rs:554) → `ScriptRuntime::new(...)` (session/mod.rs:316). The build site itself makes 8 `script_ctx.clone()` calls; `ScriptRuntime::new` (runtime/core.rs:40) stores 1 more (`ctx.clone()`, line 58). The bulk of clones are captured per-primitive inside the `register_*` closures (each does `let ctx = ctx.clone()`) — that is what "distributes" means; there is no single ~20-clone site. Explicit aggregation, single site. `ScriptRuntime` struct lives at runtime/types.rs:321; `ScriptRuntime::new` at runtime/core.rs:40 — both under `scripting/runtime/`.

## Workspace dep entries confirmed (root Cargo.toml)

`glam` (serde+bytemuck), `serde` (derive+rc), `serde_json`, `thiserror`, `anyhow`, `rquickjs` (0.11), `mlua` (0.11, luau+serde) — all available as `workspace = true`. `level-format`'s `Cargo.toml` is the optional-feature template (`serde = ["dep:serde"]`).
