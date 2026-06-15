# Runtime Game-State Handles

## Goal

Make generated `postretro/game-state` handles executable in QuickJS and Luau.
Authors can bind engine-owned readonly slots without raw dotted strings.

This plan completes the runtime half omitted from M13 G1a. The production HUD
plan depends on it.

## Scope

### In scope

- One catalog of engine-owned readonly slots derived from `SlotTable`.
- Runtime namespace and slot-handle objects in every authoring context.
- TypeScript named imports from `"postretro/game-state"`.
- Luau namespace globals matching the existing Luau SDK convention.
- Immutable handles with `.get()` only.
- Runtime tests for both backends.
- Generated declaration cleanup and drift coverage.

### Out of scope

- Reading current slot values inside authoring VMs.
- Writable engine-slot handles.
- Per-frame VM access.
- New state-store primitives.
- Changes to slot schemas or values.
- A Luau virtual `require("postretro/game-state")` module.

## Runtime Contract

`postretro/game-state` exposes one namespace object per engine-owned readonly
slot namespace. Each slot is an immutable handle.

```ts
import { player } from "postretro/game-state";

Text({ bind: player.health.get() });
```

```luau
Text({ bind = player.health:get() })
```

Both calls return:

```json
{ "slot": "player.health" }
```

`.get()` creates descriptor data only. It does not read `SlotTable`. The VM
drops after authoring; retained Rust UI resolves the dotted slot against later
state snapshots.

Handles expose no `.set()` method. Namespace and handle objects are frozen so
author code cannot replace slots or methods.

### TypeScript lowering

`postretro-script-compiler` continues treating bare specifiers as
engine-provided globals. It strips:

```ts
import { player } from "postretro/game-state";
```

The remaining `player.health.get()` resolves against the QuickJS global
installed before author code executes. No compiler rewrite or runtime module
loader is added.

### Luau surface

Luau SDK vocabulary is installed as globals before `_G` freezes. Game-state
namespaces follow that convention. Authors use `player.health:get()` directly.

Remove generated text that suggests
`require("postretro/game-state")`. The current filesystem-rooted `require`
resolver remains unchanged.

## Shared Catalog

Move the current `EngineSlotGroup`, `EngineSlotType`, and
`engine_slot_groups()` responsibility out of oversized `typedef.rs` into a
focused `scripting/game_state.rs` module.

The catalog:

- constructs the built-in `SlotTable`;
- includes only `SlotOwnership::Engine` records with `readonly == true`;
- splits dotted names into namespace and slot;
- preserves the declared `SlotType`;
- sorts namespaces and slots deterministically;
- exposes the same collection to typedef emission and both runtime installers.

Writable engine slots such as `ui.textEntry` remain absent. Adding or removing
an engine readonly slot changes generated declarations and runtime handles from
the same source.

## Context Installation

Install game-state globals after primitives and SDK prelude, before author code.
Reject a namespace collision instead of silently replacing another SDK global.

QuickJS call sites:

- `build_definition_context_from_snapshot`;
- `run_data_script_quickjs`;
- `run_mod_init_quickjs`.

Luau call site:

- `build_lua_state_with_require_tracking`, after `evaluate_prelude` and before
  `sandbox(true)`. This covers definition, data, staged-manifest, and mod-init
  states that use the shared builder.

Installation failure aborts context construction with a named `ScriptError`.

## Boundary Inventory

| Boundary | Rust catalog | TypeScript | QuickJS runtime | Luau |
| --- | --- | --- | --- | --- |
| module | readonly engine slot groups | `"postretro/game-state"` | import stripped | n/a |
| namespace | dotted-name prefix | named export, e.g. `player` | frozen global object | frozen global table |
| slot handle | full dotted slot name | `ReadonlyStateValue<T>` | frozen object | frozen table |
| bind ref | full dotted slot name | `.get()` | `{ slot: "player.health" }` | `{ slot = "player.health" }` |
| mutation | readonly schema | no `.set()` type | no `.set()` value | no `:set()` value |

