# Scripting Boundary Hardening

## Goal

Make the post-extraction `crate::scripting::*` surface a clean exemplar for future script-heavy work — chiefly the Epic 16 (Combat) fan-out, which templates off neighboring bridge/handler files far more reliably than it obeys a prose import rule. Delete the dead compatibility barrels `scripting-core-extraction` left behind, collapse the live re-export barrels (including the high-traffic floor barrels the extraction deferred) so the most-copied files name their owning crate directly, then lock the result with a static import check.

This is cleanup after `scripting-core-extraction` (`context/plans/in-progress/scripting-core-extraction/`). The build-time firewall (one-way crate edge `foundation → entities → scripting-core → postretro`, measured ~1.70s warm rebuild) already landed. This plan makes **no** build-speed, binary-size, or VM-removal claim — `postretro` still depends on `mlua`/`rquickjs`, and nothing here changes that.

## Scope

### In scope

- Delete the compatibility barrels in `crates/postretro/src/scripting/` that have zero call sites.
- Collapse every live single-target re-export barrel **of a floor-/core-owned API** by rewriting its call sites to direct `postretro_scripting_core::*` / `postretro_foundation::*` / `postretro_entities::*` imports, then delete the barrel — **including** the high-traffic floor barrels (`registry`, `components`, `ctx`) that the extraction kept to bound structural-surgery blast radius.
- Keep the `gen_script_types` bin's parallel `#[path]`-mounted `scripting` tree (`crates/postretro/src/bin/gen_script_types.rs`) in sync as the registrar files it re-includes lose their barrel imports.
- Add one static boundary check (a content scan with a small path allowlist) that fails on new `crate::scripting::<removed-barrel>` references to floor-/core-owned APIs.

### Out of scope

- Removing `mlua`/`rquickjs` or any VM dependency from `postretro`; cold-build, warm-rebuild, or binary-size work.
- Changing primitive names, SDK typedef output, wire shapes, marshalling results, or `register_all` registration order. Collapsing is import-only.
- Relocating tests. The substrate test move already shipped (~445 `#[test]`s live in `crates/scripting-core/src`); see the audit note under Tasks.
- Touching `Session` construction, the `ScriptingCore` sub-struct, or any subsystem crate-ification.
- Updating `context/lib/` (happens at promotion).

## Barrel inventory

Verified against source (`rg "scripting::<barrel>\b"` over `crates/postretro/src`, excluding `scripting/mod.rs`). "Sites" is occurrence count; `error`/`engine_state_catalog`/`luau_prelude` are reached only from the `typedef/tests` drift fixtures. Counts are an as-of-HEAD snapshot; grouped imports make exact tallies methodology-sensitive, so re-run the sweep at implementation time.

| Barrel | Owner | Collapse target | Sites | Disposition |
|---|---|---|---|---|
| `data_registry`, `foundation_pods`, `game_state_refs`, `ir`, `ir_scopes`, `luau_require`, `luau_virtual_modules`, `refresh_plan`, `luau`, `quickjs`, `watcher`, `value_types` | mixed | — | 0 | **delete (dead)** |
| `registry` | entities | `postretro_entities::registry` | 58 | collapse |
| `components` | entities | `postretro_entities::components` | 60 | collapse |
| `ctx` | entities | `postretro_entities::ctx` | 22 | collapse |
| `slot_table` | entities | `postretro_entities::slot_table` | 10 | collapse |
| `provenance` | entities | `postretro_entities::provenance` | 3 | collapse |
| `error` | entities | `postretro_entities::scripting::error` | 1 | collapse |
| `engine_state_catalog` | entities | `postretro_entities::engine_state_catalog` | 1 | collapse |
| `conv` | core | `postretro_scripting_core::conv` | 3 | collapse |
| `reaction_dispatch` | core | `postretro_scripting_core::reaction_dispatch` | 10 | collapse |
| `runtime` | core | `postretro_scripting_core::runtime` | 11 | collapse |
| `sequence` | core | `postretro_scripting_core::sequence` | 1 | collapse |
| `staged_manifest` | core | `postretro_scripting_core::staged_manifest` | 6 | collapse |
| `state_crossings` | core | `postretro_scripting_core::state_crossings` | 2 | collapse |
| `luau_prelude` | core | `postretro_scripting_core::luau_prelude` | 1 | collapse |
| `primitives_registry` | core | `postretro_scripting_core::primitives_registry` | 8 | collapse |
| `data_descriptors` | core (aggregation) | `postretro_scripting_core::data_descriptors` | 24 | collapse |

