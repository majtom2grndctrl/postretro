# Remove Live VM Context

> **Status:** ready

---

## Goal

Remove the Live VM scripting context: the `registerHandler` primitive, the behavior VM, and all supporting infrastructure. PostRetro's scripting model is declarative — scripts register entity types and reaction descriptors that Rust drives at runtime. Per-tick VM callbacks belong to a different model and have no concrete use case.

---

## Scope

### In scope

- **`registerHandler` primitive** — removed from both QuickJS and Luau contexts. Scripts that call it receive a `ReferenceError`; no stub, no `WrongContext`.
- **`Which::Behavior` context** — removed. `Which::Definition` remains as the sole persistent VM context.
- **`ContextScope::BehaviorOnly`** — removed. Only `DefinitionOnly` and `Both` remain.
- **All `BehaviorOnly` primitives** — deleted: `spawnEntity`, `despawnEntity`, `getComponent`, `setComponent`, `emitEvent`, `sendEvent`. All exist only to support the live VM tick pattern.
- **Event infrastructure** — deleted: `ScriptEvent`, `GameEvent`, `EventQueue`, `GAME_EVENTS_CAPACITY`, and the `events`/`game_events` fields on `ScriptCtx` (all in `ctx.rs`). The `game_events` drain in `main.rs` goes with them.
- **`event_dispatch.rs`** — deleted. All types it defines (`EventKind`, `Handler`, `HandlerCallable`, `HandlerTable`, `SharedHandlerTable`) are gone.
- **`call_context.rs`** — deleted. Defines `ScriptCallContext`, which exists only to support tick handler arguments. (`HandlerFn` is in `typedef.rs`.)
- **`ScriptRuntime` methods** — `fire_tick`, `fire_level_load`, `clear_level_handlers`, and `reload_behavior_context` removed.
- **`load_behavior_scripts`** — removed from `main.rs`. The `scripts/` directory is no longer scanned at level load.
- **Hot-reload watcher** — `watcher.rs` kept. Remove only the behavior-VM-reload logic: `reload_behavior_context` call and handler-clearing sequence inside `drain_reload_requests`. File watching and TS/Luau compilation infrastructure stays intact.
- **Level compiler** — `compile_worldspawn_script` and the `"script"` worldspawn KVP removed from `prl-build`. The `script` field removed from the parsed map data struct.
- **Test content** — `content/tests/scripts/rotator-driver.ts` and `rotator-driver.js` deleted.
- **SDK types** — `registerHandler`, `ScriptCallContext`, `HandlerFn`, `ScriptEvent`, `spawnEntity`, `despawnEntity`, `getComponent`, `setComponent`, `emitEvent`, and `sendEvent` removed from `postretro.d.ts` and `postretro.d.luau`.
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
- `ScriptCallContext`, `HandlerFn`, `ScriptEvent`, `spawnEntity`, `despawnEntity`, `getComponent`, `setComponent`, `emitEvent`, and `sendEvent` are absent from `postretro.d.ts` and `postretro.d.luau`.
- The `scripts/` directory under a content root is not scanned at level load. Files placed there have no effect.
- File-watch thread starts in debug builds; no behavior-VM-reload logic executes when files change.
- `content/tests/scripts/rotator-driver.ts` and `rotator-driver.js` do not exist.
- `prl-build` ignores a `"script"` worldspawn KVP without error.
- Engine boots and loads a compiled `content/tests/maps/test-3.prl` without error (the `.prl` is not checked in; compile from `test-3.map` first).
- All remaining tests pass. Tests that exercise `registerHandler` or `Which::Behavior` are deleted alongside the code they covered.
- `scripting.md` contains no mention of the Live VM context, behavior context, or hot reload.
- `boot_sequence.md` Script Roles table contains no Live VM script row; the level load sequence contains no `scripts/` scan steps.

---

## Tasks

### Task 1: Remove Rust scripting infrastructure

