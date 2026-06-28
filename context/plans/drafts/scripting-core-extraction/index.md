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
- Relocate script-callable primitive handlers (`scripting/primitives/`, `scripting/reactions/`) to co-locate with the subsystem each exposes, registering against `scripting-core`.
- Quote before/after `--timings` from the compile-time-reduction Task 1 baseline on each extraction.

### Out of scope

- Crate-ifying the bridges (`scripting/systems/`) — they stay in `postretro` (render/FX-coupled glue).
- Crate-ifying render/audio/lighting/movement subsystems — handler "co-location" means same module tree in `postretro` depending on the new crates; the handler-side firewall fully lands when those subsystems crate-ify later.
- `inventory`/`linkme` distributed registration — keep explicit `register_all` → `register_*` aggregation from `Session::build`.
- Any change to primitive names, wire shapes, marshalling results, SDK typedef output, or PRL format.
- PRL-loader / visibility extraction (separate compile-time-reduction tasks).

## Acceptance criteria

- [ ] `postretro-entity-core` and `postretro-scripting-core` exist as workspace crates; `cargo build --workspace` and `cargo test --workspace` pass.
- [ ] `cargo tree -p postretro-entity-core` (default features) shows **no** `rquickjs` or `mlua`. Enabling `--features script-ffi` adds both.
- [ ] The bridge crate/modules and the relocated handlers depend on `postretro-entity-core` **without** `script-ffi`; only `scripting-core` enables it.
- [ ] Touching a relocated primitive handler, then rebuilding, does **not** recompile `rquickjs`, `mlua`, `glyphon`, `wgpu`, `kira`, or `winit` (verified against `--timings`).
- [ ] The SDK typedef drift test (`cargo test`, §SDK Type Definitions) passes unchanged — generated `postretro.d.ts` / `postretro.d.luau` are byte-identical to pre-refactor.
- [ ] All primitives install and behave identically in both QuickJS and Luau (existing cross-runtime tests pass from their new home).
- [ ] `Session` exposes the scripting tranche through a `ScriptingCore` sub-struct; `Session::build` remains the sole construction site and still distributes all `ScriptCtx` clones there.
- [ ] No `unsafe` added. No `wgpu` call moves out of the renderer.
- [ ] Each extraction PR quotes before/after timings for the targeted edit loops from the baseline commands.

## Tasks

### Task 1: Baseline dependency (gate)

Consume the compile-time-reduction Task 1 baseline. If unavailable, capture the minimum needed here: warm `cargo check -p postretro`, a primitive-handler touch rebuild, and a `--timings` run identifying `rquickjs-sys`/`mlua-sys`/`luau0-src` critical-path position. All later tasks quote against this.

### Task 2: Extract `postretro-entity-core`

Move the VM-free data model: `scripting/registry.rs`, `scripting/ctx.rs`, `scripting/components/`, `scripting/data_registry.rs`, `scripting/slot_table.rs`, the `SystemCommandQueue` type from `scripting/reactions/system_commands.rs`, and `scripting/provenance.rs`. Default deps: `glam`, `serde`, `serde_json`, `thiserror`. Add an optional `script-ffi` feature (`dep:rquickjs`, `dep:mlua`) that compiles the FFI marshalling impls for entity-core-owned types (the `EntityId`/`Transform`/`ComponentKind`/`ComponentValue` slice of `conv.rs`). Update all call sites; `EntityId` stays an opaque handle to non-scripting modules. Mirror `crates/level-format/Cargo.toml`'s optional-feature shape.

### Task 3: Extract `postretro-scripting-core`

Move the language runtime: `scripting/primitives_registry.rs`, `scripting/luau.rs`, `scripting/quickjs.rs`, `scripting/runtime/`, `scripting/ir/`, `scripting/typedef/`, `scripting/error.rs`, and the marshalling-orchestration remainder of `conv.rs` (the `json_to_js`/`json_to_lua`/`lua_to_json` bridges). Depends on `postretro-entity-core` with `script-ffi` enabled, plus `rquickjs`, `mlua`, `serde`, `serde_json`. Keep the sealed `RegisterablePrimitive` trait + `impl_registerable!` macro and the installer type aliases here.

