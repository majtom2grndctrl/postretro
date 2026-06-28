# Engine-Data Floor Extraction

> **Status:** draft
> **Related:** `context/lib/scripting.md` §12 · `context/lib/build_pipeline.md` · `context/lib/development_guide.md` · `context/plans/drafts/compile-time-reduction/` · `context/plans/ready/scripting-core-extraction/` (this supersedes its Task 2 and reshapes its Task 3) · sibling `research.md`

## Goal

Extract the VM-free engine-data + evaluation substrate out of the `postretro` binary crate into a layered crate floor, so the entity/scripting data model stops recompiling as part of the engine binary and gives combat (Epic 16), enemy behavior, and networking a clean dependency base. Behavior-preserving: no runtime, wire-format, scripting-semantics, or SDK-typedef change. This is the foundation the scripting-core (VM runtime) extraction and handler relocation then build on.

## Why a re-draft

`scripting-core-extraction` assumed `scripting/components/` was a clean VM-free leaf movable into one crate. It is not: components couple to `movement`/`nav`/`weapon`/`ai` and to the IR substrate, and `ComponentValue` embeds every component **by value**, so the registry transitively pulls those couplings. The floor is larger and **layered**. See `research.md` for the source-grounded dependency map and the seven cycles this plan breaks.

## Scope

### In scope

- Extract a lower **substrate** crate (working name `postretro-sim-substrate`): the IR evaluator core, the `MovementScope`/`PlayerMovementComponent` cluster, the POD descriptor *types*, the POD value types (`Vec3Lit`/`EulerDegrees`), and the sunk subsystem PODs (`DamagePayload`, `NavAgentParams`, `ModMapEntry`).
- Extract an upper **data-model** crate (working name `postretro-entity-core`): the entity registry, `ComponentValue`, the remaining components, `ScriptCtx`, the slot table + engine-state catalog, provenance, the system-command queue, and a slimmed `DataRegistry`. Depends on the substrate.
- Break the seven cross-crate cycles in place first (behavior-preserving): split `data_descriptors` types from VM converters; relocate `Vec3Lit`/`EulerDegrees` out of VM-coupled `conv.rs`; sink the subsystem PODs; carve `ir/scopes.rs` from the IR core.
- Invert `movement`/`nav`/`weapon`/`ai` (and the opaque-handle consumers) to depend *up* on the floor crates.
- Per-crate orphan-rule FFI: each floor crate owns its types' marshalling impls behind an optional `script-ffi` feature; only the runtime crate enables it.

### Out of scope

- Extracting the VM runtime crate (`scripting-core`) — the rebased `scripting-core-extraction` follow-on owns that.
- Relocating primitive/reaction handlers to subsystems (the A1 work) — also the follow-on.
- Moving GPU-free `render::ui` descriptor/layout/style types into a CPU-only model crate — that is `compile-time-reduction` Task 6; this plan only keeps the UI-coupled manifest types (`RegisteredUiTree`/`LevelManifest`) **out** of the floor, it does not refactor them.
- PRL-loader / visibility extraction (`compile-time-reduction` Tasks 3–5).
- `inventory`/`linkme`. Any change to primitive names, wire shapes, marshalling results, SDK typedef output, PRL format, or runtime behavior.

## Acceptance criteria

- [ ] The substrate and data-model crates exist as workspace members; `cargo build --workspace` and `cargo test --workspace` pass.
- [ ] `cargo tree` for each floor crate **in isolation** (default features) shows **no** `rquickjs`, `mlua`, `wgpu`, `winit`, `glyphon`, or `kira`. Each crate's `--features script-ffi` adds `rquickjs`/`mlua` only.
- [ ] The dependency graph is acyclic and one-way: data-model → substrate; `movement`/`nav`/`weapon`/`ai` and the opaque-handle consumers (render/audio/netcode/agent_steering/collision) depend on the floor crates, never the reverse. `cargo build --workspace` proves no cycle.
- [ ] Editing a floor crate and running *its* tests does not recompile `rquickjs`, `mlua`, `wgpu`, `glyphon`, `kira`, or `winit`.
- [ ] `RegisteredUiTree`/`LevelManifest` and the `data_descriptors` VM converters remain in `postretro`; no floor crate depends on `render::ui` or the VMs by default.
- [ ] The SDK typedef drift test passes — generated `postretro.d.ts`/`postretro.d.luau` byte-identical to pre-refactor.
- [ ] All existing scripting/movement/combat/nav tests pass unchanged from their relocated homes; cross-runtime parity tests still pass.
- [ ] No `unsafe` added (pre-existing `unsafe` in moved code travels; the gate is net-new only). No `wgpu` call moves out of the renderer. No PRL/wire/semantics change.
- [ ] Each extraction PR quotes before/after timings from the baseline (Task 1) for the targeted edit loops.

