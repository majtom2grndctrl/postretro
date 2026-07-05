# Research — scripting-core extraction

Source-grounded findings (file:line confirmed). Feeds `index.md`; not part of the spec contract.

## Rebased onto `engine-data-floor`

This spec now stacks on the `engine-data-floor` crates. Findings here that grounded a single `entity-core` extraction are **superseded** by that plan; the floor's `research.md` is the authoritative dependency map for the VM-free data model. What remains valid and load-bearing for *this* spec is the runtime-remainder membership (what `scripting-core` is), the four-registry registration reality, the A1 source-verification of the primitive handlers, and the orphan-rule precedent — re-pointed at the floor crates below.

## Superseded by the floor

- **The single-`entity-core` module table and the "components are a VM-free leaf" assumption.** `engine-data-floor`'s research shows the VM-free floor is larger and layered: `ComponentValue` embeds every component by value, components couple to `movement`/`nav`/`weapon`/`ai` and the IR substrate, and the floor splits into `postretro-foundation` (lower) + `postretro-entities` (upper). The destinations once labeled `entity-core` (`registry.rs`, `ctx.rs`, `components/`, `data_registry.rs`, `slot_table.rs`, `SystemCommandQueue`, `provenance.rs`) now resolve from the floor crates — see `engine-data-floor/research.md` §"Layering verdict and membership".
- **The orphan-rule resolution as *this* spec's work.** The floor owns the per-crate `script-ffi` FFI impls (`EntityId`/`Transform`/`ComponentKind`/`ComponentValue`/components in `postretro-entities`; `Vec3Lit`/`EulerDegrees` + foundation-clean descriptors in `postretro-foundation`). `scripting-core` does not write these impls; it only enables `postretro-entities/script-ffi` (which forwards to `postretro-foundation/script-ffi`). The `conv.rs` split between FFI impls (down to the floor) and json-orchestration bridges (stay up in `scripting-core`) is executed by the floor plan; this spec inherits it.

## `scripting-core` membership (the runtime remainder)

After the floor lands, the VM-coupled remainder still living in `postretro` is what becomes `scripting-core`. Per `engine-data-floor/index.md` Rough sketch ("What stays in `postretro`") and its `research.md` §"Stays in the VM-coupled runtime crate":

- `ir/scopes.rs` (`StoreScope` — pulls `ScriptCtx`/`slot_table`/`primitives::store`).
- `conv.rs` json-orchestration bridges (`json_to_js`/`js_to_json`/`json_to_lua`/`lua_to_json`); the FFI impls themselves descend to the floor.
- `data_descriptors/{js,lua}/` converters + `mod.rs` VM/`render::ui` glob.
- `RegisteredUiTree`/`LevelManifest` (UI-embedding manifest types; in `data_descriptors/runtime_manifest.rs`).
- `data_descriptors/validate/runtime.rs` `mlua`/`render::ui` validators (`validate_dense_lua_array`, `parse_*`); the pure numeric/IR validators (`validate/foundation.rs`) and `build_crossing` descended to the floor and do not move.
- `data_descriptors/vm_adapters.rs` `js_err`/`lua_err` (VM adapters); `DescriptorError` descended to `postretro-foundation`, and `data_descriptors/error.rs` is now a thin barrel re-exporting it (so `error.rs` does not move).
- `runtime/*`.
- `luau.rs`, `quickjs.rs`, `primitives_registry.rs`, `reaction_dispatch.rs`, the typedef generator.
- `primitives/*` handlers — the A1 relocation target (see below).

`primitives_registry.rs` carries `rquickjs`/`mlua` only in the installer **type aliases** (`QuickJsInstaller`/`LuauInstaller`); construction/registration is runtime-agnostic. `luau.rs` carries pre-existing `unsafe` that travels into `scripting-core`; the IR-core `unsafe` (`ir/mod.rs`, `ir/alloc_probe.rs`) descends to `postretro-foundation` (not here), and `ir/scopes.rs` (which stays here) has none.

## Handler → subsystem mapping (Task 5)

Primitives: `entity.rs` → entity, `light.rs` → lighting, `world.rs` → world/entity (worldQuery, worldGetGravity, worldSetGravity), `store.rs` → state store, `mod.rs` → `register_all` entry + shared types.

