# Engine-Data Floor Extraction

> **Status:** draft
> **Related:** `context/lib/scripting.md` §12 · `context/lib/build_pipeline.md` · `context/lib/development_guide.md` · `context/plans/drafts/compile-time-reduction/` · `context/plans/ready/scripting-core-extraction/` (this supersedes its Task 2 and reshapes its Task 3) · sibling `research.md`

## Goal

Extract the VM-free engine-data + evaluation substrate out of the `postretro` binary crate into a layered crate floor, so the entity/scripting data model stops recompiling as part of the engine binary and gives combat (Epic 16), enemy behavior, and networking a clean dependency base. Behavior-preserving: no runtime, wire-format, scripting-semantics, or SDK-typedef change. This is the foundation the scripting-core (VM runtime) extraction and handler relocation then build on.

## Why a re-draft

`scripting-core-extraction` assumed `scripting/components/` was a clean VM-free leaf movable into one crate. It is not: components couple to `movement`/`nav`/`weapon`/`ai` and to the IR substrate, and `ComponentValue` embeds every component **by value**, so the registry transitively pulls those couplings. The floor is larger and **layered**. See `research.md` for the source-grounded dependency map and the seven cycles this plan breaks.

## Scope

### In scope

- Extract a lower **substrate** crate (working name `postretro-sim-substrate`): the IR evaluator core, the `MovementScope`/`PlayerMovementComponent` cluster, the **`EntityId`-free** POD descriptor *types* (movement/weapon/health/ai/light/mesh + entity-type), the POD value types (`Vec3Lit`/`EulerDegrees`), and the sunk subsystem PODs (`DamagePayload`, `NavAgentParams`, `ModMapEntry`).
- Extract an upper **data-model** crate (working name `postretro-entity-core`): the entity registry, `ComponentValue`, the remaining components, `ScriptCtx`, the slot table + engine-state catalog, provenance, the system-command queue, the **reaction/crossing descriptor types** (they reference `EntityId` — see the partition rule below), and `DataRegistry`. Depends on the substrate.
- **Descriptor partition rule:** a descriptor type that references `EntityId` lives in the **data-model** crate (with the registry); `EntityId`-free descriptor types live in the **substrate**. The implementer verifies each descriptor by this rule — do not rely on the enumerations above being exhaustive.
- Break the seven cross-crate cycles in place first (behavior-preserving): split `data_descriptors` types from VM converters; relocate `Vec3Lit`/`EulerDegrees` out of VM-coupled `conv.rs`; sink the subsystem PODs; carve `ir/scopes.rs` from the IR core. (An eighth cycle, `SequenceStep → EntityId`, is broken by *placement* — the partition rule below — not by an in-place move.)
- Invert `movement`/`nav`/`weapon`/`ai` (and the opaque-handle consumers) to depend *up* on the floor crates.
- Per-crate orphan-rule FFI: each floor crate owns its types' marshalling impls behind an optional `script-ffi` feature; only the runtime crate enables it.

### Out of scope

- Extracting the VM runtime crate (`scripting-core`) — the rebased `scripting-core-extraction` follow-on owns that.
- Relocating primitive/reaction handlers to subsystems (the A1 work) — also the follow-on.
- Refactoring GPU-free `render::ui` descriptor/layout/style types into a CPU-only model crate — that is `compile-time-reduction` Task 6. (Deciding that the UI-coupled manifest types `RegisteredUiTree`/`LevelManifest` stay out of the floor *is* in scope — Task 2 — but this plan does not touch their internals or the `render::ui` types they embed.)
- PRL-loader / visibility extraction (`compile-time-reduction` Tasks 3–5).
- `inventory`/`linkme`. Any change to primitive names, wire shapes, marshalling results, SDK typedef output, PRL format, or runtime behavior.

## Acceptance criteria