`typedef` is **not** a barrel — it is a compat module (`scripting/typedef/mod.rs`) that hosts the postretro-resident drift tests (`typedef/tests/`, on the legal `postretro → scripting-core` down-edge) plus `common`/`luau`/`ts` submodule shims. It stays. Its non-test re-export callers (`session/mod.rs`) and its internal `primitives_registry` reference collapse to direct `postretro_scripting_core::*`.

`builtins`, `entity_world_primitives`, `map_entity`, `primitives`, `reactions`, `state_persistence`, `state_store`, and `systems` (mounted at crate root as `scripting_systems` via `#[path]`) are real postretro modules, unaffected.

## Acceptance criteria

- [ ] The 12 dead barrels above no longer exist under `crates/postretro/src/scripting/`; their `gen_script_types` bin twins (`luau`, `quickjs`, `value_types`) are gone too.
- [ ] No `crate::scripting::<X>` reference remains in `crates/postretro/src` for any collapsed or deleted barrel `X` — outside the static-check module and the `scripting/typedef` allowlist. The compiler enforces this for deleted barrels; the static check enforces it against re-introduction.
- [ ] Collapsed call sites import their owning crate directly. Derive each barrel's target from its re-export — the barrel is `pub(crate) use <owner>::<barrel>::*;` (inline in `scripting/mod.rs`, or in its `*.rs`/dir file); import that exact owner path. Non-obvious targets: `error` → `postretro_entities::scripting::error` (nested, not `postretro_entities::error`); `ctx`/`components`/`registry`/`slot_table`/`provenance`/`engine_state_catalog` → `postretro_entities::*` even though `postretro_scripting_core` also re-exports some of them; all other core-owned barrels → `postretro_scripting_core::<barrel>`.
- [ ] `data_descriptors` is collapsed to `postretro_scripting_core::data_descriptors` — the aggregation that re-exports floor descriptor types and core converters (no per-type split to `postretro_foundation`/`postretro_entities`).
- [ ] `gen_script_types` builds: its `#[path]`-mounted registrar files (`primitives/*`, `state_store.rs`, `entity_world_primitives.rs`) resolve their imports directly (compiler/clippy-gated).
- [ ] (Review gate) The bin's inline `mod scripting` block carries no dead `pub(crate) use` re-exports. Not compiler-checkable — the block is `#![allow(dead_code, unused_imports)]`; Task 4's post-trim assertion is the check.
- [ ] A static boundary check fails when a non-allowlisted file under `crates/postretro/src` references `crate::scripting::<removed-barrel>` for a floor-/core-owned API. Allowlist is the check's own module plus `scripting/typedef`.
- [ ] `register_all`'s call sequence in `primitives/mod.rs` is byte-for-byte unchanged (`register_shared_types` → light shared types → light → store → world → entity).
- [ ] `cargo test -p postretro-scripting-core` passes.
- [ ] `cargo test -p postretro scripting` passes.
- [ ] The SDK typedef byte-identity drift test (`scripting/typedef/tests/committed.rs`) passes unchanged — generated `postretro.d.ts` / `postretro.d.luau` byte-identical to pre-collapse.
- [ ] No `unsafe` added.

## Tasks

> **Audit note (no task).** Test relocation is already done. ~445 substrate `#[test]`s live in `crates/scripting-core/src`. The three files an earlier draft targeted hold registrar/VM-bound tests that correctly stay in `postretro`: `state_store.rs` (31 tests, calls `register_*` + builds `mlua::Lua`/`rquickjs`), `entity_world_primitives.rs` (6 tests, registrar + VM), `primitives/mod.rs` (7 tests, registrar + VM). Nothing moves.

### Task 1: Inventory and delete dead barrels

Confirm the inventory table against source, then delete the 12 zero-caller barrels. For the inline barrels (`game_state_refs`, `ir_scopes`, `luau_require`, `luau_virtual_modules`, `refresh_plan`, `luau`, `quickjs`, `watcher`) remove their `pub(crate) mod` blocks from `scripting/mod.rs`; for the file/dir barrels (`data_registry.rs`, `foundation_pods.rs`, `ir/`, `value_types.rs`) delete the file/dir and its `mod` declaration. Delete the matching inline twins (`luau`, `quickjs`, `value_types`) from the `gen_script_types` bin's `mod scripting { … }` block. This is free hygiene: per the project's "no compat re-exports" norm, an unused alias is deleted, which lets the compiler enforce non-use. The fan-out tasks re-derive their barrel set from source; no cross-task table is handed off.

