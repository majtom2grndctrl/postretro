# Game State SDK Surface

## Goal

Ship one coherent scripting surface for durable game state.

Authors reference engine state through `gameState`, declare mod state through a
pure builder, and describe writes through reactions. Only values returned from
`setupMod()` or `setupLevel()` cross into the engine. The authoring VM then
drops.

The production HUD plan depends on this surface.

## Scope

### In scope

- One engine-state catalog shared by slot registration, generated types, and
  QuickJS/Luau runtime installation.
- A generated `gameState` root exported from `"postretro"`.
- Nested state domains such as `gameState.player.health.current`.
- Directly bindable state references. No `.get()`.
- Readonly and writable reference capabilities.
- Pure mod-state declarations returned through `setupMod().stores`.
- A typed `updateState(ref, value)` reaction constructor.
- State-consuming SDK helpers accepting references instead of raw dotted names.
- Runtime and type parity across TypeScript and Luau.

### Out of scope

- Reading current values inside an authoring VM.
- Per-frame VM access or retained script functions.
- Publishing movement fields to game state.
- A `gameState.query()` API.
- A `playerState` global.
- A `"postretro/game-state"` module.
- Renaming stable slot wire names solely to match SDK nesting.
- General runtime-expression writes. `updateState` accepts literals in this
  plan.

## Author Contract

`gameState` is the generated root for engine-owned durable state:

```ts
import { gameState, Text, Bar } from "postretro";

const health = gameState.player.health;

Text({
  content: "HP",
  bind: { ...health.current, format: "HP {}" },
});

Bar({
  bind: health.fraction,
  max: 1,
  fill: "ok",
  background: "panel.default",
});
```

State leaves are immutable descriptor references:

```ts
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
shape. For example,
`gameState.player.health.current` is `{ slot: "player.health" }`. Property
access does not read the current health value.

Readonly versus writable is a capability:

- Engine-produced observations such as player health are readonly.
- Engine-owned command surfaces such as `ui.textEntry` may be writable.
- Mod-declared state is writable unless its schema says otherwise.
- Runtime write validation remains authoritative. Types prevent common errors
  but do not replace engine checks.

State references have no `.get()`, `.set()`, or `.is()` methods. Nouns select
state. `stateEquals(ref, value)` builds an equality predicate. Reaction
constructors build writes.

### Mod state

`defineStore` becomes a pure SDK builder:

```ts
const objectives = defineStore("objectives", {
  killCount: { type: "number", default: 0 },
});
```

The store lives in a shared source module. The mod-init entry imports it and
publishes the declaration:

```ts
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
the VM drops.

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

SDK helpers accept `ReadonlyStateRef<T>` or `WritableStateRef<T>` as their
public input. They serialize the reference's `slot` field into existing wire
shapes.

This includes:

- widget `bind` properties;
- `stateEquals`;
- state-crossing watchers;
- `updateState`;
- text-edit reactions.

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
- installs QuickJS and Luau `gameState` objects.

SDK paths are explicit metadata, not inferred from camelCase wire names. This
allows:

| Stable wire name | SDK path |
| --- | --- |
| `player.health` | `gameState.player.health.current` |
| `player.healthFraction` | `gameState.player.health.fraction` |
| `screen.flash` | `gameState.screen.flash` |
| `input.mode` | `gameState.input.mode` |
| `ui.textEntry` | `gameState.ui.textEntry` |

Future movement observations may use
`gameState.player.movement.<field>`. This plan does not publish any. Movement
remains engine-private until a use case specifies each field's producer,
lifetime, and capability.

Catalog validation rejects:

- duplicate wire names;
- duplicate SDK paths;
- a path used as both a leaf and an object;
- invalid or empty path segments.

Ordering is deterministic.

## Runtime Installation

Install one frozen `gameState` object before author code executes.

TypeScript imports it from the normal SDK module:

```ts
import { gameState } from "postretro";
```

The script compiler strips the import as it does other SDK imports. The
QuickJS prelude/global installation supplies the value.

Luau receives the same `gameState` global through the shared prelude/state
builder. No virtual module or filesystem `require` path is added.

Nested objects and leaf references are frozen. Author code cannot replace a
domain, slot reference, or `slot` value.

## Boundary Inventory