- [ ] The substrate and data-model crates exist as workspace members; `cargo build --workspace` and `cargo test --workspace` pass.
- [ ] `cargo tree` for each floor crate **in isolation** (default features) shows **no** `rquickjs`, `mlua`, `wgpu`, `winit`, `glyphon`, or `kira`. Each crate's `--features script-ffi` adds `rquickjs`/`mlua` only.
- [ ] The dependency graph is acyclic and one-way: data-model → substrate; `movement`/`nav`/`weapon`/`ai` and the opaque-handle consumers (render/audio/netcode/agent_steering/collision) depend on the floor crates, never the reverse. `cargo build --workspace` proves no cycle.
- [ ] Editing a floor crate and running *its* tests does not recompile `rquickjs`, `mlua`, `wgpu`, `glyphon`, `kira`, or `winit`.
- [ ] `RegisteredUiTree`/`LevelManifest` and the `data_descriptors` VM converters remain in `postretro`; no floor crate depends on `render::ui` or the VMs by default.
- [ ] Placement holds: `DamagePayload`/`NavAgentParams`/`ModMapEntry` and the `EntityId`-free descriptors resolve from the substrate crate; the reaction/crossing descriptors (which reference `EntityId`), `apply_damage`/`attach_agent`, and `EntityId` itself resolve from the data-model crate; `weapon`/`nav` define neither sunk POD; `MenuCamera`/`Frontend` stay in `postretro`.
- [ ] The SDK typedef drift test passes — generated `postretro.d.ts`/`postretro.d.luau` byte-identical to pre-refactor.
- [ ] All existing scripting/movement/combat/nav tests pass unchanged from their relocated homes; cross-runtime parity tests still pass.
- [ ] No `unsafe` added (pre-existing `unsafe` in moved code travels; the gate is net-new only). No `wgpu` call moves out of the renderer. No PRL/wire/semantics change.
- [ ] Each extraction PR quotes before/after timings from the baseline (Task 1) for the targeted edit loops.

## Tasks

### Task 1: Baseline (gate)

Reuse the baseline already captured by the halted `scripting-core-extraction` orchestration (its Task 1 completed) or the `compile-time-reduction` Task 1 baseline. If neither is available, capture: warm `cargo check -p postretro`; a touch-rebuild of `crates/postretro/src/scripting/registry.rs`; and `cargo build -p postretro --timings` noting where `rquickjs-sys`/`mlua-sys`/`luau0-src` and the `postretro` self-compile sit on the critical path. All later tasks quote against this.

### Task 2: Break the cycles in place (behavior-preserving, no crate yet)

Stage the structural moves inside `postretro` before any crate boundary, so the extractions in Tasks 3–4 are clean module moves rather than refactors-under-transplant. Each sub-step preserves the public surface (re-export from the old path) so call sites keep compiling. **`data_descriptors` purity is at the *file* level, not just the directory** — the `types/` dir already exists but every file reaches its imports via `use super::*`/`use super::super::*` against the VM+`render::ui` glob in `mod.rs`, and two files mix down-bound and up-bound types. So splitting means per-file import surgery, not just moving directories:

- **Split `data_descriptors` types from converters.** The VM converters (`data_descriptors/{js,lua}/`) and the `mod.rs` VM/`render::ui` glob stay on the runtime side. For each descended `types/*.rs` (and `error.rs`/`validate.rs`), **replace the `use super::*` glob with explicit, VM-free imports** — that is the load-bearing edit; the dir already existing does not mean the split is done.
- **Split `validate.rs`.** It is *not* wholesale-movable: it mixes pure numeric/crossing/IR validators (descend) with `mlua::Table`-coupled validators (e.g. `validate_dense_lua_array`) that stay runtime-side. Carve the `mlua`-using helpers out; only the pure validators + `DescriptorError` descend.
- **Split `types/manifest.rs`.** `ModThemeTokens`/`ModFontAssets` are POD and descend; `RegisteredUiTree`/`LevelManifest` embed `render::ui::AnchoredTree` and **stay runtime-side**. `LevelManifest` also embeds the reaction/crossing descriptors (which go to the data-model crate, per the partition rule) plus `RegisteredUiTree` (runtime) — so `LevelManifest` itself stays runtime-side, and `DataRegistry::populate_from_manifest`'s destructuring moves runtime-side too: the runtime caller destructures `LevelManifest` and hands `DataRegistry` the already-extracted POD descriptors via POD-typed setters, so `DataRegistry`'s public API never names `LevelManifest`.
- **Relocate `Vec3Lit`/`EulerDegrees`** out of VM-coupled `conv.rs` into a VM-free module (`Vec3Lit` is stored by value in `light`/`billboard_emitter`; `EulerDegrees` is FFI-boundary-only). During this in-place stage their `FromJs`/`IntoJs`/`FromLua`/`IntoLua` impls **stay co-located in a module that `postretro` compiles unconditionally** (no real `cfg` yet — `postretro` always has the VMs); the impls become `#[cfg(feature = "script-ffi")]`-gated only when the substrate crate is created in Task 3. Behavior must not change — the FFI impls keep compiling in `postretro` throughout.
- **Sink the subsystem PODs.** Move `DamagePayload` (`weapon/damage.rs`) and `NavAgentParams` (`nav/mod.rs`) into VM-free modules re-exported from their old paths. `ModMapEntry` (`runtime/types.rs`) likewise. **`MenuCamera`/`Frontend` stay in `postretro`** — `DataRegistry` does not store them, so the floor does not need them.
- **Carve `ir/scopes.rs`** so the IR core (`ir/{mod,bind,eval,scope,load}.rs`) has no dependency on `ScriptCtx`/`slot_table`/`primitives`; `StoreScope` stays runtime-side. This also edits `ir/mod.rs` (the `mod scopes;` declaration and the `StoreScope`/`StoreCapability` re-export move runtime-side).
- **Verify `DataRegistry` is already slim.** It stores only POD descriptors + `ModMapEntry` and never stores `ui_trees` (it destructures them away). Confirm no stored field type pulls `render::ui` or the VMs; expect **no removal** — this sub-step is a verification, not surgery.