## Tasks

### Task 1: Baseline (gate)

Reuse the baseline already captured by the halted `scripting-core-extraction` orchestration (its Task 1 completed) or the `compile-time-reduction` Task 1 baseline. If neither is available, capture: warm `cargo check -p postretro`; a touch-rebuild of `crates/postretro/src/scripting/registry.rs`; and `cargo build -p postretro --timings` noting where `rquickjs-sys`/`mlua-sys`/`luau0-src` and the `postretro` self-compile sit on the critical path. All later tasks quote against this.

### Task 2: Break the cycles in place (behavior-preserving, no crate yet)

Stage the structural moves inside `postretro` before any crate boundary, so the extractions in Tasks 3–4 are clean module moves rather than refactors-under-transplant. Each sub-step preserves the public surface (re-export from the old path) so call sites keep compiling:

- **Split `data_descriptors`.** Separate the POD descriptor *types* (`data_descriptors/types/*.rs`) from the VM converters (`data_descriptors/{js,lua}/`) and the `render::ui`/VM glob in `mod.rs`. Keep `RegisteredUiTree`/`LevelManifest` (which embed `render::ui::AnchoredTree`) on the converter/runtime side. Result: the type structs import no `render::ui` and no VM.
- **Relocate `Vec3Lit`/`EulerDegrees`** out of VM-coupled `conv.rs` into a VM-free module; leave their `FromJs`/`IntoJs`/`FromLua`/`IntoLua` impls behind the future `script-ffi` boundary (a `#[cfg(feature)]`-shaped module today).
- **Sink the subsystem PODs.** Move `DamagePayload` (`weapon/damage.rs`), `NavAgentParams` (`nav/mod.rs`), and `ModMapEntry`/`MenuCamera`/`Frontend` (`runtime/types.rs`) into VM-free modules, re-exported from their old paths so `weapon`/`nav`/`runtime` keep their current call sites.
- **Carve `ir/scopes.rs`** so the IR core (`ir/{mod,bind,eval,scope,load}.rs`) has no dependency on `ScriptCtx`/`slot_table`/`primitives`; `StoreScope` stays on the runtime side.
- **Slim `DataRegistry`** so it stores only POD descriptor types + `ModMapEntry` (it already drops `ui_trees`); confirm no field type pulls `render::ui` or the VMs.

Build green after each sub-step. The sub-steps are largely disjoint and may fan out.

### Task 3: Extract the substrate crate

Move the cluster from Task 2 into a new lower crate: IR core, `MovementScope` + `PlayerMovementComponent` + `MovementState`/`DashPrograms`, the POD descriptor types + pure validators (`DescriptorError`, numeric/crossing/IR validators), `Vec3Lit`/`EulerDegrees`, and the sunk PODs. Default deps: `glam`, `serde`, `serde_json`, `thiserror`, `log`. Add an optional `script-ffi` feature (`dep:rquickjs`, `dep:mlua`) compiling the FFI impls for the substrate's own types. Mirror `crates/level-format/Cargo.toml`. `movement`/`nav`/`weapon` flip to depend on it for the sunk types.

### Task 4: Extract the data-model crate

