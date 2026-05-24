# Descriptor Hot Reload

## Goal

Support debug hot reload for all authored entity descriptors without treating every script edit as a mod-init edit. Preserve the scripting model: scripts declare, Rust executes, and no live mod-init VM survives gameplay.

## Scope

### In scope

- Track which source files can affect the mod-init manifest.
- Re-run mod init only when a changed file is part of the active mod-init dependency set.
- Rebuild the TypeScript start-script bundle when any bundled dependency changes.
- Track Luau files loaded through mod-init `require`.
- Upsert refreshed descriptors into `DataRegistry`.
- Apply explicit live refresh policies for descriptor-authored runtime components.
- Keep the current debug-only watcher and release no-op behavior.

### Out of scope

- Runtime mod hot-swap.
- Live script callbacks or persistent VMs.
- Dependency-aware reload for level data scripts.
- Auto-discovery of domain scripts not imported or required by the start script.
- Re-spawning map placements from changed descriptors.
- Replacing the TypeScript sidecar architecture.

## Acceptance Criteria

- [ ] Editing a TypeScript file imported by `start-script.ts` recompiles `start-script.js`, reruns mod init, updates `DataRegistry`, and refreshes all supported live descriptor-authored components.
- [ ] Editing an unrelated `.ts` file under `<mod>/scripts/` does not rerun mod init.
- [ ] Editing a Luau file required by `start-script.luau` reruns mod init and refreshes all supported live descriptor-authored components.
- [ ] Editing an unrelated `.luau` file under `<mod>/scripts/` does not rerun mod init.
- [ ] A failed compile or failed mod-init rerun leaves the previous descriptor registry and live component state active.
- [ ] Removing a descriptor-owned component from a descriptor does not silently leave the stale live component under the old descriptor policy.
- [ ] Watcher edit and atomic-rename tests still pass for TypeScript and Luau.
- [ ] `cargo test -p postretro` passes.

## Tasks

### Task 1: Dependency Manifest

Record the dependency set produced by a successful mod-init run. The runtime needs a stable answer to: "does this changed source path affect the active mod-init manifest?" Store TypeScript dependency paths from the bundler and Luau dependency paths from the `require` resolver. Replace broad `Scripts` reload policy with dependency-set membership.

### Task 2: TypeScript Dependency Reporting

Extend `scripts-build` so bundling can report every real file loaded for an entry. The CLI should keep writing the bundled JavaScript as it does today. Add an engine-facing path that captures dependency paths without linking SWC into the runtime crate. The watcher must classify changed TypeScript files against the last successful start-script dependency set.

### Task 3: Luau Require Tracking

Teach the mod-init Luau path to collect every file resolved by `require`. The resolver already owns path resolution and script execution. Thread a dependency sink through that path during mod init, and keep data-script `require` out of the mod-init dependency set.

### Task 4: Descriptor Refresh Engine

Move descriptor upsert and live refresh into one reload operation. On success, apply descriptor changes to existing descriptor-authored entities according to per-component policy. On failure, leave the previous registry and live components unchanged.

### Task 5: Tests And Diagnostics

Add regression tests for dependency-aware reload classification, successful descriptor refresh, unrelated-script no-op reloads, failed compile preservation, and descriptor component removal. Log enough debug detail to explain why a changed file did or did not trigger mod init.

## Sequencing

**Phase 1 (sequential):** Task 1 — defines the runtime data contract used by every later task.
**Phase 2 (concurrent):** Task 2, Task 3 — independent dependency producers for the same manifest.
**Phase 3 (sequential):** Task 4 — consumes the dependency manifest and descriptor refresh contract.
**Phase 4 (sequential):** Task 5 — verifies the full reload path and failure behavior.

## Rough Sketch

Current relevant entry points:

