# Scripting State Convergence

## Goal

One reference-and-expression model for durable game state across engine observations, mod stores, and impact
policies — replacing three non-converging authoring dialects. Removes the `store.state.key` hop, gives a store
slot a real read (`readState`) and a general write (`set`), and unifies expression-building under the shipped
fluent IR algebra so a policy can read engine state, store state, and impact facts in one expression. The
governing mental model is "the compile-to-IR shadow of Vue" (`research.md`): `Ref`/`ComputedRef` sources, named
derived expressions, and a deferred `when` guard — evaluated in Rust, not JS.

Engine-minimal: reuses the shipped IR evaluator and binder. The only engine change is one new write-primitive
match arm (`slot.set`) that reuses the existing `bind_number_write`; no new IR node type, no evaluator
vocabulary widening.

## Scope

### In scope

- **Store surface flatten.** `defineStore` returns slot refs at the top level (`store.xp`, not
  `store.state.xp`). The declaration is resolved by object identity in `defineMod`, not exposed as a property —
  removing both the `.state` hop and the author-facing `.declaration`.
- **`Ref<T>` / `ComputedRef<T>` rename.** Rename `WritableStateRef<T>` → `Ref<T>` and `ReadonlyStateRef<T>` →
  `ComputedRef<T>`, keyed on the existing per-ref writable capability (not owner). A writable engine slot
  (`ui.textEntry`) types as `Ref`; readonly engine and mod slots as `ComputedRef`/`Ref` per their catalog flag.
