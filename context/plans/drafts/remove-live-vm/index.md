# Remove Live VM Context

> **Status:** draft

---

## Goal

Remove the Live VM scripting context: the `registerHandler` primitive, the behavior VM, and all supporting infrastructure. PostRetro's scripting model is declarative — scripts register entity types and reaction descriptors that Rust drives at runtime. Per-tick VM callbacks belong to a different model and have no concrete use case.

---

## Scope

### In scope

- **`registerHandler` primitive** — removed from both QuickJS and Luau contexts. Scripts that call it receive a `ReferenceError`; no stub, no `WrongContext`.
- **`Which::Behavior` context** — removed. `Which::Definition` remains as the sole persistent VM context.
- **`ContextScope::BehaviorOnly`** — removed. Only `DefinitionOnly` and `Both` remain.
- **`event_dispatch.rs`** — deleted. All types it defines (`EventKind`, `Handler`, `HandlerCallable`, `HandlerTable`, `SharedHandlerTable`) are gone.
- **`call_context.rs`** — deleted. `ScriptCallContext` and `HandlerFn` exist only to support tick handler arguments.
- **`ScriptRuntime` methods** — `fire_tick`, `fire_level_load`, `clear_level_handlers`, and `reload_behavior_context` removed.
- **`load_behavior_scripts`** — removed from `main.rs`. The `scripts/` directory is no longer scanned at level load.
- **Hot-reload watcher** — removed. `drain_reload_requests` and `ScriptWatcher` initialization removed; watcher targeted the behavior context.
- **Level compiler** — `compile_worldspawn_script` and the `"script"` worldspawn KVP removed from `prl-build`. The `script` field removed from the parsed map data struct.
- **Test content** — `content/tests/scripts/rotator-driver.ts` and `rotator-driver.js` deleted.
- **SDK types** — `registerHandler`, `ScriptCallContext`, and `HandlerFn` removed from `postretro.d.ts` and `postretro.d.luau`.
- **Docs** — `scripting.md` and `boot_sequence.md` updated to reflect the removed context and load sequence changes.

### Out of scope

- Replacing tick callbacks with a declarative equivalent (no use case yet)
- Event-driven scripting for game-logic triggers (future work)
- Changes to `registerReaction`, `registerEntity`, `registerLevelManifest`, or the data context lifecycle
- The `"script"` worldspawn KVP in FGD or TrenchBroom tooling (`.map` source only; KVP is now silently absent from `prl-build` output)
- Changes to the `scripts/` directory convention in the planned folder reorg

---

## Acceptance criteria

- `registerHandler` is absent from all scripting contexts. Calling it from a script produces a `ReferenceError`.
- `ScriptCallContext` and `HandlerFn` are absent from `postretro.d.ts` and `postretro.d.luau`.
- The `scripts/` directory under a content root is not scanned at level load. Files placed there have no effect.
- No file-watch thread starts in debug builds.
- `content/tests/scripts/rotator-driver.ts` and `rotator-driver.js` do not exist.
- `prl-build` ignores a `"script"` worldspawn KVP without error.
- Engine boots and loads `content/tests/maps/test-3.prl` without error.
- All remaining tests pass. Tests that exercise `registerHandler` or `Which::Behavior` are deleted alongside the code they covered.
- `scripting.md` contains no mention of the Live VM context, behavior context, or hot reload.
- `boot_sequence.md` Script Roles table contains no Live VM script row; the level load sequence contains no `scripts/` scan steps.

---

## Tasks

### Task 1: Remove Rust scripting infrastructure

Delete `event_dispatch.rs` and `call_context.rs`. In `runtime.rs`: remove `Which::Behavior`, the `handlers` field, and the `fire_tick`, `fire_level_load`, `clear_level_handlers`, and `reload_behavior_context` methods. In `luau.rs`: remove `Which::Behavior` and the `behavior_ctx` method. In `quickjs.rs`: remove `behavior_ctx` and the `BehaviorOnly` install branch. In `primitives_registry.rs`: remove `ContextScope::BehaviorOnly` and all branches that install stubs for it. In `primitives.rs`: remove the install-count comment that references `registerHandler`. In `mod.rs`: remove re-exports from `event_dispatch` and `call_context`. In `typedef.rs`: remove `ScriptCallContext` and `HandlerFn` type alias entries. Remove runtime tests that exercise `Which::Behavior` or `registerHandler`.

### Task 2: Remove main.rs call sites and hot reload

Remove `load_behavior_scripts` and all its call sites. Remove `fire_tick`, `fire_level_load`, `clear_level_handlers`, and `reload_behavior_context` call sites. Remove the hot-reload block (`drain_reload_requests` loop and `ScriptWatcher` initialization).

### Task 3: Remove level compiler support

In `crates/level-compiler/src/parse.rs`: remove the `script` field from the parsed map data struct and the `get_property` call that reads the `"script"` KVP. In `crates/level-compiler/src/main.rs`: remove `compile_worldspawn_script` and its call site.

### Task 4: Remove test content and regenerate SDK types

Delete `content/tests/scripts/rotator-driver.ts` and `rotator-driver.js`. Regenerate `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` via `cargo run -p postretro --bin gen-script-types`.

### Task 5: Update docs

`scripting.md`: remove the Live VM row from the context model table (§2), remove `BehaviorOnly` from the context scope section (§3), remove the hot-reload section (§8). `boot_sequence.md`: remove the Live VM script row from §2, remove the `scripts/` scan steps from the boot sequence (§3) and level load sequence (§4).

---

## Sequencing

**Phase 1 (sequential):** Tasks 1, 2, 3 — must land together as one compilable pass. Removing Rust infrastructure (Task 1) and its call sites (Tasks 2, 3) cannot compile independently.

**Phase 2 (concurrent):** Tasks 4 and 5 — independent once the engine compiles.
