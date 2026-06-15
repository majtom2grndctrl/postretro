# Game State SDK Surface

## Goal

Ship one coherent scripting surface for durable game state.

Authors obtain engine-state references through `getGameState()`, declare mod
state through a pure builder, and describe writes through reactions. Only
values returned from `setupMod()` or `setupLevel()` cross into the engine. The
short-lived mod-init or data VM then drops. The long-lived definition context
exposes the same reference surface but is not part of that drop lifecycle.

The production HUD plan depends on this surface.

## Scope

### In scope

- One engine-state catalog shared by slot registration, generated types, and
  QuickJS/Luau runtime installation.
- A generated `getGameState()` accessor exported from `"postretro"`.
- Nested state domains such as
  `getGameState().player.health`.
- Directly bindable state references. No `.get()`.
- Readonly and writable reference capabilities.
- Pure mod-state declarations returned through `setupMod().stores`.
- A typed `updateState(ref, value)` reaction constructor.
- State-consuming SDK helpers accepting references instead of raw dotted names.
- Debug staged reload of returned store declarations.
- Runtime and type parity across TypeScript and Luau.

### Out of scope

- Reading current values inside an authoring VM.
- Per-frame VM access or retained script functions.
- Publishing movement fields to game state.
- A `gameState.query()` API.
- A `gameState` global.
- A `playerState` global.
- A `"postretro/game-state"` module.
- Renaming stable slot wire names solely to match SDK nesting.
- General runtime-expression writes. `updateState` accepts literals in this
  plan.
- UI-tree, theme, and font hot reload. The HUD plan owns the relevant UI and
  theme path.

## Author Contract

`getGameState()` returns the generated reference tree for engine-owned durable
state:

```ts
// Proposed design
import { bindState, getGameState, Text, Bar } from "postretro";

function buildHud() {
  const { player } = getGameState();

  return [
    Text({
      content: "HP",
      bind: bindState(player.health, { format: "HP {}" }),
    }),

    Bar({
      bind: player.health,
      max: player.maxHealth,
      fill: "ok",
      background: "panel.default",
    }),
  ];
}
```

Domain builders obtain the references they consume. Callers do not pass
`player`, `screen`, or other engine-state domains as component props.

`getGameState()` is pure, deterministic, and idempotent. It returns immutable
descriptor references. It does not read current game values, register data, or
cross FFI.

State leaves are immutable descriptor references:

```ts
// Proposed design
declare const stateRefValueBrand: unique symbol;
declare const writableStateRefBrand: unique symbol;

type ReadonlyStateRef<T> = {
  readonly slot: string;
  readonly [stateRefValueBrand]: T;
};

type WritableStateRef<T> = ReadonlyStateRef<T> & {
  readonly [writableStateRefBrand]: T;
};
```

The writable marker is type-only. Both reference types have the same runtime
shape. For example, `getGameState().player.health` is
`{ slot: "player.health" }`. Property access does not read current health.

Readonly versus writable is a capability:

- Engine-produced observations such as player health are readonly.
- Engine-owned command surfaces such as `ui.textEntry` may be writable.
- Mod-declared state is writable unless its schema says otherwise.
- Runtime write validation remains authoritative. Types prevent common errors
  but do not replace engine checks.

State references have no `.get()`, `.set()`, or `.is()` methods. Nouns select
state. `bindState(ref, options)` adds bind-only options without changing the
reference. `stateEquals(ref, value)` builds an equality predicate. Reaction
constructors build writes.

### Mod state

`defineStore` becomes a pure SDK builder:

```ts
// Proposed design
const objectives = defineStore("objectives", {
  killCount: { type: "number", default: 0 },
});
```

The store lives in a shared source module. The mod-init entry imports it and
publishes the declaration:

```ts
// Proposed design
export function setupMod() {
  return {
    name: "example",
    stores: [objectives.declaration],
  };
}
```

A level data entry imports the same pure module. Its VM reconstructs the same
stable references without attempting another declaration:

