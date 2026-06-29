# Scripting-Core Extraction + Primitive-Handler Relocation

> **Status:** draft
> **Related:** `context/plans/ready/engine-data-floor/` (precondition — the floor this builds on) · `context/lib/scripting.md` §12 · `context/lib/build_pipeline.md` · `context/lib/development_guide.md` · `context/plans/drafts/compile-time-reduction/` · sibling `research.md`

## Goal

Extract the VM-coupled scripting runtime — the `rquickjs`/`mlua` bindings, the marshalling orchestration, the IR store scope, and the typedef generator — out of the `postretro` binary into a `scripting-core` crate stacked on the `engine-data-floor` crates, so routine engine edits stop recompiling the VM bindings. Then co-locate script-callable primitive handlers with the subsystems they expose. Cuts incremental build time now and reduces agent fan-out conflict ahead of the script-led Epic 16 (Combat). No runtime behavior, wire-format, or scripting-semantics change.

## Precondition

This spec **depends on `engine-data-floor` having shipped.** That plan extracts the two VM-free crates this one stacks on:

- **`postretro-foundation`** (lower) — IR evaluator core, the movement/IR cluster, foundation-clean descriptor & value PODs, sunk subsystem PODs.
- **`postretro-entities`** (upper) — entity registry, `ComponentValue`, components, `ScriptCtx`, the registries, `DataRegistry`, the entities-bound descriptors.

Both crates already carry their own types' FFI marshalling impls behind an optional `script-ffi` feature (off by default; `postretro-entities/script-ffi` forwards to `postretro-foundation/script-ffi`). The floor's earlier draft had this spec extract a single `entity-core` crate; that is **superseded** — the floor replaces it. This spec turns the floor's `script-ffi` features on and pulls the VM-coupled remainder that still lives in `postretro` after the floor lands.

## Scope

### In scope

- Extract `postretro-scripting-core`: the VM-coupled runtime that still lives in `postretro` after the floor lands — the primitive registry, the two VM subsystems, the marshalling-orchestration bridges, the IR store scope, the VM/UI converters, and the typedef generator. It is the crate that turns the floor's `script-ffi` features on.
- Introduce a `ScriptingCore` sub-struct on `Session` grouping the scripting-tranche fields.
- Relocate per-primitive cross-runtime tests to `scripting-core` (or an integration crate); keep pure-logic handler tests with the handler.
- Relocate script-callable primitive handlers (`scripting/primitives/`, `scripting/reactions/`) to co-locate with the subsystem each exposes (into subsystem module trees within `postretro`, **not** new crates), registering against `scripting-core`. **The `primitives/*` handlers are not fully runtime-agnostic** — `entity.rs` (`NullableString`), `world.rs` (`WorldQueryFilterInput`), `light.rs`, `store.rs`, and `mod.rs` carry handler-local marshalling newtypes with `FromJs`/`IntoJs`/`FromLua`/`IntoLua` impls; only the closure *bodies* are VM-free (verified: the newtypes appear in closure signatures, not bodies — bodies delegate to VM-free free functions). So `primitives/*` relocation splits: the pure handler logic moves to the subsystem; the marshalling newtypes, their FFI impls, and the `register_*` wiring that references them stay in `scripting-core`. The `reactions/*` handlers are genuinely agnostic (zero `rquickjs`/`mlua` refs in **non-test** code) and relocate whole.
- Quote before/after `--timings` from the compile-time-reduction Task 1 baseline on the extraction.

### Out of scope

- The floor crates themselves (`postretro-foundation`, `postretro-entities`) and the entity/value-type FFI impls they own — shipped by `engine-data-floor`. Do not duplicate the floor's marshalling impls here; `scripting-core` only enables their `script-ffi` feature.
- Crate-ifying the bridges (`scripting/systems/`) — they stay in `postretro` (render/FX-coupled glue).
- Crate-ifying any subsystem that receives relocated handler logic (render/audio/lighting/movement, plus health/particles/mesh/command-queue per the Task 5 mapping) — handler "co-location" means same module tree in `postretro` depending on the new crates; the handler-side firewall fully lands when those subsystems crate-ify later.
- `inventory`/`linkme` distributed registration — keep explicit `register_all` → `register_*` aggregation from `Session::build`.
- Any change to primitive names, wire shapes, marshalling results, SDK typedef output, or PRL format.
- PRL-loader / visibility extraction (separate compile-time-reduction tasks).