**`primitives/*` files are NOT runtime-agnostic (codebase-anchor verified).** They import `rquickjs` + `mlua` at module scope for handler-local marshalling newtypes with FFI impls: `entity.rs` (4 refs; `NullableString` `IntoJs`/`IntoLua`), `light.rs` (25 refs, all inside `#[cfg(test)]` — production code is VM-free), `world.rs` (18 refs; `WorldQueryFilterInput` `FromJs`/`FromLua` — note: `WorldQueryFilter` is the *SDK typedef* name, a different symbol), `store.rs` (17 refs), `mod.rs` (9 refs). The newtypes appear in closure *signatures*, not bodies — bodies delegate to VM-free free functions (`apply_light_animation`, `read_store_slot`, `parse_query_filter` + collectors), so the A1 split is clean. By contrast every `reactions/*` handler has **0** `rquickjs`/`mlua` refs in **non-test** code (`set_fog_params.rs` has VM refs only inside its `#[cfg(test)]` cross-runtime parity test). This drives the A1 split in `index.md` Task 5: for `primitives/*`, the pure logic **and** the `register_*` wiring both relocate to the `postretro` subsystem, while only the marshalling newtypes + their FFI impls stay in `scripting-core` (the wiring is in `postretro` and references them as a legal down-edge — `scripting-core` sits below `postretro` and may not name a subsystem fn, so the wiring that calls one cannot live below); `reactions/*` production code relocates whole, with VM-touching `#[cfg(test)]` modules going to `scripting-core/tests/`.

Reactions: `apply_damage.rs` → health; `set_animation_state.rs` → mesh; `set_emitter_rate.rs`/`set_spin_rate.rs` → emitter; `set_fog_*` (density, glow, edge_softness, falloff, params, animation) → fog volume; `system_commands.rs` → cross-engine command queue; `registry.rs` → dispatch root; `log_capture.rs` → test-only.

## Registration reality (four registries, one aggregation site)

Codebase-anchor confirmed in `session/mod.rs` + `reactions/registry.rs`: there is no single `register_all` reaction surface. Four registry types are built in `Session::build`:

- `PrimitiveRegistry` (via `register_all`, takes `ScriptCtx`).
- `SequencedPrimitiveRegistry` (via `register_sequenced_light_primitives` + `register_sequenced_fog_primitives`, take `ScriptCtx`).
- `ReactionPrimitiveRegistry` (via `register_emitter_reaction_primitives` + `register_fog_reaction_primitives`, **no** `ScriptCtx`; handlers are `dispatch(reg, targets, &parsed)`).
- `SystemReactionRegistry` (via `register_system_reaction_primitives`, **no** `ScriptCtx`; enqueues onto `SystemCommandQueue`).

All four registrar **functions** relocate to `postretro`, co-located with the handler logic they wire; `scripting-core` retains only the registry **types** + machinery they populate (see the A1 split above and `index.md` Task 5). Fog and light each register in two registries — every site for a family relocates together. `SystemCommandQueue` lives in `postretro-entities` (the floor); the system-command **drain** runs through `reaction_dispatch.rs` (`ScriptCtx::system_commands`), which stays in `scripting-core`.

## Aggregation site

`Session` stays in the `postretro` binary; `Session::build` is the sole runtime construction site for all four registries: `ScriptCtx::new()` → `register_all(&mut script_registry, script_ctx.clone())` → `ScriptRuntime::new(...)`. The build site makes several `script_ctx.clone()` calls; the bulk of clones are captured per-primitive inside the `register_*` closures (each does `let ctx = ctx.clone()`) — that is what "distributes" means; there is no single ~20-clone site. Explicit aggregation, single site (no `inventory`/`linkme`). The `ScriptRuntime` struct and `ScriptRuntime::new` live under `scripting/runtime/` and move to `scripting-core` with `runtime/*`.

## Orphan-rule precedent (re-pointed at the floor)

The orphan rule (`impl ForeignTrait for LocalType` is legal only where the type is local) is handled by the floor: each floor crate owns its types' FFI impls behind an optional `script-ffi` feature. `Vec3Lit`/`EulerDegrees` are the glam-wrapping newtype precedent (now in `postretro-foundation`). `crates/level-format`'s optional `serde`/`gltf-resolve` features are the Cargo template. `scripting-core` writes no FFI impls for floor-owned types; it enables `postretro-entities/script-ffi`, which forwards to `postretro-foundation/script-ffi`.

## Workspace dep entries confirmed (root Cargo.toml)

`glam` (serde+bytemuck), `serde` (derive+rc), `serde_json`, `thiserror`, `anyhow`, `rquickjs` (0.11), `mlua` (0.11, luau+serde) — all available as `workspace = true`. `level-format`'s `Cargo.toml` is the optional-feature template (`serde = ["dep:serde"]`).