```ts
// Proposed design
const resetKills = defineReaction(
  "resetKills",
  updateState(objectives.state.killCount, 0),
);

export function setupLevel() {
  return {
    reactions: [resetKills],
  };
}
```

`defineStore` returns:

- `declaration`: plain namespace and schema data for
  `setupMod().stores`;
- `state`: typed references keyed by schema field.

Calling `defineStore`, `updateState`, or `defineReaction` performs no FFI and
changes no engine state. Unreturned declarations and reactions disappear when
the short-lived setup VM drops.

`updateState` emits the existing `setState` reaction wire descriptor. This plan
changes author syntax, not the reaction opcode:

```json
{
  "primitive": "setState",
  "args": {
    "slot": "objectives.killCount",
    "value": 0
  }
}
```

### State consumers

SDK helpers serialize a reference's `slot` field into existing wire shapes.
Capabilities and value types are explicit:

| Consumer | Accepted reference | Result |
| --- | --- | --- |
| `Text.bind` | readonly scalar | text bind |
| `Panel.bind` | readonly numeric-array | color bind |
| `Bar.bind` | readonly number | numeric bind |
| `Bar.max` | readonly number or literal number | max field extended to accept literals or state refs |
| `Slider.bind` | writable number | numeric bind and event-time writes |
| `bindState` | any bind-compatible reference plus matching options | existing `{ slot, format?, tween? }` bind |
| `stateEquals` | readonly number, boolean, string, or enum | `{ slot, equals }` predicate |
| `onStateCrossing` | readonly number | existing crossing descriptor |
| `updateState` | writable `T` plus literal `T` | existing `setState` reaction |
| `Tree.textEntryTarget` | writable string | existing dotted target field |
| text-edit reactions | writable string | existing text-edit reactions |

`ReadonlyStateRef` in this table means readable capability; writable
references also satisfy it. Array equality remains unsupported.

`bindState(ref, options)` is the cross-runtime option-composition syntax:

```ts
// Proposed design
bindState(player.health, { format: "HP {}" });
```

```luau
-- Proposed design
bindState(player.health, { format = "HP {}" })
```

Passing a bare reference remains valid when no bind options are needed.
TypeScript and Luau emit byte-identical bind descriptors.

JSON descriptors and Rust wire structures continue using dotted slot strings.
The reference object is an SDK authoring affordance, not a new retained wire
format.

## Engine State Catalog

Extract built-in engine slot declarations from `slot_table.rs` into a focused
catalog module. Each entry carries:

- stable dotted wire name;
- generated SDK path segments;
- slot schema and default;
- readonly/writable capability;
- declared value type.

The same catalog:

- constructs the built-in `SlotTable`;
- generates TypeScript and Luau state types;
- supplies the frozen reference tree returned by QuickJS and Luau
  `getGameState()`.

SDK paths are explicit metadata, not inferred from camelCase wire names. This
allows:

| Stable wire name | SDK path |
| --- | --- |
| `player.health` | `getGameState().player.health` |
| `player.maxHealth` | `getGameState().player.maxHealth` |
| `screen.flash` | `getGameState().screen.flash` |
| `input.mode` | `getGameState().input.mode` |
| `ui.textEntry` | `getGameState().ui.textEntry` |

Future movement observations may use
`getGameState().player.movement.<field>`. This plan does not publish any.
Movement remains engine-private until a use case specifies each field's
producer, lifetime, and capability.

Catalog validation rejects:

- duplicate wire names;
- duplicate SDK paths;
- a path used as both a leaf and an object;
- invalid or empty path segments.

Ordering is deterministic.

Catalog construction returns a named validation error for malformed path
metadata. Context construction fails before author code runs; it never exposes
a partial reference tree.

## Runtime Installation

Rust installs the frozen catalog projection under an engine-internal bridge
global before SDK prelude evaluation. The prelude captures that tree into a
language-native closure:

```ts
// Proposed design
const gameStateRefs = globalThis.__postretroGameStateRefs;
delete globalThis.__postretroGameStateRefs;

export function getGameState(): GameStateRefs {
  return gameStateRefs;
}
```

