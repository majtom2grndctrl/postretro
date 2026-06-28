# Scripting-Core Extraction + Primitive-Handler Relocation

> **Status:** draft
> **Related:** `context/lib/scripting.md` · `context/lib/boot_sequence.md` §1 · `context/lib/build_pipeline.md` · `context/lib/development_guide.md` · `context/plans/drafts/compile-time-reduction/` (Task 6 follow-up) · sibling `research.md`

## Goal

Isolate the heavy scripting runtime dependencies (`rquickjs`, `mlua`) behind a stable crate boundary so routine engine edits stop recompiling the VM bindings, and co-locate script-callable primitive handlers with the subsystems they expose. Cuts incremental build time now and reduces agent fan-out conflict ahead of the script-led Epic 16 (Combat). No runtime behavior, wire-format, or scripting-semantics change.

## Scope

### In scope

- Extract `postretro-entity-core`: the VM-free scripting **data model** — entity registry, components, `ScriptCtx` and its cluster (`DataRegistry`, `SlotTable`, `SystemCommandQueue`), with FFI marshalling behind an optional `script-ffi` feature.
- Extract `postretro-scripting-core`: the language runtime — primitive registry, the two VM subsystems, marshalling orchestration, IR substrate, typedef generator.
- Resolve the orphan-rule constraint on `conv.rs`'s FFI trait impls (see Rough sketch).
- Introduce a `ScriptingCore` sub-struct on `Session` grouping the scripting-tranche fields.
- Relocate per-primitive cross-runtime tests to `scripting-core` (or an integration crate); keep pure-logic handler tests with the handler.
- Relocate script-callable primitive handlers (`scripting/primitives/`, `scripting/reactions/`) to co-locate with the subsystem each exposes (into subsystem module trees within `postretro`, **not** new crates), registering against `scripting-core`. **The `primitives/*` handlers are not fully runtime-agnostic** — `entity.rs` (`NullableString`), `world.rs` (`WorldQueryFilterInput`), `light.rs`, `store.rs`, and `mod.rs` carry handler-local marshalling newtypes with `FromJs`/`IntoJs`/`FromLua`/`IntoLua` impls; only the closure *bodies* are VM-free (verified: the newtypes appear in closure signatures, not bodies — bodies delegate to VM-free free functions). So `primitives/*` relocation splits: the pure handler logic moves to the subsystem; the marshalling newtypes, their FFI impls, and the `register_*` wiring that references them stay in `scripting-core`. The `reactions/*` handlers are genuinely agnostic (zero `rquickjs`/`mlua` refs in **non-test** code) and relocate whole.
- Quote before/after `--timings` from the compile-time-reduction Task 1 baseline on each extraction.

### Out of scope

- Crate-ifying the bridges (`scripting/systems/`) — they stay in `postretro` (render/FX-coupled glue).
- Crate-ifying any subsystem that receives relocated handler logic (render/audio/lighting/movement, plus health/particles/mesh/command-queue per the Task 6 mapping) — handler "co-location" means same module tree in `postretro` depending on the new crates; the handler-side firewall fully lands when those subsystems crate-ify later.
- `inventory`/`linkme` distributed registration — keep explicit `register_all` → `register_*` aggregation from `Session::build`.
- Any change to primitive names, wire shapes, marshalling results, SDK typedef output, or PRL format.
- PRL-loader / visibility extraction (separate compile-time-reduction tasks).

## Acceptance criteria