## Acceptance criteria

- [ ] `postretro-scripting-core` exists as a workspace crate; `cargo build --workspace` and `cargo test --workspace` pass.
- [ ] `scripting-core` depends on both floor crates with `script-ffi` enabled — `postretro-entities/script-ffi` (which forwards to `postretro-foundation/script-ffi`). `cargo tree -p postretro-scripting-core` shows `rquickjs` and `mlua`; the floor crates remain VM-free in a default `cargo tree` of themselves (that AC is owned by `engine-data-floor`).
- [ ] The bridges and the relocated **reaction** handlers (plus the relocated pure-logic of the primitive handlers) depend on the floor crates **without** `script-ffi`. The primitive-handler marshalling newtypes that need the VMs stay in `scripting-core`; only `scripting-core` enables `script-ffi` on the floor.
- [ ] Touching the relocated **pure-logic** of a primitive handler (the subsystem-side fn, not the `scripting-core` marshalling wiring), then rebuilding, does **not** recompile `rquickjs`, `mlua`, `glyphon`, `wgpu`, `kira`, or `winit`. (Observation gate via `--timings`, not a runnable test.)
- [ ] The SDK typedef drift test (`cargo test`, §SDK Type Definitions) passes unchanged — generated `postretro.d.ts` / `postretro.d.luau` are byte-identical to pre-refactor (this guards `register_all`'s registration/iteration order).
- [ ] All primitives install and behave identically in both QuickJS and Luau (existing cross-runtime tests pass from their new home — `scripting-core/tests/`).
- [ ] Relocated fog handlers preserve the cross-boundary name mappings — `edgeSoftness`→`edge_softness` (Rust field), `falloff`→`radial_falloff` (wire/WGSL) — unchanged from pre-refactor. Oracle: the typedef drift test covers the script/wire half (`edgeSoftness`); the `edge_softness` Rust field lives in `set_fog_params.rs` (relocates with the fog handler), while the `falloff`→`radial_falloff` wire translation lives in `scripting/systems/fog_volume_bridge.rs` — a bridge that is **out of scope to move**, so that half is asserted against the un-relocated bridge.
- [ ] `Session` exposes the scripting tranche through a `ScriptingCore` sub-struct; `Session::build` remains the sole construction site and still triggers all `ScriptCtx` clone distribution there (the build-site clones plus the per-closure clones inside `register_*`). All `Session` reads of scripting-tranche fields route through `ScriptingCore` — compiler-enforced once the fields move into the sub-struct.
- [ ] No `unsafe` added. No `wgpu` call moves out of the renderer.
- [ ] The extraction PR quotes before/after timings for the targeted edit loops from the baseline commands.

## Tasks

### Task 1: Baseline dependency (gate)

Consume the compile-time-reduction Task 1 baseline (or the `engine-data-floor` baseline, if its post-floor numbers are committed). If unavailable, capture the minimum needed here with these exact commands, recording wall-clock for each so later tasks have a concrete referent:

1. Warm full check: `cargo check -p postretro` (run twice; record the second, warm time).
2. Primitive-handler touch rebuild: `touch crates/postretro/src/scripting/primitives/light.rs && cargo check -p postretro`.
3. Critical-path profile: `cargo build -p postretro --timings`, then note where `rquickjs-sys`, `mlua-sys`, and `luau0-src` sit on the critical path in the generated `cargo-timing.html`.

All later tasks quote before/after against these three commands.

### Task 2: Extract `postretro-scripting-core`

After the floor lands, the VM-coupled remainder still living in `postretro` is exactly what becomes the `scripting-core` crate. Per `engine-data-floor`, that remainder is:

- `ir/scopes.rs` (`StoreScope`)
- `conv.rs` — the json-orchestration bridges (`json_to_js`/`js_to_json`/`json_to_lua`/`lua_to_json`)
- `data_descriptors/{js,lua}/` converters + the `mod.rs` VM/`render::ui` glob
- `RegisteredUiTree`/`LevelManifest`
- the `validate.rs` `mlua` + `render::ui` validators (`validate_dense_lua_array`, `parse_*`)
- `data_descriptors/error.rs` `js_err`/`lua_err`
- `runtime/*`
- `luau.rs`, `quickjs.rs`, `primitives_registry.rs`, `reaction_dispatch.rs`, and the typedef generator

Move this cluster into `postretro-scripting-core`. Depend on both floor crates with `script-ffi` enabled — `postretro-entities = { workspace = true, features = ["script-ffi"] }`, which forwards to `postretro-foundation/script-ffi`. This crate is the one that turns the floor's `script-ffi` on; the entity/value-type FFI **impls already live in the floor crates** under `script-ffi` (do not duplicate them). `scripting-core` owns the json-orchestration bridges and the VM subsystems only.

Default deps beyond the floor crates: `rquickjs`, `mlua`, `serde`, `serde_json`. Keep the sealed `RegisterablePrimitive` trait + `impl_registerable!` macro and the installer type aliases here. Note: `luau.rs` carries pre-existing `unsafe` that travels into `scripting-core` (as does any in `watcher.rs`/the `scripting` mod root that lands here). The IR-core `unsafe` (`ir/mod.rs`, `ir/alloc_probe.rs`) does **not** come here — it descends to `postretro-foundation` per `engine-data-floor` (`ir/scopes.rs`, which stays here, has no `unsafe`). The "No `unsafe` added" AC is a net-new delta gate — do not add `#![forbid(unsafe_code)]` or treat the inherited `unsafe` as a violation.

### Task 3: `ScriptingCore` sub-struct on `Session`

`Session` stays in the `postretro` binary. Group the scripting-tranche fields (script runtime, `ScriptCtx`, the registries, the `ScriptCtx`-clone consumers) into a `ScriptingCore` struct owned by `Session` — the sub-struct groups handles into `scripting-core`/floor types. `Session::build` constructs it at the existing single site and distributes clones there; field access updates to go through the sub-struct. Behavior-preserving.

### Task 4: Relocate cross-runtime tests

Move the per-primitive tests that instantiate `rquickjs::Runtime` + `mlua::Lua` into `scripting-core/tests/` (a standalone `scripting-integration` crate only as the fallback per Decisions). Leave pure-logic handler tests (e.g. `apply_light_animation_inner`-style) with their handler so subsystem `cargo test` needs no VM. Note: some `reactions/*` files carry a cross-runtime parity test in a `#[cfg(test)]` block (e.g. `set_fog_params.rs`) — relocating those handlers whole means their VM-touching test module moves to `scripting-core/tests/`, while the handler's pure code lands in its subsystem.

### Task 5: Relocate primitive + reaction handlers

Relocate each handler to co-locate with the subsystem it exposes. **There is no single `register_all` reaction surface** — handlers register through **four distinct registry types**, all constructed at the single `Session::build` site (`crates/postretro/src/session/mod.rs`). A handler relocates *with all of its registration sites*. The marshalling newtypes + `register_*` wiring stay in `scripting-core` (the VM crate); the pure handler logic relocates to subsystem modules in `postretro`.

| Registry | Registrar(s) (current location) | Takes `ScriptCtx`? | Handler shape |
|---|---|---|---|
| `PrimitiveRegistry` | `register_all` → `register_*` per domain (`primitives/mod.rs`) | yes | closure capturing `ScriptCtx` |
| `SequencedPrimitiveRegistry` | `register_sequenced_light_primitives`, `register_sequenced_fog_primitives` (`reactions/registry.rs`) | yes | `(id, args)` closure capturing `ScriptCtx` |
| `ReactionPrimitiveRegistry` | `register_emitter_reaction_primitives`, `register_fog_reaction_primitives` (`reactions/registry.rs`) | **no** | `(reg, targets, args)` closure → `dispatch(reg, targets, &parsed)` |
| `SystemReactionRegistry` | `register_system_reaction_primitives` (`reactions/system_commands.rs`) | **no** | enqueues onto `SystemCommandQueue` |

All four registrars stay in `scripting-core`. `SystemCommandQueue` itself lives in `postretro-entities` (the floor); the **drain** runs through `reaction_dispatch.rs` (in `scripting-core`) via `ScriptCtx::system_commands`. Several families register in **more than one** registry — **fog** registers in both `ReactionPrimitiveRegistry` and `SequencedPrimitiveRegistry`; **light** registers in both `PrimitiveRegistry` (via `register_all`) and `SequencedPrimitiveRegistry`. Move every site for a family together.

Relocation mapping (inlined — the orchestrate contract hides `research.md`):

| Family | Handlers | Registry(ies) | Destination |
|---|---|---|---|
| light | `setLightAnimation` + sequenced-light steps | `PrimitiveRegistry`, `SequencedPrimitiveRegistry` | lighting |
| entity/world | `entityExists`, map-kvp getter, `worldQuery`, `worldGetGravity`, `worldSetGravity` | `PrimitiveRegistry` | entity |
| store | `defineStore`, store read/write | `PrimitiveRegistry` | state store |
| emitter | `setEmitterRate`, `setSpinRate` | `ReactionPrimitiveRegistry` | particles |
| mesh | `setAnimationState` | `ReactionPrimitiveRegistry` | mesh |
| health | `applyDamage` | `ReactionPrimitiveRegistry` | health |
| fog | `setFogDensity`, `setFogGlow`, `setFogEdgeSoftness`, `setFogFalloff`, `setFogParams`, `setFogAnimation` | `ReactionPrimitiveRegistry` **+** `SequencedPrimitiveRegistry` | fog |
| system | `playSound`, `rumble`, `flashScreen`, etc. | `SystemReactionRegistry` | command-queue glue |

**Two relocation shapes (per A1, scope):**

- **`reactions/*` (agnostic, zero non-test VM refs):** relocate the whole handler — its `Args` type, `dispatch` fn, and the registrar closure — to its subsystem module. The `dispatch(reg, targets, &parsed)` signature already takes native args (no VM types). `system_commands` handlers enqueue onto `SystemCommandQueue` (in `postretro-entities`, from the floor); the **drain** runs through `reaction_dispatch.rs` via `ScriptCtx::system_commands` and stays in `scripting-core` (not the `scripting/systems/` bridges). Confirm zero `rquickjs`/`mlua` import in moved non-test code; any VM-touching `#[cfg(test)]` module goes to `scripting-core/tests/` (Task 4).
- **`primitives/*` (carry marshalling newtypes):** split. The pure handler logic moves to its subsystem module; the marshalling newtypes (`NullableString`, `WorldQueryFilterInput`, etc.), their `FromJs`/`IntoJs`/`FromLua`/`IntoLua` impls, and the `register_*` wiring that references them **stay in `scripting-core`**. The subsystem-side logic must not import `rquickjs`/`mlua`; the `scripting-core`-side wiring may. Confirm the subsystem module is VM-free after the split.

  **The seam:** the `scripting-core` `register_*` closure does the marshalling (decode script args via the newtypes, encode the result), then calls the subsystem-side pure-logic fn with native Rust args plus the `ScriptCtx`/cluster handle. The subsystem fn signature takes native types only — never `rquickjs`/`mlua` types. This already matches source: closure bodies today are one-line delegations to free fns (`apply_light_animation`, `read_store_slot`, `parse_query_filter` + collectors), so the split relocates those free fns and leaves the newtype-unpacking in the closure. For `worldQuery` specifically, `parse_query_filter` and the collectors relocate; the `WorldQueryFilterInput`→native unpacking and the `JsonValue` result wrap stay wiring-side.

`Session::build` remains the sole aggregation site: all four registries are constructed there and the relocated registrar fns are still *called* from there. Registration order within each registry must be preserved across the move — for `PrimitiveRegistry`, order is an invariant since the typedef generator's byte-identical output (AC) may depend on registry iteration order.

## Sequencing

**Precondition:** `engine-data-floor` has shipped — `postretro-foundation` and `postretro-entities` exist with their `script-ffi` features in place. This spec does not start until then.

**Phase 1 (sequential):** Task 1 — baseline gates all timing claims.
**Phase 2 (sequential):** Task 2 — `scripting-core` extraction pulls the VM-coupled remainder onto the floor and enables `script-ffi`. The crate must exist before the sub-struct and test/handler relocations can target it.
**Phase 3 (concurrent):** Task 3, Task 4 — independent once the crate exists (Session grouping vs. test relocation). Task 4's relocated tests instantiate raw `rquickjs::Runtime` + `mlua::Lua` (not `Session`), so they don't touch Task 3's `ScriptingCore` restructuring.
**Phase 4 (sequential, after Task 3):** Task 5 — per-handler relocations fan out across handler families (each is an independent unit), but the phase as a whole runs after Task 3 because both Task 3 and Task 5 edit `Session::build`'s registration block (`register_all` call site + clone distribution). Within the phase, handler families parallelize. Task 4's relocated cross-runtime tests (already in `scripting-core/tests/` from Phase 3) exercise primitives through the registry/runtime public API, not handler file paths, so the Task 5 handler split doesn't invalidate them; Task 5 fixes any test import path it does touch.

Structural surgery (the extraction in Phase 2) is serialized deliberately; the handler fan-out is Phase 4, gated behind Task 3's `Session::build` restructuring. The shape is "serialize structural surgery, fan out handler relocation."

## Rough sketch

**Crate layout.**
- Floor (from `engine-data-floor`, precondition): `crates/foundation/` (`postretro-foundation`) and `crates/entities/` (`postretro-entities`). Both VM-free by default; each owns its types' FFI impls under `script-ffi`.
- `crates/scripting-core/` (`postretro-scripting-core`): the VM-coupled runtime. Depends on `postretro-entities = { workspace = true, features = ["script-ffi"] }` (which forwards to `postretro-foundation/script-ffi`), plus `rquickjs`, `mlua`. This is the crate that turns `script-ffi` on.

**The firewall.** Cargo feature unification compiles the floor crates once with `script-ffi` on in the full build, but `rquickjs`/`mlua` remain upstream deps of the floor crates — editing a handler or bridge in `postretro` never rebuilds them, and editing a floor crate's pure code never rebuilds the VMs (the floor's own AC). `scripting-core` is the single point where the VMs enter the graph for the runtime.