### Task 2: Collapse the systems bridges

Rewrite every `crate::scripting::<barrel>` reference in `crates/postretro/src/scripting/systems/*` to its direct collapse target. These files (the FX/render bridges — `*_bridge.rs`, `hit_zones.rs`, `mesh_*.rs`, `particle_*.rs`, the decay systems, `ui_proxy.rs`, `ai*.rs`, `health.rs`) carry the bulk of `registry`/`components`/`ctx`/`slot_table`/`conv`/`state_crossings`/`data_descriptors` usage and are exactly what an Epic 16 agent opens. Sweep the whole `systems/` directory — the parenthetical is illustrative, not exhaustive (e.g. `input_mode.rs`, `presentation_cells.rs` also carry refs). Import-only; do not touch behavior. Do not delete barrels (shared with other tasks). Resolve each barrel's collapse target per AC-3 (read its `pub(crate) use` re-export).

### Task 3: Collapse the builtins

Rewrite barrel references in `crates/postretro/src/scripting/builtins/*` (`data_archetype*.rs`, `net_descriptor.rs`, `billboard_emitter.rs`, `prop_mesh.rs`, `mod.rs`) to direct imports — `registry`, `components`, `conv`, `provenance`, `data_descriptors`. Import-only. Resolve each barrel's collapse target per AC-3 (read its `pub(crate) use` re-export).

### Task 4: Collapse the primitive registrars and sync the bin