Build green after each sub-step. The genuinely disjoint sub-steps (the `DamagePayload`/`NavAgentParams`/`ModMapEntry` sinks, the `ir/scopes.rs` carve) may fan out; the `data_descriptors` sub-steps are ordered — the `validate.rs`/`manifest.rs` splits and the `DataRegistry` verification follow the types/converters split, since they reference paths it churns.

### Task 3: Extract the substrate crate

Move the cluster from Task 2 into a new lower crate: IR core, `MovementScope` + `PlayerMovementComponent` + `MovementState`/`DashPrograms`, the **`EntityId`-free** POD descriptor types + the pure validators carved in Task 2 (`DescriptorError`, numeric/crossing/IR validators), `Vec3Lit`/`EulerDegrees`, and the sunk PODs (`DamagePayload`/`NavAgentParams`/`ModMapEntry`). The reaction/crossing descriptor types do **not** go here — they reference `EntityId` and belong in the data-model crate (Task 4). Default deps: `glam`, `serde`, `serde_json`, `thiserror`, `log`. Add an optional `script-ffi` feature (`dep:rquickjs`, `dep:mlua`) compiling the FFI impls for the substrate's own types. Mirror `crates/level-format/Cargo.toml`. `movement`/`nav`/`weapon` flip to depend on it for the sunk types. Widen every moved symbol referenced across the new crate boundary from `pub(crate)` to `pub` (preserve opacity via private fields, not visibility — e.g. `Vec3Lit`'s inner array stays private). Quote before/after edit-loop timings against the Task 1 baseline in the PR.

### Task 4: Extract the data-model crate