**Aggregation unchanged.** `register_all` (primitives/mod.rs, in `scripting-core`) stays the explicit primitive entry; its sibling registrars for the other three registries (`register_emitter_reaction_primitives`/`register_fog_reaction_primitives`, `register_system_reaction_primitives`, `register_sequenced_light_primitives`/`register_sequenced_fog_primitives`) stay explicit too (no `inventory`/`linkme`). `Session::build` (in `postretro`) stays the single runtime construction site for all four registries. The "one indivisible group" stays indivisible at runtime (shared `Rc`) and becomes the `ScriptingCore` type at compile time.

**Naming.** Working name `postretro-scripting-core`; final name decided at implementation.

## Decisions (resolved during review)

- **The floor is a precondition, not a task.** `engine-data-floor` ships `postretro-foundation` + `postretro-entities` (and their `script-ffi` impls). The earlier single-`entity-core` extraction is superseded; this spec stacks `scripting-core` on top.
- **Cross-runtime tests land in `scripting-core/tests/`.** A standalone `scripting-integration` crate is the fallback only if a test drags handler-specific fixtures that don't belong in `scripting-core`.
- **A1 (primitive-handler marshalling):** the `primitives/*` marshalling newtypes and their `register_*` wiring stay in `scripting-core`; only the pure handler logic relocates to subsystems (see Scope, Task 5).
- **Phase 4 stays in this spec — not split out.** Task 5's handler relocation is ~8 independent, low-conflict families across subsystems — the shape `/orchestrate` fans out well, so handler count is not a reason to split. Keeping it unified maximizes progress in one orchestration session and avoids re-opening `Session::build` in a later spec (Task 3 and Task 5 both touch its registration block; doing them in one spec touches the file once). The firewall (Phases 1–3) and the co-location (Phase 4) land together. The Phase 1–3 boundary remains independently shippable if implementation later forces a stop, but that is a fallback, not the plan.