Rewrite barrel references in `crates/postretro/src/scripting/primitives/mod.rs` (9 refs: `ctx`×2, `data_descriptors`×2, `primitives_registry`×2, `registry`, `runtime`, `slot_table`). `state_store.rs` and `entity_world_primitives.rs` already import `postretro_entities::*`/`postretro_scripting_core::*` directly (zero `crate::scripting::<barrel>` refs); they are in Task 4's scope only to confirm the `#[path]` bin mount still compiles after the bin trim. These files are `#[path]`-mounted into `gen_script_types` and compiled in both the lib and the bin; direct imports resolve in both (both depend on the floor crates). Then trim the now-unused inline barrels from the bin's `mod scripting { … }` block and point its `use scripting::ctx::ScriptCtx` / `register_all` lines at the surviving paths. Task 4 is the sole owner of the bin's inline-barrel removal. After the trim, assert the bin's `mod scripting` block declares only its three `#[path]`-mounted submodules (`primitives`, `state_store`, `entity_world_primitives`) plus `use scripting::primitives::register_all` and `use postretro_entities::ctx::ScriptCtx` — no surviving `pub(crate) use` re-exports (the block's `#![allow(dead_code, unused_imports)]` hides dead aliases otherwise). Repoint the bin's surviving `use scripting::ctx::ScriptCtx` to `postretro_entities::ctx::ScriptCtx` (the inventory's owning crate, matching the already-collapsed `state_store.rs`), not `postretro_scripting_core`. **Do not alter `register_all`'s call sequence** — the collapse changes imports only.

### Task 5: Collapse the top-level engine call sites

Rewrite barrel references in the engine code outside the scripting tree: `session/mod.rs`, `startup/*`, `main.rs`, `sim/*`, `render/ui/*` (including the `#[cfg(test)]` `lifecycle_render_test.rs`, which imports `data_descriptors`/`primitives_registry`/`runtime`/`staged_manifest`), `scripting/map_entity.rs`, `scripting/state_persistence.rs`. Covers `reaction_dispatch`, `runtime`, `sequence`, `staged_manifest`, `primitives_registry`, `state_crossings`, `slot_table`, `conv`, `data_descriptors`, and the non-test `typedef` callers (→ `postretro_scripting_core::typedef`). Resolve each barrel's collapse target per AC-3 (read its `pub(crate) use` re-export). Several refs use the `scripting` self-alias (`use crate::scripting::{self, reaction_dispatch}` then `scripting::runtime::…`/`scripting::sequence::…`/`scripting::state_crossings::…`, e.g. `main.rs`, `startup/lifecycle.rs`, `scripting/state_persistence.rs`) or a relative `use super::slot_table::{…}` — rewrite these too; a literal `crate::scripting::<barrel>` grep misses them. Split the grouped import and keep the `scripting` alias only where it still serves real-module paths (`scripting::reactions::*`, `scripting::builtins::*`).

### Task 6: Collapse the typedef compat module and drift fixtures

Rewrite the floor-/core-barrel references inside `scripting/typedef/mod.rs` (its `primitives_registry` import) and the `typedef/tests/*` fixtures (`ctx`, `registry`, `primitives_registry`, `error`, `engine_state_catalog`, `luau_prelude`) to direct imports. Targets for the trap barrels: `error` → `postretro_entities::scripting::error`; `engine_state_catalog` → `postretro_entities::engine_state_catalog`; `luau_prelude` → `postretro_scripting_core::luau_prelude`. After this the fixtures reference only `crate::scripting::typedef` (the retained module) — the down-edge that lets them call `register_all` + the generator (`typedef/tests/snapshots.rs` keeps a `postretro::scripting::error::ScriptError` string literal as intentional converter-test input — not an import, harmless, and the `typedef/` dir is allowlisted). The drift test must still pass byte-identically. Resolve each barrel's collapse target per AC-3 (read its `pub(crate) use` re-export).

### Task 7: Tear down collapsed barrels

Once Tasks 2–6 land, delete every collapsed barrel from `scripting/mod.rs` (keep `typedef` — the compat module on the `postretro → scripting-core` down-edge) and remove the file barrels (`ctx.rs`, `registry.rs`, `components/mod.rs`, `slot_table.rs`, `provenance.rs`, `error.rs`, `engine_state_catalog.rs`, `state_crossings.rs`, `luau_prelude.rs`). Then assert zero remaining `crate::scripting::<removed-barrel>` references survive (a `cargo check -p postretro --tests` (compiles `#[cfg(test)]` modules so a test-only straggler surfaces) plus a grep sweep). Any straggler is collapsed here, not allowlisted. Also remove the orphaned empty `scripting/data_descriptors/{js,lua,tests,types,validate}` leftover dirs (extraction residue; git tracks nothing under them).

### Task 8: Static boundary check (capstone)

Add a `#[cfg(test)]` content-scan check — a sibling to the existing `extraction_path_tests` in `scripting/mod.rs`, but a different mechanism: `extraction_path_tests` is a filesystem path-existence walk; this is a **new, larger** apparatus that reads Rust source text and pattern-matches imports. It walks `crates/postretro/src`, and for each non-allowlisted file fails on any `crate::scripting::<name>` where `<name>` is one of the removed/collapsed barrels: the 12 dead (`data_registry`, `foundation_pods`, `game_state_refs`, `ir`, `ir_scopes`, `luau_require`, `luau_virtual_modules`, `refresh_plan`, `luau`, `quickjs`, `watcher`, `value_types`), the 16 collapsed (`registry`, `components`, `ctx`, `slot_table`, `provenance`, `error`, `engine_state_catalog`, `conv`, `reaction_dispatch`, `runtime`, `sequence`, `staged_manifest`, `state_crossings`, `luau_prelude`, `primitives_registry`, `data_descriptors`), plus `typedef` (forbidden outside the `scripting/typedef` allowlist, to push new code to `postretro_scripting_core::typedef`). Allowlist: the check's own module and `scripting/typedef` (the down-edge drift-test host). Keep the failure message pointed at the canonical rule: import floor/core APIs from `postretro_foundation`/`postretro_entities`/`postretro_scripting_core` directly.

The check earns its keep as a re-introduction lock and executable documentation for the Epic 16 fan-out: the compiler already rejects importing a deleted barrel, but it does **not** stop an agent from re-adding a barrel and threading it in one diff — this check flags the import. **No** AC-6-style "scripting-core does not import `crate::scripting::*`" test: `crates/scripting-core/src` has no `scripting` module, so such an import cannot compile (verified: 0 occurrences). It is a compiler-guaranteed invariant, not a test. State the limitation in the failure message / a code comment: the literal `crate::scripting::<name>` scan does not catch the `scripting`-alias or `super::`-relative forms (post-deletion those are compile errors, so this is a precedent nudge, not a complete guard).

### Task 9: Verification gate

Run `cargo test -p postretro-scripting-core`, `cargo test -p postretro scripting`, and confirm `scripting/typedef/tests/committed.rs` passes byte-identically. Run `cargo clippy -p postretro --all-targets -- -D warnings` to catch any unused-import fallout from the collapse (`--all-targets` so `#[cfg(test)]` modules, where much of the collapse lands, are linted).

## Sequencing

**Phase 1 (sequential):** Task 1 — inventory gates the fan-out's disposition table; dead-barrel deletion is independent low-risk hygiene.
**Phase 2 (concurrent):** Tasks 2, 3, 4, 5, 6 — collapse fan-out, partitioned by file group so every file is rewritten by exactly one agent. (Partition by file, not by barrel: `registry`/`components`/`ctx` co-occur in the same bridge files, so a per-barrel split would collide.) Run in isolated worktrees.
**Phase 3 (sequential):** Task 7 — barrel teardown consumes all collapse output; it edits the shared `scripting/mod.rs`, so it serializes after the fan-out.
**Phase 4 (sequential):** Task 8 — the static check lands *after* the collapse. It could not pass against `registry`/`components`/`ctx` while those barrels were still threaded through the bridges without an allowlist so large it guts the check; once collapsed, the allowlist is tiny and the check has teeth.
**Phase 5 (sequential):** Task 9 — final verification.

## Rough sketch

Collapse is mechanical path surgery: each barrel file is `pub(crate) use <owner>::<barrel>::*;`, so `crate::scripting::registry::EntityId` → `postretro_entities::registry::EntityId`, etc. The floor crates already export these paths (the extraction flipped movement/nav/weapon/netcode the same way — e.g. `weapon/mod.rs` already imports `postretro_entities::registry::{EntityId, EntityRegistry}`).

The byte-identity typedef drift test is the safety net: import-path rewrites resolve to identical types, so generated `.d.ts`/`.d.luau` cannot drift. `register_all`'s call order in `primitives/mod.rs` is the one ordering invariant — left untouched because the collapse never edits the registration block.

Two parallel mount points complicate the file sweep, both `#[path]`-based and both on-disk under `crates/postretro/src/scripting/`: the `gen_script_types` bin re-roots a curated partial `scripting` tree (with its own inline barrels) so it can populate a registry without dragging wgpu, and `main.rs` mounts `scripting/systems/mod.rs` as `crate::scripting_systems` for the same reason. The static check scans by on-disk path, so it covers both.

## Decisions

- **Reframed around precedent, not regression-guarding a documented rule.** The extraction already documents the import rule and the kept-barrel list. The durable value here is making the files agents *copy* clean — bridge/handler files in `systems/*`, `builtins/*`, `primitives/*` — because the Epic 16 multi-agent fan-out templates off neighbors more reliably than it reads prose.
- **The collapse is expanded to the high-traffic floor barrels** (`registry`, `components`, `ctx`). The extraction deferred these only to bound structural-surgery blast radius; that reason is gone, and these are precisely the most-copied bridge files. The work is bounded, mechanical, import-only.
- **`data_descriptors` is collapsed, not kept.** Source contradicts the "split-ownership" premise: the postretro `data_descriptors/` directory is empty leftover dirs, and `crate::scripting::data_descriptors` is a pure re-export of `postretro_scripting_core::data_descriptors`. That scripting-core module is the deliberate aggregation that re-exports floor descriptor types *and* the core converters, so all 24 sites collapse to one target mechanically. Future agents import the scripting-core aggregation directly, not each type's originating floor module.
- **Test relocation is dropped — already done.** ~445 substrate tests live in `scripting-core`; the three candidate files hold registrar/VM-bound tests that stay in `postretro`.
- **The static check is a capstone gated on the collapse, and is slimmed.** This inverts the prior "check before cleanup" sequencing: the check cannot have teeth while the floor barrels are still threaded through the bridges. The AC-6-style "scripting-core does not import `crate::scripting`" test is dropped as compiler-redundant (no `scripting` module exists in `scripting-core` to resolve against). Honestly, the content-scan-with-allowlist is a new, larger apparatus than the existing path-existence walk, and for already-deleted barrels it duplicates a compiler error — its residual value is blocking barrel re-introduction and documenting the rule in executable form.
- **`typedef` stays as a compat module** because it hosts the byte-identity drift tests that must live in `postretro` on the `postretro → scripting-core` down-edge; it is allowlisted in the static check.
