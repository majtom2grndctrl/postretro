# Scripting Boundary Hardening

## Goal

Harden the post-extraction scripting boundary so new engine code uses the owning crates directly. Collapse low-value compatibility barrels, add static import checks, and finish moving substrate-only tests into `postretro-scripting-core`.

This is cleanup after `scripting-core-extraction`. It does not remove VM dependencies from the `postretro` binary or claim binary-size wins.

## Scope

### In scope

- Inventory `crates/postretro/src/scripting` compatibility barrels and remove the ones whose callers are low-churn and have clear owning-crate imports.
- Keep compatibility barrels only where they still protect broad call-site churn, legacy test paths, or intentional compatibility wrappers.
- Add source-scanning tests that reject new engine imports of core-owned scripting APIs through `crate::scripting::*` when `postretro_scripting_core::*` is the owner.
- Add source-scanning tests that reject new engine imports of floor-owned scripting APIs through `crate::scripting::*` when `postretro_foundation::*` or `postretro_entities::*` is the owner.
- Audit remaining substrate-only tests under `crates/postretro/src/scripting` and move them into `crates/scripting-core/src` when they do not depend on a postretro-owned registrar or subsystem.
- Keep registrar-dependent and subsystem tests in `postretro`.

### Out of scope

- Removing `mlua`, `rquickjs`, or any VM dependency from the `postretro` binary.
- Cold-build reduction, release binary slimming, or timing gates.
- Large subsystem crate-ification.
- Changing primitive semantics, SDK types, wire formats, generated typedef order, registration order, or scripting runtime behavior.
- Moving registrar-dependent tests out of `postretro`.
- Updating `context/lib`.

## Acceptance criteria

- [ ] `crates/postretro/src/scripting` has an explicit compatibility-barrel inventory in code comments or test allowlist data. Every remaining barrel is categorized as core-owned, floor-owned, primitive compatibility, reaction compatibility, or test-only.
- [ ] Low-churn barrels are collapsed. Their call sites import directly from `postretro_scripting_core`, `postretro_foundation`, or `postretro_entities`.
- [ ] Remaining compatibility barrels are allowlisted by path and reason. The allowlist includes only compatibility modules and test modules that intentionally exercise legacy paths.
- [ ] A static boundary test fails on a new non-allowlisted engine import from core-owned compatibility paths such as `crate::scripting::conv`, `crate::scripting::primitives_registry`, `crate::scripting::reaction_dispatch`, `crate::scripting::runtime`, `crate::scripting::sequence`, `crate::scripting::staged_manifest`, or `crate::scripting::watcher`.
- [ ] A static boundary test fails on a new non-allowlisted engine import from floor-owned compatibility paths such as `crate::scripting::ctx`, `crate::scripting::registry`, `crate::scripting::slot_table`, `crate::scripting::data_registry`, `crate::scripting::engine_state_catalog`, `crate::scripting::components`, `crate::scripting::ir`, `crate::scripting::foundation_pods`, `crate::scripting::value_types`, or `crate::scripting::provenance`.
- [ ] `crates/scripting-core/src` does not import from `crate::scripting::*`.
- [ ] Substrate-only tests that use only `postretro-scripting-core` APIs, raw `rquickjs`/`mlua` contexts, or floor crates live in `crates/scripting-core/src`.
- [ ] Tests that call `register_all`, `register_store_primitives`, `register_world_primitives`, `register_entity_primitives`, `register_light_entity_primitives`, reaction registrars, or subsystem handlers remain in `postretro`.
- [ ] SDK typedef drift tests under `crates/postretro/src/scripting/typedef/tests` stay in `postretro` and still pass. They depend on `register_all`.
- [ ] No primitive names, SDK output, wire shapes, registration order, or reaction dispatch behavior changes.
- [ ] `cargo test -p postretro-scripting-core` passes.
- [ ] `cargo test -p postretro scripting` passes.
- [ ] No `unsafe` added.

## Tasks

### Task 1: Inventory current compatibility barrels

Build a current inventory from source, not from the extraction plan. Start with `crates/postretro/src/scripting/mod.rs`, the thin file barrels (`ctx.rs`, `registry.rs`, `slot_table.rs`, `data_registry.rs`, `error.rs`, `engine_state_catalog.rs`, `foundation_pods.rs`, `value_types.rs`, `provenance.rs`, `components/mod.rs`, `ir/mod.rs`, `state_crossings.rs`, `luau_prelude.rs`), primitive barrels under `crates/postretro/src/scripting/primitives/`, reaction barrels under `crates/postretro/src/scripting/reactions/`, and `crates/postretro/src/scripting/typedef/mod.rs`.

Classify each path by owner:

- core-owned: re-exporting `postretro_scripting_core`;
- floor-owned: re-exporting `postretro_foundation` or `postretro_entities`;
- postretro-owned compatibility: re-exporting relocated postretro handler modules;
- test-only helper.

For each path, decide collapse or keep. Collapse when the caller count is small, the target owner is obvious, and the rewrite does not touch unrelated behavior. Keep when it prevents broad churn or preserves a deliberate legacy test path.

### Task 2: Add static boundary checks

Add source-scanning tests in a small test-only module, not in large handler files. The existing `extraction_path_tests` in `crates/postretro/src/scripting/mod.rs` already proves moved implementation paths do not reappear; either extend that pattern or add a sibling `#[cfg(test)]` module under `crates/postretro/src/scripting`.