Delete `event_dispatch.rs` and `call_context.rs`. In `runtime.rs`: remove `Which::Behavior`, the `handlers` field, and the `fire_tick`, `fire_level_load`, `clear_level_handlers`, and `reload_behavior_context` methods. In `luau.rs`: remove `Which::Behavior`, the `behavior_lua` field/getter, the `reload_behavior_context` method, and the two `ContextScope::BehaviorOnly` call sites that construct and reload the behavior Lua state. In `quickjs.rs`: remove `behavior_ctx` and the `BehaviorOnly` install branch. In `primitives_registry.rs`: remove `ContextScope::BehaviorOnly` and all branches that install stubs for it. In `primitives.rs`: remove `spawnEntity`, `despawnEntity`, `getComponent`, `setComponent`, `emitEvent`, and `sendEvent` (all `BehaviorOnly`), plus the install-count comment that references `registerHandler`. In `ctx.rs`: remove `ScriptEvent`, `GameEvent`, `EventQueue`, `GAME_EVENTS_CAPACITY`, and the `events` and `game_events` fields from `ScriptCtx`. In `runtime.rs`: remove the `current_source` publish call site (~line 245) that exists only for `registerHandler` to stamp source filenames onto handlers. In `mod.rs`: remove the `pub(crate) mod event_dispatch;` and `pub(crate) mod call_context;` declarations. In `typedef.rs`: remove `ScriptCallContext`, `HandlerFn`, and `ScriptEvent` type alias entries. Remove tests that exercise `Which::Behavior`, `registerHandler`, or the deleted primitives in `runtime.rs`, `quickjs.rs`, and `luau.rs`.

### Task 2: Remove main.rs call sites and hot reload

Remove `load_behavior_scripts` and all its call sites. Remove `fire_tick`, `fire_level_load`, `clear_level_handlers`, and `reload_behavior_context` call sites. Remove the behavior-VM-reload logic inside `drain_reload_requests` (the `reload_behavior_context` call and handler-clearing sequence); leave the `drain_reload_requests` call site in the game loop and the `ScriptWatcher` initialization intact. Remove the `game_events` drain block.

### Task 3: Remove level compiler support

In `crates/level-compiler/src/map_data.rs`: remove the `script` field from `MapData`. In `crates/level-compiler/src/parse.rs`: remove the `get_property` call that reads the `"script"` KVP and the `script` field initializer in the `MapData { … }` struct literal. In `crates/level-compiler/src/main.rs`: remove `compile_worldspawn_script` and its call site.

### Task 4: Remove test content and regenerate SDK types

Delete `content/tests/scripts/rotator-driver.ts` and `rotator-driver.js`. Regenerate `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` via `cargo run -p postretro --bin gen-script-types`.

### Task 5: Update docs

`scripting.md`: remove the Live VM row from the context model table (§2); fix the Data context lifecycle paragraph, which currently references Live VM ordering; remove the closing `registerHandler` sentence in §2; rewrite §1's 'escape hatch' sentence; rewrite §3 to describe the two remaining scopes (`DefinitionOnly`, `Both`); remove the hot-reload section (§8). `boot_sequence.md`: remove the Live VM script row from §2, remove the `scripts/` scan steps from the boot sequence (§3) and level load sequence (§4), and fix the cross-references at the 'Live VM scripts call `registerHandler`' line in §2 and the 'Hot reload targets the behavior context' line in §5. Also update `context/lib/index.md` to remove `hot reload` from the scripting routing entry.

---

## Open questions

None.

---

## Sequencing

**Phase 1 (sequential):** Tasks 1, 2, 3 — must land together as one compilable pass. Removing Rust infrastructure (Task 1) and its call sites (Tasks 2, 3) cannot compile independently.

**Phase 2 (concurrent):** Tasks 4 and 5 — independent once the engine compiles. Note: SDK regen in Task 4 must be committed in the same pass as Phase 1 — the in-tree `.d.ts`/`.d.luau` drift-detection test fires immediately otherwise.