Luau follows the same capture-then-hide sequence before sandbox freeze.
Calling `getGameState()` invokes no host callback and performs no FFI. Repeated
calls return the same frozen tree for that authoring context.

TypeScript imports it from the normal SDK module:

```ts
// Proposed design
import { getGameState } from "postretro";
```

The script compiler strips the import as it does other SDK imports. The
QuickJS prelude/global installation supplies the value.

No virtual module or filesystem `require` path is added. The internal bridge
name is absent after prelude installation.

Nested objects and leaf references are frozen. Author code cannot replace a
domain, slot reference, or `slot` value.

## Boundary Inventory

| Concept | Rust / catalog | Wire | TypeScript | Luau |
| --- | --- | --- | --- | --- |
| engine accessor | catalog tree | n/a | `getGameState(): GameStateRefs` | `getGameState(): GameStateRefs` |
| readonly leaf | readonly catalog entry | dotted slot string | `ReadonlyStateRef<T>` | `ReadonlyStateRef<T>` |
| writable leaf | writable catalog entry | dotted slot string | `WritableStateRef<T>` | `WritableStateRef<T>` |
| mod declaration | namespace + schema | `ModManifest.stores[]` | `defineStore(...).declaration` | same |
| mod references | schema-derived names | `{ slot: string }` where consumed | `defineStore(...).state` | same |
| state write | reaction descriptor | primitive `"setState"` | `updateState(ref, value)` | `updateState(ref, value)` |
| equality predicate | predicate descriptor | `{ slot, equals }` | `stateEquals(ref, value)` | same |
| UI bind | existing bind descriptor | `{ slot, ...options }` | bare ref or `bindState(ref, options)` | same |

## Staged Reload

Cold and staged mod-init parse `setupMod().stores` through the same returned
manifest contract. The staged worker carries the validated
`StoreDeclarationSet` as owned `Send` data. It installs no attempt-local
`defineStore` primitive.

Main-thread commit keeps existing store semantics:

- identical declarations preserve values;
- new non-overlapping namespaces commit;
- changed or colliding schemas reject the whole staged result;
- omitted declarations do not delete committed stores;
- failed and stale staged results preserve prior state.

## Acceptance Criteria

- [ ] `import { getGameState } from "postretro"` bundles and executes in mod
  init.
- [ ] `getGameState().player.health` is exactly the immutable
  descriptor `{ slot: "player.health" }` in QuickJS and Luau.
- [ ] `getGameState().player.maxHealth` maps to the immutable descriptor
  `{ slot: "player.maxHealth" }` in QuickJS and Luau.
- [ ] Repeated calls in one authoring context return the same frozen reference
  tree, invoke no host callback, and leave no bridge global visible.
- [ ] A UI builder can call `getGameState()` internally; its caller passes no
  engine-state domain as a prop.
- [ ] Generated types and both runtimes contain the same catalog paths,
  value types, and write capabilities.
- [ ] Writable engine state appears as `WritableStateRef<T>`; readonly state
  cannot be passed to `updateState` in TypeScript.
- [ ] Direct state references bind to widgets without `.get()`.
- [ ] `bindState` composes `format` and matching tween options in TypeScript
  and Luau with byte-identical wire output.
- [ ] Type checks require writable-number refs for `Slider`, writable-string
  refs for text-entry targets and text edits, numeric refs for crossings, and
  scalar refs for equality predicates.
- [ ] `defineStore` performs no FFI. A returned store declaration commits; an
  unreturned declaration does not.
- [ ] Failed `setupMod()` validation commits no store declarations.
- [ ] Staged mod init consumes returned stores without a side-effecting
  `defineStore` primitive. Compatible additions commit; incompatible schemas,
  failed builds, and stale generations preserve the prior table.
- [ ] `updateState` is pure and emits the existing `setState` wire descriptor.
- [ ] A returned reaction updates writable mod state at the game-logic stage.
- [ ] `stateEquals`, crossing watchers, and text-edit reactions accept typed
  references and preserve their existing wire formats.