- **`readState(ref)` read helper.** Lifts any state ref — engine or store — into the shipped `NumberRef` /
  `BoolRef` fluent algebra (type-directed by the ref's value type). A noun-selects-state helper, not a `.get()`.
- **Expression-algebra unification.** Export or repackage the private `numberRef`/`boolRef` adapters so a
  `runtime.*` `RuntimeValue` and a state read both compose with impact facts and satisfy `GatedEffect.when`.
- **`set(writableRef, expr)` general write.** Writes an arbitrary expression to a store slot, lowering to a new
  `slot.set` wire primitive and Rust binder arm that reuses `bind_number_write`.
- **`when(cond, effects)` sugar** over the shipped `GatedEffect` object literal.
- **Luau twins** for every new surface; regenerate and commit `sdk/types/postretro.d.{ts,luau}`; migrate
  `content/` call sites; cross-runtime parity tests.
- **Ref shaped owner-ready.** The ref keeps its `{slot}` identity such that an owner key is additive — the
  deferral contract with E16 (below).

### Out of scope

- **Per-owner `.byPlayer` / owner-addressed access.** `E16--per-player-currency` owns it. Unbuilt today (zero
  `sdk/lib/` hits; `slot.add` rejects any target, `impact_policy.rs:497`), and its read leg is an open question
  even on paper. This spec only guarantees the ref is owner-ready; it builds no owner path.
- **Widening the engine IR evaluator vocabulary.** `slot.set` reuses the shipped binder; no new IR node.
- **The component-local presentation-cell model** (`ui.createLocalState().cells.*.get()/.set()`). Ephemeral,
  instance-scoped state on a different lifecycle (G1-owned). Reconciling it with the authoritative `{slot}`
  model is a separate concern, not pulled in here.
- **Wire slot-name renames.** The retained wire stays dotted-name based (`scripting.md:151`).
- **The `slot.add` additive primitive.** Kept as the additive shorthand; not retired.
- **UI-reaction `updateState` unification.** Whether `set` also subsumes the UI-reaction `updateState` (a
  different dispatch context and wire path) is an open question (below), not a decided in-scope change.
- **Persistence, replication, and the `setState` opcode.** Unchanged.

## Direction

**Problem.** The scripting layer grew three non-converging dialects for referencing and expressing state —
engine-state nouns-plus-helpers, `runtime.*` builders, and impact-policy fluent methods — that do not
interoperate (`GatedEffect.when` needs a `BoolRef`; `runtime.*` yields an unregistered `RuntimeValue`; the
bridging adapters are unexported, `data_script.ts:283-320`). The `store.state.key` hop and the phantom
readonly/writable brand are the visible symptoms; the cause is the absence of one reference-and-expression model
across engine, store, and impact state. This is the "dialect convergence" problem `E16` and
`descriptor-identity-and-naming-sugar` both named and deferred to "its own spec."

**Placement.** SDK authoring surface (`sdk/lib/**` + the generated typedefs), not the engine. The shipped IR
evaluator and `EntityScope` already resolve store-slot reads by name (`impact_policy.rs:484-494`) and bind
arbitrary expressions to a slot output (`bind_number_write`, `:534-544`); the gaps are surface-side. Placing
this in the engine would widen the closed IR evaluator, which the surface deliberately avoids
(`data_script.ts:169-171`: "Impact policies use the shipped, closed runtime IR without widening its evaluator
vocabulary"). The one engine touch — the `slot.set` arm — reuses the existing binder and adds no IR node, so it
respects that line: a new primitive dispatch that lowers to the same binder is not a vocabulary widening.

**Prior commitments.**
- "Nouns select state. Helpers describe how a reference is used. No `.get()`/`.set()`" (`scripting.md:145`).
  Honored: reads are the helper `readState(ref)` returning a `NumberRef` — the exact shape `TargetHandle.state`
  already ships (`data_script.ts:231`) — not arithmetic or a getter bolted onto the ref. `set(ref, expr)` is a
  free write constructor, not a `.set()` method on the ref.
- Closed IR evaluator, not widened (`data_script.ts:169-171`). Honored — see Placement.
- Behavioral twins are "module IDs and export vocabulary, not syntax" (`scripting.md:243-249`). The binding-name
  sugar stays TS-only; Luau spells boolean composition `ref["and"](…)` because `and`/`or`/`not` are Luau
  keywords (`data_script.luau:237-239`). Both are sanctioned syntax asymmetries, pre-existing.
- The binding-name sugar just shipped (`descriptor-identity-and-naming-sugar`). Preserved: it injects the
  namespace argument in the `crates/script-compiler` pass, orthogonal to the store's return shape.
- The retained wire is dotted-name based (`scripting.md:151`). Honored — no wire slot-name changes; the flatten
  and rename lower to byte-identical descriptors (Invariants).
- `E16--per-player-currency` owns per-owner state. This spec diverges from nothing there; it defers `.byPlayer`
  and guarantees owner-readiness so E16 bolts on rather than re-cuts.

**Alternatives rejected.**
- *Narrow `.state`-hop fix only.* Fixes the symptom, leaves the three dialects. `myXp.plus(bonus)` stays
  impossible and a policy still cannot read engine state; the convergence would re-touch the same surface. The
  user's own investigation and the E16 scratchpad locate the cause in the dialect split, not the hop.
- *Free-combinator flavor* (`add(read(x), y)`, `gt(a, b)` as free functions). Too dense, and it discards the
  shipped fluent `NumberRef`/`BoolRef` algebra that already works byte-identically in both runtimes.
- *Literal Vue with live `.value` runtime reactivity.* Impossible — the authoring VM drops; there is no runtime
  to hold reactive cells. This boundary is what forces `when` ≠ `if` and read-as-node, and it is why the design
  is the *compile-to-IR shadow* of Vue, not Vue.
- *Arithmetic/`.set()` methods directly on the store ref* (the withdrawn E16 Variant C(b)). Rejected as the
  noun/helper-boundary violation `scripting.md:145` forbids; `readState`/`set` keep verbs off the ref.

## Acceptance criteria

- [ ] A store slot is referenced as `store.key` (e.g. `puzzles.northHeld`) — no `.state` hop — and this ref
  binds a widget, feeds `readState`, and feeds `set`.
- [ ] `defineMod({ stores: [store] })` commits the store's declaration; a mod-init that returns the store
  handle (not a `.declaration` property) registers its slots in the slot table.
- [ ] A store slot named `declaration` or `state` declares and resolves without collision (identity-resolved
  declaration, not a reserved property name).
- [ ] An unreturned `defineStore` result commits no slots (parity with today's discard-on-VM-drop).
- [ ] `readState(store.numberSlot)` yields a `NumberRef`; `readState(store.boolSlot)` yields a `BoolRef`; each
  composes with the fluent algebra (`.plus`, `.gt`, `.and`, …).
- [ ] `readState(getGameState().player.health)` (an engine `ComputedRef`) yields a `NumberRef` usable in the
  same expression as an impact fact and a store read.
- [ ] `set(store.writableNumberSlot, expr)` writes the evaluated expression to the slot; a readonly ref
  (`ComputedRef`) passed to `set` is a TypeScript type error.
- [ ] `set(store.xp, readState(store.xp).plus(1))` and the shipped `slot(store.xp).add(1)` produce the same
  stored value when evaluated against an `EntityScope` seeded with a known `store.xp` input.
- [ ] The `slot.set` primitive rejects a present target with a diagnostic, mirroring `slot.add`
  (`impact_policy.rs:497`), and binds through `bind_number_write` (a non-numeric expression is rejected).
- [ ] `when(cond, effects)` guards its effects on the deferred `BoolRef`; an effect list with no `when` still
  runs unconditionally. An author-named `BoolRef` const passed to `when` behaves identically to an inline one.
- [ ] A `runtime.*` value (`RuntimeValue`) can be handed to `when` and to the fluent algebra without a WeakMap
  miss — the three-dialect bridge is closed.
- [ ] `Ref<T>` is writable and `ComputedRef<T>` is readonly at the type level; `getGameState().ui.textEntry`
  types as `Ref<string>` (writable engine slot) while `getGameState().player.health` types as
  `ComputedRef<number>`.
- [ ] Every new TS surface has a Luau twin producing byte-identical wire; the Luau boolean path is documented
  as `ref["and"](…)` (keyword collision).
- [ ] The converged surface lowers to byte-identical wire descriptors as the pre-convergence surface for
  equivalent authoring (golden comparison over the migrated `content/dev` scripts) — except the newly added
  `slot.set` primitive.
- [ ] `cargo run -p postretro --bin gen-script-types` produces no diff against committed
  `sdk/types/postretro.d.{ts,luau}`; the drift-detection test is green.
- [ ] Migrated `content/dev` scripts (`run-counter.ts`, `coop-two-button-puzzles.ts`, `start-script.ts`,
  `hud.ts`, `typed-handles-fixture.ts`) compile and load on a running engine.
- [ ] **Review gate:** every ref consumer reads only `.slot` and tolerates additional fields, so an owner key
  is additive — no consumer does an exhaustive-shape check that a `{slot, owner}` ref would fail.

## Tasks

### Task 1: Expression bridge + `slot.set` — thin slice

Falsifies the load-bearing assumption that a store slot is readable and generally writable in one expression on
the shipped evaluator, engine-minimal. Add a `slot.set` variant to `ImpactEffectWire` (`sdk/lib/data_script.ts`,
beside the `slot.add` variant at `:212`): `{ primitive: "slot.set"; args: { slot: string; value: RuntimeValue } }`.
Add a Rust match arm in `bind_effect` (`crates/postretro/src/impact_policy.rs`, beside `slot.add` at `:484`):
`"slot.set" if target.is_none() => { let slot = required_string(args, "slot", …)?; let value = args.get("value")…?;
bind_number_write(slot.to_string(), value, scope).map(BoundEffect::Write) }` and a target-present rejection arm
mirroring `:497`. This reuses the shipped binder (`:534-544`) — no new `IrNode` variant. Add the SDK surface in
`data_script.ts`: `readState(ref): NumberRef` / `readState(ref): BoolRef` (type-directed) emitting
`{ op: "input", name: ref.slot }` wrapped via the existing private `numberRef`/`boolRef`
(`data_script.ts:287, 306`); and `set(ref: WritableStateRef<number>, value: NumberValue): Effect` emitting the
`slot.set` wire. Prove end to end: a Rust test seeds an `EntityScope` with a `store.xp` input, evaluates the
descriptor from `set(store.xp, readState(store.xp).plus(1))`, and asserts the same stored result as
`slot(store.xp).add(1)` — mirror the `breakable_threshold` harness (`impact_policy.rs:813-861`). Uses the
pre-rename type names (`WritableStateRef`); Task 2 renames.

### Task 2: `Ref` / `ComputedRef` rename

Behavior-preserving type rename, keyed on the per-ref writable capability, not owner. Rename
`WritableStateRef<T>` → `Ref<T>` and `ReadonlyStateRef<T>` → `ComputedRef<T>` at the definition
(`sdk/lib/ui/widgets.ts:47-55`) and every reference across `sdk/lib/**` — enumerate: `widgets.ts`,
`ui/state.ts`, `ui/reactions.ts`, `ui/tree.ts`, `game_state.ts`, `data_script.ts` (the `slot()` signature at
`:371` and `StoreDefinition`), plus the typedef templates `crates/scripting-core/src/typedef/templates/
sdk_lib.d.ts` and `sdk_lib.luau`. The runtime `{slot}` shape and the phantom brands are unchanged; only the
exported type names change. Engine-catalog slots keep their per-slot capability — `ui.textEntry` stays writable
(`Ref<string>`), `player.*`/`screen.*` stay readonly (`ComputedRef`) — so the writable/readonly axis is per-ref,
absorbing the writable-engine-slot exception rather than collapsing onto engine-vs-mod. This also removes the
phantom-brand silent-no-op: a `set`/write against a `ComputedRef` is now a type error at every author site.
Do not regenerate committed typedefs here (Task 5 owns the single regen).

### Task 3: Store surface flatten

Remove the `.state` hop and the author-facing `.declaration`. Change `defineStore` (`sdk/lib/data_script.ts`,
`StoreDefinition` at `:152`) to return a frozen object whose enumerable keys are the slot refs
(`{ [K in keyof S]: Ref<T> | ComputedRef<T> }` per each slot's `readonly` flag), and register the object →
`StoreDeclaration` mapping in a new module-level identity `WeakMap` (mirror `numberNodes`/`boolNodes` at
`:272-273`). Extend `defineMod` (`data_script.ts:701`, currently `return config`) to walk `config.stores` and
replace each entry that is a registered store handle with its declaration data (`{ namespace, schema }` — the
shape `drain_store_declarations_js` expects, `crates/scripting-core/src/store_bridge.rs:225`); a non-store entry
passes through unchanged. Mirror both changes in the Luau twins (`sdk/lib/data_script.luau` `defineStore` and
`defineMod` at `:918`). Migrate the `content/dev` store authoring this breaks so the surface stays loadable:
`stores: [store.declaration]` → `stores: [store]` and `store.state.key` → `store.key` in `start-script.ts:63`,
`run-counter.ts`, `coop-two-button-puzzles.ts`, `typed-handles-fixture.ts`. The binding-name sugar
(`crates/script-compiler`) is untouched — it injects the namespace argument, independent of the return shape.
Consumes the renamed types from Task 2.

### Task 4: Expression-algebra unification + `when`

Close the three-dialect bridge so one algebra spans engine reads, store reads, `runtime.*` values, and impact
facts. Repackage the private `numberRef`/`boolRef` adapters (`sdk/lib/data_script.ts:287, 306`) so a
`RuntimeValue` — from `runtime.*` or from `readState` — lifts into the fluent `NumberRef`/`BoolRef` algebra and
registers in the `numberNodes`/`boolNodes` WeakMaps (`:272-273`), resolving the `boolNode(runtimeValue)`
WeakMap-miss (`:283-284`) that today blocks a `runtime.*` value from `GatedEffect.when`. Prefer a
`readState`/`fromRuntime` helper family over exporting the raw adapters, to keep the raw-node privacy the comment
at `:169-171` relies on. Add `when(cond: BoolRef, effects: readonly Effect[]): GatedEffect` returning
`{ when: cond, do: effects }` (the shipped shape, `:218`). Mirror all of it in `data_script.luau`, and document
the Luau boolean spelling `ref["and"](ref, other)` in the SDK docs, since `when`'s condition path leans on
boolean composition and `and`/`or`/`not` are Luau keywords (`.luau:237-239`). Consumes Task 1's `readState` and
Task 2's renamed types; shares `data_script.ts` with Task 3, so sequence after it.

### Task 5: Typedef regen, docs, content sweep, parity tests

Regenerate and commit `sdk/types/postretro.d.{ts,luau}` via `cargo run -p postretro --bin gen-script-types`;
confirm the drift-detection test (`committed_sdk_types_match_current_registry`) is green. Complete the
`content/dev` migration beyond Task 3's store edits — `hud.ts` and any consumer using the old ref idioms —
and rebuild any committed `.js`. Update `scripting.md` §5 to the converged surface: `store.key`, `readState`,
`set`, `when`, and the `Ref`/`ComputedRef` vocabulary, removing `.state`/`.declaration` examples. Add
cross-runtime parity tests asserting byte-identical wire for `readState`, `set`/`slot.set`, `when`, the
flattened store, and the `runtime.*` bridge. Consumes Tasks 2, 3, 4.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies the expression-bridge / `slot.set` boundary assumption
end to end before any fan-out.
**Phase 2 (sequential):** Task 2 — the `Ref`/`ComputedRef` rename underlies every later signature.
**Phase 3 (sequential):** Task 3 — store flatten; consumes the renamed types.
**Phase 4 (sequential):** Task 4 — algebra unification + `when`; shares `data_script.ts` with Task 3.
**Phase 5 (sequential):** Task 5 — regen, docs, content sweep, parity tests; consumes 2–4.

Concurrency is limited by the shared hub file (`data_script.ts`) and the single committed-typedef output, so the
phases run largely linear. An optional split of `data_script.ts`'s IR-algebra section from its
declaration-builder section (see `research.md`) would let Tasks 3 and 4 run concurrently; not gated.

## Boundary inventory

Rust ↔ wire (JSON) ↔ TS ↔ Luau. No FGD surface. Rust snake_case; wire/JS/Luau camelCase. Existing surfaces
unchanged; new names below.

| Name | Rust | Wire / serde | TS | Luau |
|---|---|---|---|---|
| general slot write | `bind_effect` arm `"slot.set"` → `bind_number_write` | `{ primitive: "slot.set", args: { slot, value } }` | `set(ref, value)` | `set(ref, value)` |
| state read (into expr) | n/a (emits `{op:"input", name}`) | `{ op: "input", name: "<dotted.slot>" }` | `readState(ref)` | `readState(ref)` |
| effect guard sugar | n/a (existing `GatedEffect`) | `{ when, do }` (unchanged) | `when(cond, effects)` | `when(cond, effects)` |
| writable ref type | `WritableStateRef` (internal) | `{ slot: "<dotted>" }` (unchanged) | `Ref<T>` | `Ref<T>` |
| readonly ref type | `ReadonlyStateRef` (internal) | `{ slot: "<dotted>" }` (unchanged) | `ComputedRef<T>` | `ComputedRef<T>` |
| store handle | n/a | `stores[]` = `{ namespace, schema }` (unchanged) | `defineStore(...)` → slots top-level | same |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Converged surface lowers to byte-identical wire as the pre-convergence surface (except `slot.set`) | Tasks 2, 3, 4 (surface-only changes) | `defineMod` store resolution must emit the same `{namespace, schema}`; the flatten must yield the same `{slot}` refs; the rename must not touch runtime shape | AC "byte-identical wire", golden over `content/dev`; drift test |
| `slot.set` reuses the shipped binder — no new `IrNode`, no evaluator vocabulary widening | Task 1 (`bind_number_write` reuse) | any temptation to add an IR node for a general write | AC "binds through `bind_number_write`"; `research.md` placement |
| Ref is owner-ready — an owner key is additive to `{slot}` | Task 2 (per-ref `{slot}` shape) | a consumer doing an exhaustive-shape check would reject `{slot, owner}` | AC review gate (every consumer reads only `.slot`) |

## Script syntax examples

**Before (shipped).**
```ts
// Proposed design — contrast only
const progression = defineStore({ xp: { type: "number", default: 0, persist: true }, teamKills: { type: "number", default: 0 } });
export function setupMod() { return defineMod({ /* … */ stores: [progression.declaration] }); }

const reward = defineImpactEvent("dev:reward", { tag: "enemy" }, (impact) => {
  const isKill = impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0));
  const bonus  = impact.target.healthAfter.le(-40).select(50, 25);
  return [{ when: isKill, do: [
    slot(progression.state.xp).add(bonus),        // .state hop; only the additive door
    slot(progression.state.teamKills).add(1),
  ]}];
});
```

**After (this spec).**
```ts
// Proposed design
const progression = defineStore({ xp: { type: "number", default: 0, persist: true }, teamKills: { type: "number", default: 0 } });
export function setupMod() { return defineMod({ /* … */ stores: [progression] }); }   // handle, not .declaration

const reward = defineImpactEvent("dev:reward", { tag: "enemy" }, (impact) => {
  const isKill = impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0));   // BoolRef (computed)
  const bonus  = impact.target.healthAfter.le(-40).select(50, 25);
  const xp     = readState(progression.xp);       // Ref → NumberRef, signposted (no .state hop)
  return [ when(isKill, [
    set(progression.xp, xp.plus(bonus)),          // general expression write
    set(progression.teamKills, readState(progression.teamKills).plus(1)),
  ])];
});
```

`readState(getGameState().player.health)` lifts an engine `ComputedRef` into the same algebra, so a policy can
read engine state, store state, and impact facts in one expression — the incoherence this spec removes. Luau is
the behavioral twin, with the explicit-namespace `defineStore("progression", …)` form and `cond["and"](cond,
other)` boolean spelling.

## Open questions

- **Does `set` also subsume the UI-reaction `updateState`?** `updateState(ref, value)` (`sdk/lib/ui/reactions.ts`)
  emits a `setState` reaction descriptor in the UI-reaction dispatch context, distinct from the impact-effect
  `slot.set` this spec adds. Unifying the two author-facing verbs is desirable but crosses dispatch contexts and
  wire paths; verify the descriptors before claiming it. Owner: this spec's reviewer, or a follow-up — do not
  fold `updateState` into `set` without grounding the wire shapes.
- **Is `readState` the right name?** It fits the `bindState`/`stateEquals`/`updateState` helper family and does
  not imply a live read (it emits an input node). Alternatives (`asExpr`, `expr`) were weighed against family
  consistency. Pinned to `readState` unless review prefers otherwise.