Move `registry.rs`, the remaining `components/*` + their `&EntityRegistry` behavior fns (`apply_damage`, `attach_agent`, mesh/brain), `ctx.rs`, `slot_table.rs` + `engine_state_catalog.rs`, `provenance.rs`, `scripting/error.rs` (`ScriptError` — **not** `data_descriptors/error.rs`, which is `DescriptorError` and descended in Task 3), `reactions/system_commands.rs` (queue + command enum), the **reaction/crossing descriptor types** (`SequenceStep`/`NamedReaction`/`ReactionDescriptor`/`PrimitiveDescriptor`/`ProgressDescriptor`/`CrossingCondition`/`CrossingDescriptor` — they embed `EntityId`), and `DataRegistry` into a new upper crate. **Cargo wiring (load-bearing):** depend on the substrate with `default-features = false`, and make this crate's own `script-ffi` feature *forward* to the substrate's — `script-ffi = ["dep:rquickjs", "dep:mlua", "postretro-sim-substrate/script-ffi"]`. Do **not** enable the substrate's `script-ffi` unconditionally, or a default `cargo tree` of this crate pulls the VMs and violates the isolation AC. This crate's `script-ffi` compiles the FFI impls for `EntityId`/`Transform`/`ComponentKind`/`ComponentValue`/components/reaction descriptors. `EntityId` stays an opaque handle to non-scripting modules (private field; `to_raw`/`from_raw` widen to `pub` for the runtime crate's `conv.rs` to reach). Widen moved `pub(crate)` symbols to `pub` as in Task 3. Quote before/after timings against the Task 1 baseline in the PR.

### Task 5: Consumer flips + verification

**Productive work:** Tasks 3–4 already flip the subsystems whose dependency they create (`movement`/`nav`/`weapon` for the sunk types; the data-model's own consumers). Task 5 flips the *remaining* consumers — `ai` (`scripting/systems/ai.rs`) and the opaque-handle consumers (render/audio/netcode/agent_steering/collision) — to import `EntityId`/components/descriptors from the floor crates instead of `scripting::`. These are import-path updates (the consumers already treat the types as opaque handles / data), low-conflict and per-subsystem, so they fan out.

**Verification gate:** confirm no reverse edge (`cargo build --workspace` proves acyclic); run the isolation `cargo tree` checks and the touch-rebuild timing checks from the AC; verify the typedef drift test is byte-identical.

## Sequencing

**Phase 1 (sequential):** Task 1 — baseline gates the timing claims.
**Phase 2 (sequential):** Task 2 — the in-place cycle-breaking is the foundation; its sub-steps fan out internally but the phase gates the extractions.
**Phase 3 (sequential):** Task 3 — the substrate is the dependency floor; the data-model can't compile against it until it exists.
**Phase 4 (sequential):** Task 4 — the data-model crate depends on Task 3's substrate (and its `script-ffi` feature).
**Phase 5 (concurrent):** Task 5 — verification + the consumer-flip sweep fans out across subsystems once both crates exist.

Structural surgery (Phases 2–4) is serialized deliberately; the verification/flip fan-out is Phase 5.

## Rough sketch

**Crate layout (two layers, one-way edge).**
- `crates/sim-substrate/` (`postretro-sim-substrate`): IR evaluator core + movement/IR cluster + `EntityId`-free descriptor & value PODs + sunk subsystem PODs. Default = no VMs. `script-ffi = ["dep:rquickjs","dep:mlua"]` compiles its types' FFI impls.
- `crates/entity-core/` (`postretro-entity-core`): registry + `ComponentValue` + remaining components + `ScriptCtx` + registries + `DataRegistry` + the reaction/crossing descriptors. Depends on `postretro-sim-substrate` with `default-features = false`; its `script-ffi` forwards to the substrate's (`script-ffi = ["dep:rquickjs","dep:mlua","postretro-sim-substrate/script-ffi"]`) so a default `cargo tree` of this crate stays VM-free.

**Why layered, not one crate.** The `{IR core + MovementScope + PlayerMovementComponent + descriptor PODs}` cluster has no edge up into `EntityId`/`EntityRegistry`/`ComponentValue` (research.md §3) — a clean one-way edge. One crate would force the IR/movement substrate and the registry to recompile together for no structural reason; both grow under Epic 16 (combat adds descriptors and IR adoption; the registry grows components). Splitting lets each recompile independently.

**Orphan-rule, per crate.** Each crate owns the marshalling impls for the types it defines, behind its own optional `script-ffi` feature pulling the VMs — `impl rquickjs::FromJs for LocalType` is legal only where the type is local. Default builds have no VM deps; the runtime crate enables both features. Foreign types (glam) wrap in local newtypes (`Vec3Lit`/`EulerDegrees`). This is `scripting.md §12`'s contract, applied to two crates.

**What stays in `postretro` (the future runtime crate).** `ir/scopes.rs` (`StoreScope`), `conv.rs` FFI/json bridges, `data_descriptors/{js,lua}/` converters + the `render::ui` glob, `RegisteredUiTree`/`LevelManifest`, `runtime/*`, `luau.rs`/`quickjs.rs`/`primitives/*`/`primitives_registry.rs`/`reaction_dispatch.rs`/typedef generator.

**Relationship to `scripting-core-extraction`.** That spec's Task 2 (extract `entity-core`) is replaced by this plan. Its Task 3 (extract `scripting-core`) is reshaped to pull the VM-coupled remainder above and enable `script-ffi` on both floor crates. Its Tasks 4–6 (ScriptingCore sub-struct, test relocation, handler relocation) ride unchanged on top. After this lands, rebase that spec onto the floor before re-orchestrating it.

## Decisions

- **Two-layer floor**, not one crate — the cluster's one-way edge makes it clean and lets the substrate and registry recompile independently as both grow.
- **Descriptor partition by `EntityId`-reference.** Descriptor types that reference `EntityId` (the reaction/crossing descriptors) live in the data-model crate with the registry; `EntityId`-free descriptors live in the substrate. This breaks the `SequenceStep → EntityId` cross-crate cycle by placement rather than by sinking `EntityId` down (which would change the thesis — `EntityId` stays the data-model's opaque handle).
- **POD-sink inversion** for the `health→weapon` and `agent→nav` edges: sink the trivial PODs (`DamagePayload`, `NavAgentParams`) into the substrate; the behavior fns (`apply_damage`, `attach_agent`) stay with their component. `ModMapEntry` sinks too; `MenuCamera`/`Frontend` do not (the floor doesn't consume them).
- **`data_descriptors` splits at the *file* level** — types (down) from VM converters + UI-manifest types (stay up). `validate.rs` and `types/manifest.rs` are mixed files that must be carved, not moved whole; `LevelManifest` stays runtime-side and `DataRegistry`'s manifest population takes POD pieces. The `render::ui` CPU-only-model cleanup is deferred to `compile-time-reduction` Task 6.
- **`ir/scopes.rs` stays in the runtime crate** (pulls `ScriptCtx`/`slot_table`/`primitives`); only the VM-free IR core descends.

## Open questions

- Final crate names (`postretro-sim-substrate` / `postretro-entity-core` are working names). Decide at implementation; the lower crate is wider than movement or IR alone.