- [ ] Nested runtime objects and state references cannot be mutated.
- [ ] Malformed catalog SDK paths fail context construction with a named
  diagnostic and expose no partial tree.
- [ ] QuickJS definition, data, and mod-init contexts install `getGameState`.
- [ ] Every Luau authoring state installs `getGameState` before sandbox freeze.
- [ ] No `.get()`-based engine handle, `gameState.query`, `playerState` global,
  `gameState` global, or `"postretro/game-state"` module remains in generated
  SDK declarations.
- [ ] No live state read primitive or retained VM reference is introduced.
- [ ] Authoritative `storeHandle` usage is removed; presentation-local state
  handles remain unchanged.
- [ ] Durable scripting and UI documentation shows `getGameState()`,
  `bindState`, returned store declarations, and reaction-based writes without
  legacy `.get()` or import-time registration examples.

## Tasks

### Task 1: Extract and enrich the engine-state catalog

Move built-in slot declarations out of `slot_table.rs`. Add explicit SDK paths
and capability metadata. Make `SlotTable`, typedef generation, and later
runtime installers consume this catalog.

Add deterministic tree construction and validation tests. Preserve existing
wire names and slot behavior.

### Task 2: Define the state-reference SDK contract

Replace branded string and `.get()` handle types with readonly and writable
reference objects. Update widget binds and other state-consuming SDK helpers to
accept references while emitting existing dotted-name wire fields.

Add `bindState` in both SDKs. Pin each consumer's readable/writable and value
constraints to the matrix above. Remove `storeHandle` from the
authoritative-state path. Presentation-local state remains separate and keeps
its existing scoped cell behavior.

### Task 3: Make store declarations return through setup

Turn `defineStore` into a pure cross-runtime builder returning
`{ declaration, state }`. Add `ModManifest.stores`. Drain and validate returned
declarations only after `setupMod()` succeeds.

Keep declaration commits atomic. Preserve identical-schema resume and
persistence behavior after commit.

Update development stores to return declarations from `setupMod()` instead of
registering through import-time FFI.

Update the staged QuickJS and Luau manifest builders to parse the same returned
`stores` field. Remove their attempt-local store primitive and carry validated
declarations through the worker envelope into existing reconcile/commit logic.

### Task 4: Add reaction-based state writes

Add typed `updateState(ref, value)` and migrate author-facing state writes to
it. Keep the Rust `"setState"` reaction handler and wire descriptor unchanged.

Replace authoritative `StoreHandle.is` with `stateEquals`. Update crossing
watchers and text-edit builders to take state references. Preserve
game-logic-stage dispatch and readonly runtime gating.

### Task 5: Install `getGameState` in QuickJS and Luau

Build one frozen nested reference tree from the shared catalog per authoring
context. Install it under a hidden bridge before the prelude. The prelude
captures it into a pure `getGameState` closure and removes the bridge before
author code runs. Reject collisions with existing SDK globals.

Add TypeScript source-to-bundle-to-mod-init coverage, Luau parity coverage,
bridge-hiding coverage, and a regression proving repeated calls invoke no host
function.

### Task 6: Align generated declarations and regressions

Generate `GameStateRefs` and `getGameState()` under `"postretro"` for both
languages. Remove the special game-state module, `gameState` global, and
`.get()` declarations.

Add catalog/type/runtime parity tests, negative capability tests, setup-return
commit and staged-reload tests, `bindState` byte-parity tests, catalog
diagnostic tests, and generated-file drift coverage.

Update durable scripting and UI documentation to the final accessor, binding,
returned-store, and reaction-write contracts. Remove legacy `.get()` and
import-time store registration examples.

## Sequencing

**Phase 1 (sequential):** Task 1 establishes the catalog.

**Phase 2 (concurrent):** Task 2 defines references; Task 3 defines setup-return
store publication.

**Phase 3 (sequential):** Task 4 consumes Tasks 2 and 3.

**Phase 4 (sequential):** Task 5 consumes the catalog and reference contract.

**Phase 5 (sequential):** Task 6 aligns both backends and generated artifacts.

## Open Questions

None.