### Task 4: `ScriptingCore` sub-struct on `Session`

Group the scripting-tranche fields (script runtime, `ScriptCtx`, the registries, the `ScriptCtx`-clone consumers) into a `ScriptingCore` struct owned by `Session`. `Session::build` constructs it at the existing single site and distributes clones there; field access updates to go through the sub-struct. Behavior-preserving.

### Task 5: Relocate cross-runtime tests

Move the per-primitive tests that instantiate `rquickjs::Runtime` + `mlua::Lua` into `scripting-core` or a dedicated integration test crate. Leave pure-logic handler tests (e.g. `apply_light_animation_inner`-style) with their handler so subsystem `cargo test` needs no VM.

### Task 6: Relocate primitive + reaction handlers

Move each handler to co-locate with the subsystem it exposes, each exposing a `register_*(&mut PrimitiveRegistry, ScriptCtx)` fn against `scripting-core`, still called by `register_all` from `Session::build`. Mapping in `research.md` (light→lighting, world/entity→entity, fog→fog, emitter→particles, damage→health, animation→mesh, system_commands→command queue). Per-handler, confirm no `rquickjs`/`mlua` import remains in the relocated non-test code.

## Sequencing

**Phase 1 (sequential):** Task 1 — baseline gates all timing claims.
**Phase 2 (sequential):** Task 2 — `entity-core` is the dependency floor for everything below; the orphan-rule feature-gate must land before `scripting-core` can compile against it.
**Phase 3 (sequential):** Task 3 — `scripting-core` depends on Task 2's `script-ffi` feature.
**Phase 4 (concurrent):** Task 4, Task 5 — independent once both crates exist (Session grouping vs. test relocation).
**Phase 5 (concurrent):** Task 6 — per-handler relocations fan out; low-conflict once Phases 2–3 fix the boundary. Each handler family is an independent unit.

Structural surgery (Phases 2–3) is serialized deliberately; the fan-out is Phase 5.

## Rough sketch

**Crate layout.**
- `crates/entity-core/` (`postretro-entity-core`): data model. Default = no VMs. Feature `script-ffi = ["dep:rquickjs", "dep:mlua"]` compiles the entity-type FFI impls.
- `crates/scripting-core/` (`postretro-scripting-core`): language runtime. Depends on `postretro-entity-core = { workspace = true, features = ["script-ffi"] }`.

**Orphan-rule resolution (the crux).** `conv.rs` today does `impl rquickjs::FromJs for EntityId` etc. — legal only while `EntityId` is local. After extraction, that impl is legal **only in the crate that owns the type**. So the entity-type marshalling impls move into `entity-core` under `script-ffi`; trait foreign, type local → allowed. The `Vec3Lit`/`EulerDegrees` newtypes already in `conv.rs` are the established pattern for wrapping foreign types (glam) to satisfy this. Component-type impls (`LightComponent`, `LightAnimation`) ride with their component defs under the same feature. Cargo feature unification compiles `entity-core` once with `script-ffi` on in the full build, but `rquickjs`/`mlua` remain upstream deps — editing a handler or bridge in `postretro` never rebuilds them. That is the firewall.

**Aggregation unchanged.** `register_all` (primitives/mod.rs) stays the explicit entry; `Session::build` stays the single runtime construction site. The "one indivisible group" stays indivisible at runtime (shared `Rc`) and becomes the `ScriptingCore` type at compile time.

**Naming.** Working names `postretro-entity-core` / `postretro-scripting-core`; final names decided at implementation. `entity-core` is the scripting *data model* (wider than the entity registry alone) — a rename (e.g. `script-model`) is acceptable if it reads truer.

## Open questions

- Does `entity-core`'s scope (registry + components + ctx + data_registry + slot_table + command queue) want one crate or two (pure entity registry vs. broader script-state model)? Decide against Task 2 call-site churn; default is one crate.
- Cross-runtime test relocation target: fold into `scripting-core`'s own `tests/`, or a standalone `scripting-integration` crate? Default: `scripting-core/tests/` unless it drags handler-specific fixtures.
- If Task 6's handler count makes the spec unwieldy at orchestration time, Phase 5 may split into its own follow-up spec — the Phase 1–4 boundary is independently shippable.
