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
- **`call_context.rs`** — deleted. Defines `ScriptCallContext`, which exists only to support tick handler arguments. (`HandlerFn` is in `typedef.rs`.)
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

- `registerHandler` is absent from all scripting contexts. Calling it from a script raises an error (`ReferenceError` in QuickJS, `attempt to call a nil value` in Luau).
- `ScriptCallContext` and `HandlerFn` are absent from `postretro.d.ts` and `postretro.d.luau`.
- The `scripts/` directory under a content root is not scanned at level load. Files placed there have no effect.
- No file-watch thread starts in debug builds.
- `content/tests/scripts/rotator-driver.ts` and `rotator-driver.js` do not exist.
- `prl-build` ignores a `"script"` worldspawn KVP without error.
- Engine boots and loads a compiled `content/tests/maps/test-3.prl` without error (the `.prl` is not checked in; compile from `test-3.map` first).
- All remaining tests pass. Tests that exercise `registerHandler` or `Which::Behavior` are deleted alongside the code they covered.
- `scripting.md` contains no mention of the Live VM context, behavior context, or hot reload.
- `boot_sequence.md` Script Roles table contains no Live VM script row; the level load sequence contains no `scripts/` scan steps.

---

## Tasks

### Task 1: Remove Rust scripting infrastructure

Delete `event_dispatch.rs` and `call_context.rs`. In `runtime.rs`: remove `Which::Behavior`, the `handlers` field, and the `fire_tick`, `fire_level_load`, `clear_level_handlers`, and `reload_behavior_context` methods. In `luau.rs`: remove `Which::Behavior`, the `behavior_lua` field/getter, the `reload_behavior_context` method, and the two `ContextScope::BehaviorOnly` call sites that construct and reload the behavior Lua state. In `quickjs.rs`: remove `behavior_ctx` and the `BehaviorOnly` install branch. In `primitives_registry.rs`: remove `ContextScope::BehaviorOnly` and all branches that install stubs for it. In `primitives.rs`: remove the install-count comment that references `registerHandler`. In `runtime.rs`: remove the `current_source` publish call site (~line 245) that exists only for `registerHandler` to stamp source filenames onto handlers. In `mod.rs`: remove the `pub(crate) mod event_dispatch;` and `pub(crate) mod call_context;` declarations. In `typedef.rs`: remove `ScriptCallContext` and `HandlerFn` type alias entries. Remove tests that exercise `Which::Behavior` or `registerHandler` in `runtime.rs`, `quickjs.rs`, and `luau.rs`.

### Task 2: Remove main.rs call sites and hot reload

Remove `load_behavior_scripts` and all its call sites. Remove `fire_tick`, `fire_level_load`, `clear_level_handlers`, and `reload_behavior_context` call sites. Remove the hot-reload block (`drain_reload_requests` loop and `ScriptWatcher` initialization). Remove the `start_watcher` call and the `mod watcher` declaration in `scripting/mod.rs` (subject to the watcher TS-compilation decision — see architectural findings).

### Task 3: Remove level compiler support

In `crates/level-compiler/src/map_data.rs`: remove the `script` field from `MapData`. In `crates/level-compiler/src/parse.rs`: remove the `get_property` call that reads the `"script"` KVP and the `script` field initializer in the `MapData { … }` struct literal. In `crates/level-compiler/src/main.rs`: remove `compile_worldspawn_script` and its call site.

### Task 4: Remove test content and regenerate SDK types

Delete `content/tests/scripts/rotator-driver.ts` and `rotator-driver.js`. Regenerate `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` via `cargo run -p postretro --bin gen-script-types`.

### Task 5: Update docs

`scripting.md`: remove the Live VM row from the context model table (§2); fix the Data context lifecycle paragraph, which currently references Live VM ordering; remove the closing `registerHandler` sentence in §2; rewrite §1's 'escape hatch' sentence; rewrite §3 to describe the two remaining scopes (`DefinitionOnly`, `Both`); remove the hot-reload section (§8). `boot_sequence.md`: remove the Live VM script row from §2, remove the `scripts/` scan steps from the boot sequence (§3) and level load sequence (§4), and fix the cross-references at the 'Live VM scripts call `registerHandler`' line in §2 and the 'Hot reload targets the behavior context' line in §5. Also update `context/lib/index.md` to remove `hot reload` from the scripting routing entry.

---

## Sequencing

**Phase 1 (sequential):** Tasks 1, 2, 3 — must land together as one compilable pass. Removing Rust infrastructure (Task 1) and its call sites (Tasks 2, 3) cannot compile independently.

**Phase 2 (concurrent):** Tasks 4 and 5 — independent once the engine compiles. Note: SDK regen in Task 4 must be committed in the same pass as Phase 1 — the in-tree `.d.ts`/`.d.luau` drift-detection test fires immediately otherwise.