- `ScriptWatcher::spawn` watches `<mod>/scripts/` recursively and the mod root non-recursively.
- `ReloadKind` currently distinguishes `Scripts` from `ModInit`, but the frame loop treats both as mod-init reloads.
- `ScriptRuntime::run_mod_init` runs `start-script.js` or `start-script.luau`.
- `compile_start_script` always rebuilds `start-script.js` from `start-script.ts` in debug builds.
- `scripts-build` bundles TypeScript through `bundle_entry`.
- Luau `require` is installed by `install_require_resolver`.
- `DataRegistry::upsert_entity_type` updates descriptor storage keyed by `canonical_name`.
- `App::refresh_active_wieldable_from_descriptors` is the only live descriptor refresh hook today.

Proposed shape:

1. Add a small dependency-manifest type near `ScriptRuntime`.
2. Store it only after a successful mod-init run.
3. For TypeScript, have the sidecar emit dependency paths in a machine-readable form, or add a narrow sidecar mode that reports dependencies while still writing the bundle.
4. For Luau, collect each resolved `require` path during mod init.
5. Let the watcher enqueue changed source paths, not only broad reload kinds.
6. Let the frame-loop policy ask the runtime whether the changed path belongs to the active mod-init dependency set.
7. Run mod init into a temporary manifest first. Commit `DataRegistry` and live refresh only after validation succeeds.

Live refresh policies should be explicit:

| Descriptor component | Current materialization | Refresh policy |
|---|---|---|
| `weapon` | `WeaponComponent::from_descriptor` | Update authored stats. Preserve cooldown and trigger-edge state. Remove stale weapon component only for descriptor-owned wieldables. |
| `movement` | `PlayerMovementComponent::from_descriptor` | Update authored physics and capsule params. Preserve velocity, grounded state, air-jump state, and air-tick state unless the descriptor no longer declares movement. |
| `light` | `LightComponent` built from `LightDescriptor` | Update authored intensity, color, and range for descriptor-spawned dynamic lights. Preserve runtime animation state where compatible. Keep map-authored PRL lights out of descriptor refresh. |
| `emitter` | `BillboardEmitterComponent` cloned from descriptor with `initial_*` map overrides | Update descriptor-authored fields while preserving current burst/animation state where compatible. Preserve map `initial_*` overrides. |
| `defaultWeapon` | Player-start equip target | A changed default affects future spawns only. Do not switch the active weapon mid-level unless the active descriptor itself changed. |

Descriptor-spawned entities need provenance. Add a way to tell which live entities came from which descriptor and which component values were map-overridden at spawn. Without that, refresh cannot safely distinguish descriptor-authored state from runtime mutation or map initial state.

## Boundary Inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Entity descriptor | `EntityTypeDescriptor` | `canonicalName`, `defaultWeapon`, `components` | `EntityTypeDescriptor` | table with same keys | `classname` matches `canonicalName` |
| Weapon descriptor | `WeaponDescriptor` | `weapon` under `components` | `WeaponDescriptor` | table with same keys | n/a |
| Movement descriptor | `PlayerMovementDescriptor` | `movement` under `components` | `PlayerMovementDescriptor` | table with same keys | n/a |
| Light descriptor | `LightDescriptor` | `light` under `components` | `LightDescriptor` | table with same keys | `initial_*` overrides on placements |
| Emitter descriptor | `BillboardEmitterComponent` | `emitter` under `components` | `BillboardEmitterComponent` | table with same keys | `initial_*` overrides on placements |
| Mod manifest | `ModManifestResult` | `name`, `entities` | `setupMod()` return | `setupMod()` return table | n/a |
| Dependency manifest | new runtime-owned type | internal only | n/a | n/a | n/a |

## Open Questions

- Should the TypeScript sidecar write dependency metadata beside `start-script.js`, or should it print JSON to stdout for the engine to parse?
- Should descriptor refresh remove components when a descriptor stops declaring them, or only warn and preserve live state for safety?
- Which runtime mutations should beat descriptor refresh for light and emitter components?
- Should descriptor provenance live as an ECS component, a side table owned by `App`, or metadata inside `DataRegistry`?