The check scans Rust source under `crates/postretro/src` and reports non-allowlisted `crate::scripting::<barrel>` imports/usages for core-owned and floor-owned APIs. It must support path allowlists with short reasons. Allow compatibility barrel files themselves, `crates/postretro/src/scripting/typedef/tests`, and other tests that intentionally validate legacy compatibility. Do not allow ordinary engine modules to add new barrel imports for core-owned or floor-owned types.

Also add a `postretro-scripting-core` check that rejects `crate::scripting::` in `crates/scripting-core/src`. That crate should import floor types through `postretro_foundation` and `postretro_entities`, or local substrate modules through `crate::*`.

### Task 3: Collapse low-churn barrels and direct imports

Apply the inventory. Rewrite direct call sites before deleting a barrel. Prefer crate-root imports where the owner exports the symbol:

- `postretro_scripting_core::{conv, primitives_registry, reaction_dispatch, reaction_registry, runtime, sequence, staged_manifest, state_crossings, typedef, watcher}` for core-owned APIs;
- `postretro_entities::{ScriptCtx, EntityRegistry, EntityId, ComponentKind, ComponentValue, Transform, DataRegistry, SlotValue, ...}` for entity floor APIs;
- `postretro_foundation::{Vec3Lit, EulerDegrees, DamagePayload, ModMapEntry, NavAgentParams}` for foundation floor APIs.

Keep `crates/postretro/src/scripting/primitives::{entity,light,store,world}.rs` only if they still provide useful compatibility for registrar-dependent tests or broad internal paths. Keep reaction compatibility modules only where the legacy paths still have meaningful test or subsystem callers. Delete barrels whose remaining purpose is only to hide a direct owner import.

Preserve `register_all` order in `crates/postretro/src/scripting/primitives/mod.rs`: shared types, light shared types, light primitives, store primitives, world primitives, entity primitives.

### Task 4: Move substrate-only tests to scripting-core

Audit `crates/postretro/src/scripting/state_store.rs`, `crates/postretro/src/scripting/entity_world_primitives.rs`, `crates/postretro/src/scripting/primitives/mod.rs`, and the compatibility wrappers for tests that no longer need postretro-owned registrars or subsystem state.

Move tests when their referents live fully in `postretro-scripting-core`, such as VM conversion, primitive adapter marshalling, store bridge helpers, typedef generator helpers that do not call `register_all`, and raw QuickJS/Luau substrate behavior.

Keep tests in `postretro` when they call postretro registrars or engine handlers. This includes tests using `register_all`, `register_store_primitives`, `register_world_primitives`, `register_entity_primitives`, `register_light_entity_primitives`, `register_emitter_reaction_primitives`, `register_fog_reaction_primitives`, `register_system_reaction_primitives`, or subsystem dispatch functions.

`crates/postretro/src/scripting/state_store.rs` is already large. Prefer moving substrate-only tests out instead of adding new test infrastructure there.

### Task 5: Verification and cleanup

Run focused checks:

- `cargo test -p postretro-scripting-core`
- `cargo test -p postretro scripting`

If the static checks reveal a compatibility path still used by broad engine code, either direct-import it in the same task or add an allowlist reason that says why it remains. Do not leave a path unclassified.

## Sequencing

**Phase 1 (sequential):** Task 1 — inventory gates the allowlist and collapse decisions.

**Phase 2 (sequential):** Task 2 — static checks land before import cleanup so they define the target boundary.

**Phase 3 (concurrent):** Task 3, Task 4 — direct-import cleanup and test relocation can proceed independently after the allowlist exists. Coordinate if both touch the same test module.

**Phase 4 (sequential):** Task 5 — final verification consumes the import and test moves.

## Rough sketch

Current source confirms `postretro-scripting-core` exists at `crates/scripting-core`, exports VM substrate modules from `crates/scripting-core/src/lib.rs`, and denies unsafe code. `postretro` still has compatibility barrels in `crates/postretro/src/scripting/mod.rs` and thin files under `crates/postretro/src/scripting`.

`Session` already has a `ScriptingCore` sub-struct in `crates/postretro/src/session/mod.rs`; this follow-up should not restructure session construction.

Registrar-dependent typedef tests currently live under `crates/postretro/src/scripting/typedef/tests` and call `register_all`. Keep them there. `crates/postretro/src/scripting/typedef/mod.rs` is a compatibility module over `postretro_scripting_core::typedef`.

Handler relocation has already happened for several families: light logic in `crates/postretro/src/lighting/script_primitives.rs`, store primitive registration in `crates/postretro/src/scripting/state_store.rs`, entity/world primitive registration in `crates/postretro/src/scripting/entity_world_primitives.rs`, emitter/fog reactions under `crates/postretro/src/fx`, health reactions under `crates/postretro/src/health/reactions.rs`, mesh animation reactions under `crates/postretro/src/model/animation_reactions.rs`, and system reactions under `crates/postretro/src/scripting/systems/system_reactions.rs`.

Large-file note: `crates/postretro/src/scripting/state_store.rs` and `crates/postretro/src/lighting/script_primitives.rs` are over the normal split threshold. This plan should mostly reduce or rewrite imports in them. New static-check infrastructure belongs in a small test module.

## Open questions

None.