Move `registry.rs`, the remaining `components/*` + their `&EntityRegistry` behavior fns (`apply_damage`, `attach_agent`, mesh/brain), `ctx.rs`, `slot_table.rs` + `engine_state_catalog.rs`, `provenance.rs`, `error.rs`, `reactions/system_commands.rs` (queue + command enum), and the slimmed `DataRegistry` into a new upper crate depending on the substrate (with `script-ffi` for the substrate's marshalling). Add its own optional `script-ffi` feature for `EntityId`/`Transform`/`ComponentKind`/`ComponentValue`/component FFI impls. `EntityId` stays an opaque handle to non-scripting modules.

### Task 5: Inversion sweep + verification

Confirm the subsystem flips landed: `movement` remainder, `nav`, `weapon`, `ai` (`scripting/systems/ai.rs`), and the opaque-handle consumers (render/audio/netcode/agent_steering/collision) all depend on the floor crates with no reverse edge. Run the isolation `cargo tree` checks and the touch-rebuild timing checks from the AC. Verify the typedef drift test is byte-identical.

## Sequencing

**Phase 1 (sequential):** Task 1 — baseline gates the timing claims.
**Phase 2 (sequential):** Task 2 — the in-place cycle-breaking is the foundation; its sub-steps fan out internally but the phase gates the extractions.
**Phase 3 (sequential):** Task 3 — the substrate is the dependency floor; the data-model can't compile against it until it exists.
**Phase 4 (sequential):** Task 4 — the data-model crate depends on Task 3's substrate (and its `script-ffi` feature).
**Phase 5 (concurrent):** Task 5 — verification + the consumer-flip sweep fans out across subsystems once both crates exist.

Structural surgery (Phases 2–4) is serialized deliberately; the verification/flip fan-out is Phase 5.

## Rough sketch

**Crate layout (two layers, one-way edge).**
- `crates/sim-substrate/` (`postretro-sim-substrate`): IR evaluator core + movement/IR cluster + descriptor & value PODs + sunk subsystem PODs. Default = no VMs. `script-ffi = ["dep:rquickjs","dep:mlua"]` compiles its types' FFI impls.
- `crates/entity-core/` (`postretro-entity-core`): registry + `ComponentValue` + remaining components + `ScriptCtx` + registries + slimmed `DataRegistry`. Depends on `postretro-sim-substrate`. Own `script-ffi` feature for its types' FFI impls.

**Why layered, not one crate.** The `{IR core + MovementScope + PlayerMovementComponent + descriptor PODs}` cluster has no edge up into `EntityId`/`EntityRegistry`/`ComponentValue` (research.md §3) — a clean one-way edge. One crate would force the IR/movement substrate and the registry to recompile together for no structural reason; both grow under Epic 16 (combat adds descriptors and IR adoption; the registry grows components). Splitting lets each recompile independently.

**Orphan-rule, per crate.** Each crate owns the marshalling impls for the types it defines, behind its own optional `script-ffi` feature pulling the VMs — `impl rquickjs::FromJs for LocalType` is legal only where the type is local. Default builds have no VM deps; the runtime crate enables both features. Foreign types (glam) wrap in local newtypes (`Vec3Lit`/`EulerDegrees`). This is `scripting.md §12`'s contract, applied to two crates.

**What stays in `postretro` (the future runtime crate).** `ir/scopes.rs` (`StoreScope`), `conv.rs` FFI/json bridges, `data_descriptors/{js,lua}/` converters + the `render::ui` glob, `RegisteredUiTree`/`LevelManifest`, `runtime/*`, `luau.rs`/`quickjs.rs`/`primitives/*`/`primitives_registry.rs`/`reaction_dispatch.rs`/typedef generator.

**Relationship to `scripting-core-extraction`.** That spec's Task 2 (extract `entity-core`) is replaced by this plan. Its Task 3 (extract `scripting-core`) is reshaped to pull the VM-coupled remainder above and enable `script-ffi` on both floor crates. Its Tasks 4–6 (ScriptingCore sub-struct, test relocation, handler relocation) ride unchanged on top. After this lands, rebase that spec onto the floor before re-orchestrating it.

## Decisions

- **Two-layer floor**, not one crate — the cluster's one-way edge makes it clean and lets the substrate and registry recompile independently as both grow.
- **POD-sink inversion** for the `health→weapon` and `agent→nav` edges: sink the trivial PODs (`DamagePayload`, `NavAgentParams`) into the substrate; the behavior fns (`apply_damage`, `attach_agent`) stay with their component. Same for `ModMapEntry`.
- **`data_descriptors` splits** types (down) from VM converters + UI-manifest types (stay up). The `render::ui` CPU-only-model cleanup is explicitly deferred to `compile-time-reduction` Task 6.
- **`ir/scopes.rs` stays in the runtime crate** (pulls `ScriptCtx`/`slot_table`/`primitives`); only the VM-free IR core descends.

## Open questions

- Final crate names (`postretro-sim-substrate` / `postretro-entity-core` are working names). Decide at implementation; the lower crate is wider than movement or IR alone.