- [ ] `postretro-entity-core` and `postretro-scripting-core` exist as workspace crates; `cargo build --workspace` and `cargo test --workspace` pass.
- [ ] `cargo tree -p postretro-entity-core` run against the crate **in isolation** (default features, not the workspace-unified graph) shows **no** `rquickjs` or `mlua`. Enabling `--features script-ffi` adds both.
- [ ] The bridges and the relocated **reaction** handlers (plus the relocated pure-logic of the primitive handlers) depend on `postretro-entity-core` **without** `script-ffi`. The primitive-handler marshalling newtypes that need the VMs stay in `scripting-core`; only `scripting-core` enables `script-ffi` on `entity-core`.
- [ ] Touching the relocated **pure-logic** of a primitive handler (the subsystem-side fn, not the `scripting-core` marshalling wiring), then rebuilding, does **not** recompile `rquickjs`, `mlua`, `glyphon`, `wgpu`, `kira`, or `winit`. (Observation gate via `--timings`, not a runnable test.)
- [ ] The SDK typedef drift test (`cargo test`, §SDK Type Definitions) passes unchanged — generated `postretro.d.ts` / `postretro.d.luau` are byte-identical to pre-refactor (this guards `register_all`'s registration/iteration order).
- [ ] All primitives install and behave identically in both QuickJS and Luau (existing cross-runtime tests pass from their new home — `scripting-core/tests/`).
- [ ] Relocated fog handlers preserve the cross-boundary name mappings — `edgeSoftness`→`edge_softness` (Rust field), `falloff`→`radial_falloff` (wire/WGSL) — unchanged from pre-refactor. Oracle: the typedef drift test covers the script/wire half (`edgeSoftness`); the `edge_softness` Rust field lives in `set_fog_params.rs` (relocates with the fog handler), while the `falloff`→`radial_falloff` wire translation lives in `scripting/systems/fog_volume_bridge.rs` — a bridge that is **out of scope to move**, so that half is asserted against the un-relocated bridge.
- [ ] `Session` exposes the scripting tranche through a `ScriptingCore` sub-struct; `Session::build` remains the sole construction site and still triggers all `ScriptCtx` clone distribution there (the build-site clones plus the per-closure clones inside `register_*`). All `Session` reads of scripting-tranche fields route through `ScriptingCore` — compiler-enforced once the fields move into the sub-struct.
- [ ] No `unsafe` added. No `wgpu` call moves out of the renderer.
- [ ] Each extraction PR quotes before/after timings for the targeted edit loops from the baseline commands.

## Tasks

### Task 1: Baseline dependency (gate)

Consume the compile-time-reduction Task 1 baseline. If unavailable, capture the minimum needed here with these exact commands, recording wall-clock for each so later tasks have a concrete referent:

1. Warm full check: `cargo check -p postretro` (run twice; record the second, warm time).
2. Primitive-handler touch rebuild: `touch crates/postretro/src/scripting/primitives/light.rs && cargo check -p postretro`.
3. Critical-path profile: `cargo build -p postretro --timings`, then note where `rquickjs-sys`, `mlua-sys`, and `luau0-src` sit on the critical path in the generated `cargo-timing.html`.

All later tasks quote before/after against these three commands.

### Task 2: Extract `postretro-entity-core`

Move the VM-free data model: `scripting/registry.rs`, `scripting/ctx.rs`, `scripting/components/`, `scripting/data_registry.rs`, `scripting/slot_table.rs`, the `SystemCommandQueue` type from `scripting/reactions/system_commands.rs`, and `scripting/provenance.rs`. Default deps: `glam`, `serde`, `serde_json`, `thiserror`. Add an optional `script-ffi` feature (`dep:rquickjs`, `dep:mlua`) that compiles the FFI marshalling impls for entity-core-owned types (the `EntityId`/`Transform`/`ComponentKind`/`ComponentValue` slice of `conv.rs`). **Why they move:** after extraction, `impl rquickjs::FromJs for EntityId` written in `scripting-core` is an orphan violation (both trait and type foreign); the impl is legal only in the crate that owns the type, so the entity-type impls — plus the `Vec3Lit`/`EulerDegrees` glam-wrapping newtypes they depend on (the existing orphan-rule precedent in `conv.rs`) — move under `script-ffi`. **What stays:** `conv.rs` also holds FFI impls for `EntityTypeDescriptor`, whose type lives in `data_descriptors/` (not moved); those impls stay in `postretro`/`scripting-core` with their type — do not over-grab the whole `conv.rs` FFI block. Update all call sites; `EntityId` stays an opaque handle to non-scripting modules. Mirror `crates/level-format/Cargo.toml`'s optional-feature shape. Verify the AC's isolation invariant with `cargo tree -p postretro-entity-core` (default features — confirms no `rquickjs`/`mlua`) and `cargo tree -p postretro-entity-core --features script-ffi` (confirms both appear).

### Task 3: Extract `postretro-scripting-core`

Move the language runtime: `scripting/primitives_registry.rs`, `scripting/luau.rs`, `scripting/quickjs.rs`, `scripting/runtime/`, `scripting/ir/`, `scripting/typedef/`, `scripting/error.rs`, and the marshalling-orchestration remainder of `conv.rs` (the `json_to_js`/`json_to_lua`/`lua_to_json` bridges). Depends on `postretro-entity-core` with `script-ffi` enabled, plus `rquickjs`, `mlua`, `serde`, `serde_json`. Keep the sealed `RegisterablePrimitive` trait + `impl_registerable!` macro and the installer type aliases here. Note: `luau.rs` and `ir/` carry pre-existing `unsafe`; it travels with the move. The "No `unsafe` added" AC is a net-new delta gate — do not add `#![forbid(unsafe_code)]` or treat the inherited `unsafe` as a violation.

### Task 4: `ScriptingCore` sub-struct on `Session`

Group the scripting-tranche fields (script runtime, `ScriptCtx`, the registries, the `ScriptCtx`-clone consumers) into a `ScriptingCore` struct owned by `Session`. `Session::build` constructs it at the existing single site and distributes clones there; field access updates to go through the sub-struct. Behavior-preserving.

### Task 5: Relocate cross-runtime tests

Move the per-primitive tests that instantiate `rquickjs::Runtime` + `mlua::Lua` into `scripting-core/tests/` (a standalone `scripting-integration` crate only as the fallback per Decisions). Leave pure-logic handler tests (e.g. `apply_light_animation_inner`-style) with their handler so subsystem `cargo test` needs no VM. Note: some `reactions/*` files carry a cross-runtime parity test in a `#[cfg(test)]` block (e.g. `set_fog_params.rs`) — relocating those handlers whole means their VM-touching test module moves to `scripting-core/tests/`, while the handler's pure code lands in its subsystem.

### Task 6: Relocate primitive + reaction handlers

Relocate each handler to co-locate with the subsystem it exposes. **There is no single `register_all` reaction surface** — handlers register through **four distinct registry types**, all constructed at the single `Session::build` site (`crates/postretro/src/session/mod.rs`, lines ~315, ~333–334, ~339–340, ~346). A handler relocates *with all of its registration sites*.

| Registry | Registrar(s) (current location) | Takes `ScriptCtx`? | Handler shape |
|---|---|---|---|
| `PrimitiveRegistry` | `register_all` → `register_*` per domain (`primitives/mod.rs`) | yes | closure capturing `ScriptCtx` |
| `SequencedPrimitiveRegistry` | `register_sequenced_light_primitives`, `register_sequenced_fog_primitives` (`reactions/registry.rs`) | yes | `(id, args)` closure capturing `ScriptCtx` |
| `ReactionPrimitiveRegistry` | `register_emitter_reaction_primitives`, `register_fog_reaction_primitives` (`reactions/registry.rs`) | **no** | `(reg, targets, args)` closure → `dispatch(reg, targets, &parsed)` |
| `SystemReactionRegistry` | `register_system_reaction_primitives` (`reactions/system_commands.rs`) | **no** | enqueues onto `SystemCommandQueue` |

Several families register in **more than one** registry — **fog** registers in both `ReactionPrimitiveRegistry` and `SequencedPrimitiveRegistry`; **light** registers in both `PrimitiveRegistry` (via `register_all`) and `SequencedPrimitiveRegistry`. Move every site for a family together.

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

- **`reactions/*` (agnostic, zero non-test VM refs):** relocate the whole handler — its `Args` type, `dispatch` fn, and the registrar closure — to its subsystem module. The `dispatch(reg, targets, &parsed)` signature already takes native args (no VM types). `system_commands` handlers enqueue onto `SystemCommandQueue` (now in `entity-core`, Task 2); the **drain** runs through `reaction_dispatch.rs` via `ScriptCtx::system_commands` and stays in `postretro` (not the `scripting/systems/` bridges). Confirm zero `rquickjs`/`mlua` import in moved non-test code; any VM-touching `#[cfg(test)]` module goes to `scripting-core/tests/` (Task 5).
- **`primitives/*` (carry marshalling newtypes):** split. The pure handler logic moves to its subsystem module; the marshalling newtypes (`NullableString`, `WorldQueryFilterInput`, etc.), their `FromJs`/`IntoJs`/`FromLua`/`IntoLua` impls, and the `register_*` wiring that references them **stay in `scripting-core`**. The subsystem-side logic must not import `rquickjs`/`mlua`; the `scripting-core`-side wiring may. Confirm the subsystem module is VM-free after the split.

  **The seam:** the `scripting-core` `register_*` closure does the marshalling (decode script args via the newtypes, encode the result), then calls the subsystem-side pure-logic fn with native Rust args plus the `ScriptCtx`/cluster handle. The subsystem fn signature takes native types only — never `rquickjs`/`mlua` types. This already matches source: closure bodies today are one-line delegations to free fns (`apply_light_animation`, `read_store_slot`, `parse_query_filter` + collectors), so the split relocates those free fns and leaves the newtype-unpacking in the closure. For `worldQuery` specifically, `parse_query_filter` and the collectors relocate; the `WorldQueryFilterInput`→native unpacking and the `JsonValue` result wrap stay wiring-side.

`Session::build` remains the sole aggregation site: all four registries are constructed there and the relocated registrar fns are still *called* from there. Registration order within each registry must be preserved across the move — for `PrimitiveRegistry`, order is an invariant since the typedef generator's byte-identical output (AC) may depend on registry iteration order.

## Sequencing

**Phase 1 (sequential):** Task 1 — baseline gates all timing claims.
**Phase 2 (sequential):** Task 2 — `entity-core` is the dependency floor for everything below; the orphan-rule feature-gate must land before `scripting-core` can compile against it.
**Phase 3 (sequential):** Task 3 — `scripting-core` depends on Task 2's `script-ffi` feature.
**Phase 4 (concurrent):** Task 4, Task 5 — independent once both crates exist (Session grouping vs. test relocation). Task 5's relocated tests instantiate raw `rquickjs::Runtime` + `mlua::Lua` (not `Session`), so they don't touch Task 4's `ScriptingCore` restructuring.
**Phase 5 (sequential, after Task 4):** Task 6 — per-handler relocations fan out across handler families (each is an independent unit), but the phase as a whole runs after Task 4 because both Task 4 and Task 6 edit `Session::build`'s registration block (`register_all` call site + clone distribution). Within the phase, handler families parallelize. Task 5's relocated cross-runtime tests (already in `scripting-core/tests/` from Phase 4) exercise primitives through the registry/runtime public API, not handler file paths, so the Task 6 handler split doesn't invalidate them; Task 6 fixes any test import path it does touch.

Structural surgery (Phases 2–3) is serialized deliberately; the handler fan-out is Phase 5, gated behind Task 4's `Session::build` restructuring.

## Rough sketch

**Crate layout.**
- `crates/entity-core/` (`postretro-entity-core`): data model. Default = no VMs. Feature `script-ffi = ["dep:rquickjs", "dep:mlua"]` compiles the entity-type FFI impls.
- `crates/scripting-core/` (`postretro-scripting-core`): language runtime. Depends on `postretro-entity-core = { workspace = true, features = ["script-ffi"] }`.

**Orphan-rule resolution (the crux).** `conv.rs` today does `impl rquickjs::FromJs for EntityId` etc. — legal only while `EntityId` is local. After extraction, that impl is legal **only in the crate that owns the type**. So the entity-type marshalling impls move into `entity-core` under `script-ffi`; trait foreign, type local → allowed. The `Vec3Lit`/`EulerDegrees` newtypes already in `conv.rs` are the established pattern for wrapping foreign types (glam) to satisfy this. `conv.rs` physically splits along the crate boundary: the entity-type FFI impls plus the `Vec3Lit`/`EulerDegrees` newtypes they depend on move to `entity-core` under `script-ffi` (they wrap glam, so their impls are legal only where they live); the marshalling-orchestration bridges (`json_to_js`/`js_to_json`/`json_to_lua`/`lua_to_json`) move to `scripting-core`. Component-type impls (`LightComponent`, `LightAnimation`) ride with their component defs (`components/light.rs`) under the same feature. Cargo feature unification compiles `entity-core` once with `script-ffi` on in the full build, but `rquickjs`/`mlua` remain upstream deps — editing a handler or bridge in `postretro` never rebuilds them. That is the firewall.

**Aggregation unchanged.** `register_all` (primitives/mod.rs) stays the explicit primitive entry; its sibling registrars for the other three registries (`register_emitter_reaction_primitives`/`register_fog_reaction_primitives`, `register_system_reaction_primitives`, `register_sequenced_light_primitives`/`register_sequenced_fog_primitives`) stay explicit too (no `inventory`/`linkme`). `Session::build` stays the single runtime construction site for all four registries. The "one indivisible group" stays indivisible at runtime (shared `Rc`) and becomes the `ScriptingCore` type at compile time.

**Naming.** Working names `postretro-entity-core` / `postretro-scripting-core`; final names decided at implementation. `entity-core` is the scripting *data model* (wider than the entity registry alone) — a rename (e.g. `script-model`) is acceptable if it reads truer.

## Decisions (resolved during review)

- **`entity-core` is one crate**, not two — registry + components + ctx + data_registry + slot_table + command queue ship together as the scripting data model. (Splitting pure-registry from broader script-state would multiply Task 2 call-site churn for no compile-firewall gain; the whole cluster is the dependency floor.)
- **Cross-runtime tests land in `scripting-core/tests/`.** A standalone `scripting-integration` crate is the fallback only if a test drags handler-specific fixtures that don't belong in `scripting-core`.
- **A1 (primitive-handler marshalling):** the `primitives/*` marshalling newtypes and their `register_*` wiring stay in `scripting-core`; only the pure handler logic relocates to subsystems (see Scope, Task 6).

## Open questions

- If Task 6's handler count makes the spec unwieldy at orchestration time, Phase 5 may split into its own follow-up spec — the Phase 1–4 boundary is independently shippable.