| Concept | Rust / catalog | Wire | TypeScript | Luau |
| --- | --- | --- | --- | --- |
| engine root | catalog tree | n/a | `gameState` | `gameState` |
| readonly leaf | readonly catalog entry | dotted slot string | `ReadonlyStateRef<T>` | `ReadonlyStateRef<T>` |
| writable leaf | writable catalog entry | dotted slot string | `WritableStateRef<T>` | `WritableStateRef<T>` |
| mod declaration | namespace + schema | `ModManifest.stores[]` | `defineStore(...).declaration` | same |
| mod references | schema-derived names | `{ slot: string }` where consumed | `defineStore(...).state` | same |
| state write | reaction descriptor | primitive `"setState"` | `updateState(ref, value)` | `updateState(ref, value)` |
| equality predicate | predicate descriptor | `{ slot, equals }` | `stateEquals(ref, value)` | same |
| UI bind | existing bind descriptor | `{ slot, ...options }` | state ref or spread with options | same |

## Acceptance Criteria

- [ ] `import { gameState } from "postretro"` bundles and executes in mod init.
- [ ] `gameState.player.health.current` is exactly the immutable descriptor
  `{ slot: "player.health" }` in QuickJS and Luau.
- [ ] `gameState.player.health.fraction` maps to
  `{ slot: "player.healthFraction" }`.
- [ ] Generated types and both runtimes contain the same catalog paths,
  value types, and write capabilities.
- [ ] Writable engine state appears as `WritableStateRef<T>`; readonly state
  cannot be passed to `updateState` in TypeScript.
- [ ] Direct state references bind to widgets without `.get()`.
- [ ] Bind options such as `format` and `tween` compose by spreading a state
  reference into the existing bind descriptor.
- [ ] `defineStore` performs no FFI. A returned store declaration commits; an
  unreturned declaration does not.
- [ ] Failed `setupMod()` validation commits no store declarations.
- [ ] `updateState` is pure and emits the existing `setState` wire descriptor.
- [ ] A returned reaction updates writable mod state at the game-logic stage.
- [ ] `stateEquals`, crossing watchers, and text-edit reactions accept typed
  references and preserve their existing wire formats.
- [ ] Nested runtime objects and state references cannot be mutated.
- [ ] QuickJS definition, data, and mod-init contexts install `gameState`.
- [ ] Every Luau authoring state installs `gameState` before sandbox freeze.
- [ ] No `.get()`-based engine handle, `gameState.query`, `playerState` global,
  or `"postretro/game-state"` module remains in generated SDK declarations.
- [ ] No live state read primitive or retained VM reference is introduced.

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

Remove `storeHandle` from the authoritative-state path. Presentation-local
state remains separate and keeps its existing scoped cell behavior.

### Task 3: Make store declarations return through setup

Turn `defineStore` into a pure cross-runtime builder returning
`{ declaration, state }`. Add `ModManifest.stores`. Drain and validate returned
declarations only after `setupMod()` succeeds.

Keep declaration commits atomic. Preserve identical-schema resume and
persistence behavior after commit.

Update development stores to return declarations from `setupMod()` instead of
registering through import-time FFI.

### Task 4: Add reaction-based state writes

Add typed `updateState(ref, value)` and migrate author-facing state writes to
it. Keep the Rust `"setState"` reaction handler and wire descriptor unchanged.

Replace authoritative `StoreHandle.is` with `stateEquals`. Update crossing
watchers and text-edit builders to take state references. Preserve
game-logic-stage dispatch and readonly runtime gating.

### Task 5: Install `gameState` in QuickJS and Luau

Build frozen nested objects from the shared catalog. Install them in every
authoring context before user code. Reject collisions with existing SDK
globals.

Add TypeScript source-to-bundle-to-mod-init coverage and Luau parity coverage.

### Task 6: Align generated declarations and regressions

Generate `gameState` under `"postretro"` for both languages. Remove the special
game-state module and `.get()` declarations.

Add catalog/type/runtime parity tests, negative capability tests, setup-return
commit tests, and generated-file drift coverage.

## Sequencing

**Phase 1 (sequential):** Task 1 establishes the catalog.

**Phase 2 (concurrent):** Task 2 defines references; Task 3 defines setup-return
store publication.

**Phase 3 (sequential):** Task 4 consumes Tasks 2 and 3.

**Phase 4 (sequential):** Task 5 consumes the catalog and reference contract.

**Phase 5 (sequential):** Task 6 aligns both backends and generated artifacts.

## Open Questions

None.