## Acceptance Criteria

- [ ] Bundled TypeScript containing
  `import { player } from "postretro/game-state"` executes in mod init.
- [ ] `player.health.get()` returns exactly `{ slot: "player.health" }` in
  QuickJS.
- [ ] `player.health:get()` returns the equivalent table in Luau.
- [ ] Runtime namespaces and generated declarations contain the same engine
  readonly slots, derived from one catalog.
- [ ] Writable engine slots are absent from declarations and both runtimes.
- [ ] Handles expose no write method. Attempts to replace a namespace slot or
  its `get` method fail or leave the original value unchanged.
- [ ] QuickJS definition, data, and mod-init contexts install game-state
  globals.
- [ ] Every Luau state built through the shared authoring-state builder
  installs game-state globals before sandbox freeze.
- [ ] A generated namespace collision fails context setup with a named
  diagnostic.
- [ ] Existing generated TypeScript and Luau declaration drift tests remain
  green.
- [ ] Luau declarations document bare namespace globals and no longer claim a
  virtual `require("postretro/game-state")` module.
- [ ] No live state read primitive or retained VM reference is introduced.

## Tasks

### Task 1: Extract the shared catalog

Add `crates/postretro/src/scripting/game_state.rs`. Move engine readonly slot
grouping and type mapping out of `typedef.rs`. Keep `SlotTable` as source of
truth. Update TypeScript and Luau emitters to consume the extracted catalog
without changing generated slot coverage.

Add catalog tests for deterministic ordering, readonly filtering, full dotted
names, and declared slot types.

### Task 2: Install QuickJS handles

Add a QuickJS installer in `game_state.rs`. It creates frozen namespace and
slot objects from the catalog. Each `get` closure captures one owned dotted
slot name and returns a fresh bind descriptor.

Add `postretro-script-compiler` to
`crates/postretro/Cargo.toml` as a dev-dependency. The runtime execution test
calls its public `bundle_entry` API.

Call the installer after `evaluate_prelude` in
`build_definition_context_from_snapshot`, `run_data_script_quickjs`, and
`run_mod_init_quickjs`. Preserve each context's existing primitive and
store-declaration setup.

Test direct context behavior plus a real TypeScript source-to-bundle-to-mod-init
flow. The execution test must use the named import, not hand-written JavaScript
globals.

### Task 3: Install Luau handles

Add the Luau installer in `game_state.rs`. It creates frozen namespace and slot
tables from the same catalog. Each colon-call-compatible `get` ignores `self`
and returns a fresh bind table.

Call it in `build_lua_state_with_require_tracking` after the SDK prelude and
before `sandbox(true)`. Do not special-case the filesystem `require` resolver.

Add runtime tests for bare `player.health:get()`, readonly behavior, catalog
coverage, and mod-init manifest use.

### Task 4: Align generated declarations and regressions

Keep the TypeScript ambient module and `ReadonlyStateValue<T>` contract. Update
Luau comments and examples to describe bare globals. Regenerate
`sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau`.

Extend drift tests so every catalog namespace and slot appears in generated
types and both runtime surfaces. Add a regression proving `.set` is absent at
type and runtime levels.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the shared catalog.

**Phase 2 (concurrent):** Task 2, Task 3 — independent runtime backends consume
the catalog.

**Phase 3 (sequential):** Task 4 — aligns declarations and cross-runtime drift
coverage.

## Rough Sketch

`game_state.rs` owns language-neutral catalog records and backend installers.
`typedef.rs` renders catalog `SlotType` values into language spellings. Runtime
installers use only namespace, slot, and full dotted name.

Do not route `.get()` through a primitive. The bind reference is immutable
descriptor data and needs no engine access.

`typedef.rs` and `runtime.rs` are oversized. Task 1 removes the existing
game-state block from `typedef.rs`; Tasks 2 and 3 add only installer calls to
runtime call sites. New logic and tests stay in `game_state.rs`.

## Open Questions

None.
