# Scripting State Convergence

## Goal

One reference-and-expression model for durable game state across engine observations, mod stores, and impact
policies — replacing three non-converging authoring dialects. Removes the `store.state.key` hop; gives a store
slot a read (`read`), an absolute write (`set`), and a read-modify-write (`update`); and unifies
expression-building under the shipped fluent IR algebra so a policy can read engine state, store state, and
impact facts in one expression. The governing mental model is "the compile-to-IR shadow of Vue" (`research.md`):
`Ref`/`ComputedRef` sources, named derived expressions, and a deferred `when` guard — evaluated in Rust, not JS.

Engine-minimal: reuses the shipped IR evaluator and binder. Net engine churn is one arm added, two removed:
Task 1 adds a `slot.set` write-primitive match arm that reuses the existing `bind_number_write`; Task 5 removes
the retired `slot.add` arm and its target-rejection arm. No new IR node type, no evaluator vocabulary widening.

## Scope

### In scope

- **Store surface flatten.** `defineStore` returns slot refs at the top level (`store.xp`, not
  `store.state.xp`). The declaration is resolved by object identity in `defineMod`, not exposed as a property —
  removing both the `.state` hop and the author-facing `.declaration`.
- **`Ref<T>` / `ComputedRef<T>` rename.** Rename `WritableStateRef<T>` → `Ref<T>` and `ReadonlyStateRef<T>` →
  `ComputedRef<T>`, keyed on the existing per-ref writable capability (not owner). A writable engine slot
  (`ui.textEntry`) types as `Ref`; readonly engine and mod slots as `ComputedRef`/`Ref` per their catalog flag.
- **`read(ref)` read helper.** Lifts any state ref — engine or store — into the shipped `NumberRef` /
  `BoolRef` fluent algebra, type-directed by the ref's `kind` field: a build-time-only tag (never serialized)
  stamped onto the ref by `defineStore` and the engine catalog, holding the slot's declared value type.
  `read` consults `ref.kind` to pick `numberRef` or `boolRef` and register the result in the matching WeakMap;
  the wire is unaffected and consumers still read only `.slot`. A noun-selects-state helper, not a `.get()`. The
  short name matches its `set`/`update` siblings; the `*State` suffix is reserved for the UI/reaction subsystem
  helpers (`bindState`/`stateEquals`/`updateState`/`onStateCrossing`). Scoped to reading a slot into a condition
  or a *different* slot's expression; the read-modify-write of one slot is `update` (below), which names the
  slot once.
- **Expression-algebra unification.** Export or repackage the private `numberRef`/`boolRef` adapters so a
  `runtime.*` `RuntimeValue` and a state read both compose with impact facts and satisfy `GatedEffect.when`.
- **`set(writableRef, expr)` absolute write.** Writes an arbitrary expression to a store slot; performs no
  *implicit* self-read (unlike `update`, below) — an author can still pass a `read(ref)` into `set`
  explicitly, which is legal, just non-idiomatic. Lowers to a new `slot.set` wire primitive and Rust binder arm
  that reuses `bind_number_write`.
- **`update(writableRef, cur => expr)` read-modify-write.** The functional-updater form: the callback receives
  the slot's current value as a `NumberRef` (`cur`, exactly `read(ref)`) and returns the new expression, so
  the slot is named once. Lowers to `slot.set` — the same wire as `set`, with `cur` inlined as the input leaf.
  Supersedes the additive-only `slot(ref).add(delta)`, which is retired.
- **Retire `slot(ref).add()` / `slot.add`.** The additive builder and its wire primitive are removed once
  content migrates to `update`/`set` — one write vocabulary, not a general door plus a redundant additive one.
- **`when(cond, effects)` sugar** over the shipped `GatedEffect` object literal.
- **Luau twins** for every new surface; regenerate and commit `sdk/types/postretro.d.{ts,luau}`; migrate
  `content/` call sites; cross-runtime parity tests.
- **Ref kept minimal for E16.** The ref keeps its `{slot}` identity and consumers read only `.slot`, so E16's
  later owner-addressing has room to work. Best-effort discipline, not a contract — E16 owns the owner shape
  (below).

### Out of scope

- **Per-owner `.byPlayer` / owner-addressed access.** `E16--per-player-currency` owns it. Unbuilt today (zero
  `sdk/lib/` hits; `slot.add` rejects any target, `impact_policy.rs:497`), and its read leg is an open question
  even on paper. This spec builds no owner path and makes no shape guarantee for it: E16's draft currently uses
  a `ref.byPlayer(token)` method and its placement (ref vs store handle) is unsettled — E16 owns that decision
  and the own-property-enumeration hazard a method on the ref carries.
- **Widening the engine IR evaluator vocabulary.** `slot.set` reuses the shipped binder; no new IR node.
- **The component-local presentation-cell model** (`ui.createLocalState().cells.*.get()/.set()`). Ephemeral,
  instance-scoped state on a different lifecycle (G1-owned). Reconciling it with the authoritative `{slot}`
  model is a separate concern, not pulled in here.
- **Wire slot-name renames.** The retained wire stays dotted-name based (`scripting.md:151`).
- **UI-reaction `updateState` — decided KEEP-SEPARATE.** Not a literal-vs-expression split — `updateState`
  already accepts full IR via `SystemReactionIrBindings`, so it is not literals-only. Four durable separators
  instead: different wire descriptor (`setState` reaction descriptor vs the `slot.set` effect), different scope
  type (`DispatchScope` seeded with `@rising` ephemeral dispatch inputs vs `EntityScope` seeded with the frozen
  `@impact.*`/`@state.*` snapshot), different frame stage (the app-frame system-command drain after the
  post-tick event drains vs synchronously inside the fixed simulation tick), and different trigger (a named UI
  event — button/crossing/slider — vs per-damage-hit). They share no engine slot-write primitive, so a single
  author verb would either hide that difference or drag one subsystem's timing/scope into the other. `set`/
  `update` are the impact-effect write twins, deliberately named apart from the UI-reaction `updateState` — the
  reason the `*State` suffix marks UI/reaction-subsystem membership, and what makes the `read` naming (Scope,
  above) coherent.
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
vocabulary"). The engine touches are the `slot.set` arm added in Task 1 and the `slot.add` arm removed in Task 5
— both reuse the existing binder and add no IR node, so neither is a vocabulary widening.

**Frame and ordering.** A policy fires in GameLogic synchronously after each in-tick damage hit; `read` of
engine or store state and impact facts all resolve against one snapshot seeded at that fire (via
`seed_impact_from_registry`), not a live end-of-tick read. Same-slot writes across independent events or groups
follow the same frozen-read, last-writer-wins rule as within one `do:` list; an override replaces (evicts) its
base rather than composing with it (Orderings, below).

**Prior commitments.**
- "Nouns select state. Helpers describe how a reference is used. No `.get()`/`.set()`" (`scripting.md:145`).
  Honored: reads are the helper `read(ref)` returning a `NumberRef` — the exact shape `TargetHandle.state`
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
- `E16--per-player-currency` owns per-owner state. This spec defers `.byPlayer` and keeps the ref minimal
  (consumers read only `.slot`) so E16 has room to work — a best-effort discipline, not a guarantee, since
  E16's owner shape is unsettled (ref-method vs store-handle) and E16 owns whichever it picks. One deliberate
  foreclosure: flattening the store handle to *be* the slot-ref map means an author method (`byPlayer` or any
  other) cannot live on the store handle without colliding with a slot name — so E16 must place owner-addressing
  on the ref, not the handle. Stated so the foreclosure is explicit, and because it settles E16's open
  ref-vs-handle placement question by construction.

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
  noun/helper-boundary violation `scripting.md:145` forbids; `read`/`set`/`update` keep verbs off the ref.
- *Read-modify-write via `set(ref, read(ref).expr)`.* The obvious general form, but it names the slot
  twice per RMW — the duplication that made the shipped `slot(ref).add()` terser than its replacement. `update`
  binds the read as `cur` so the slot is named once, and reduces to `set` for the absolute-write case.
- *Engine-native type names instead of Vue's (`StateRef`/`ReadonlyStateRef`).* Rejected, narrowly. Vue's
  `Ref`/`ComputedRef` do carry the mis-expectation the design cannot honor (no live `.value`, `when ≠ if`) — but
  the names are *inferred*, never written by authors (a modder writes `const xp = read(store.xp)`, never
  `Ref<number>`), so the risk lives only in the mental model and docs, not at any call site. Against that, the
  Vue frame is the clearest available account of the writable/readonly split (`Ref` mutates, `ComputedRef` is
  derived-readonly). Keep the names; the docs state plainly that these are compile-time descriptors with no live
  read. (If review still judges the frame misleading, the fallback is engine-native names — a mechanical rename.)

## Acceptance criteria

- [ ] A store slot is referenced as `store.key` (e.g. `puzzles.northHeld`) — no `.state` hop — and this ref
  binds a widget, feeds `read`, and feeds `set`.
- [ ] `defineMod({ stores: [store] })` commits the store's declaration; a mod-init that returns the store
  handle (not a `.declaration` property) registers its slots in the slot table.
- [ ] A store slot named `declaration` or `state` declares and resolves without collision (identity-resolved
  declaration, not a reserved property name).
- [ ] An unreturned `defineStore` result commits no slots (parity with today's discard-on-VM-drop).
- [ ] `read(store.numberSlot)` yields a `NumberRef`; `read(store.boolSlot)` yields a `BoolRef`; each
  composes with the fluent algebra (`.plus`, `.gt`, `.and`, …).
- [ ] `read(getGameState().player.health)` (an engine `ComputedRef`) yields a `NumberRef` usable in the
  same expression as an impact fact and a store read; inside an impact expression this resolves via the
  store-slot path only if `player.health` is present in the slot table at policy-bind time, so the test must
  seed it.
- [ ] `set(store.writableNumberSlot, expr)` writes the evaluated expression to the slot, performing no implicit
  self-read — runtime-testable: `set(xp, 5)` stores `5`, not `xp+5`. A readonly ref (`ComputedRef`) passed to
  `set` or `update` is a TypeScript type error — a `tsc`/review gate via `@ts-expect-error` type-test fixtures
  (no `tsc` CI), not a `cargo test`.
- [ ] `update(store.xp, (xp) => xp.plus(1))` names the slot once and performs a read-modify-write; it and the
  pre-retirement `slot(store.xp).add(1)` produce the same stored value against an `EntityScope` seeded with a
  known `store.xp` input. The callback's `cur` argument is a `NumberRef` equal to `read(store.xp)`. Task 4
  asserts this directly on the `update` form's bound wire (not only via a `set` surrogate), so the equivalence
  has a verifying artifact.
- [ ] The `slot.set` primitive rejects a present target with a diagnostic (mirroring the pre-retirement
  `slot.add` rejection at `impact_policy.rs:497`) and binds through `bind_number_write` (a non-numeric
  expression is rejected).
- [ ] No author-facing `slot(ref)` / `slot.add` builder remains in `sdk/lib`, and the `slot.add` wire arm and
  its tests are removed from `impact_policy.rs`; a grep gate over `content/` finds no `slot.add` call — a
  regression guard against reintroduction, not proof of a migration, since no `content/` site ever emitted
  `slot(ref).add()` (content writes state via UI-reaction `updateState` and `accumulate` schema hooks).
- [ ] `when(cond, effects)` guards its effects on the deferred `BoolRef`; an effect list with no `when` still
  runs unconditionally. An author-named `BoolRef` const passed to `when` behaves identically to an inline one.
- [ ] A `runtime.*` value (`RuntimeValue`) can be handed to `when` and to the fluent algebra without a WeakMap
  miss — the three-dialect bridge is closed.
- [ ] `Ref<T>` is writable and `ComputedRef<T>` is readonly at the type level; `getGameState().ui.textEntry`
  types as `Ref<string>` (writable engine slot) while `getGameState().player.health` types as
  `ComputedRef<number>` — these are `tsc`/review gates via `@ts-expect-error` type-test fixtures (no `tsc` CI),
  not a `cargo test`. Both carry a build-time `kind` field alongside `.slot`, never serialized — a grep/parity
  gate: only `.slot` reaches the wire. The writable/readonly split stays keyed on capability, not on `kind`.
- [ ] Every new TS surface has a Luau twin producing byte-identical wire; the Luau boolean path is documented
  as `ref["and"](…)` (keyword collision).
- [ ] The converged surface lowers to the same wire as the pre-convergence surface for equivalent authoring.
  There is no `content/dev` pre-vs-post wire-golden harness in-tree, and Task 5's cross-runtime parity tests
  build TS↔Luau parity — a different axis — so this is verified by the coverage that actually exists: Task 5's
  TS↔Luau byte-identical parity tests, the typedef drift-detection test, and targeted equality tests (the store
  flatten emits the same `{namespace, schema}`; the `Ref`/`ComputedRef` rename doesn't touch the runtime
  `{slot}` shape; Task 1's P1 stored-result equality) — except the newly added `slot.set` primitive and the
  Rust `breakable_threshold` test's `slot.add`→`update` conversion (the only real `slot.add` emitter; no
  `content/` site emits it today), which are semantically equivalent (same stored result), not byte-identical.
- [ ] `cargo run -p postretro --bin gen-script-types` produces no diff against committed
  `sdk/types/postretro.d.{ts,luau}`; the drift-detection test is green.
- [ ] Migrated `content/dev` scripts (`run-counter.ts`, `coop-two-button-puzzles.ts`, `start-script.ts`,
  `hud.ts`, `typed-handles-fixture.ts`) compile and load on a running engine — `scripts-build` compiles by
  stripping types (no type-check), so this "compile" doesn't catch a TypeScript error; a `@ts-expect-error`
  fixture like `typed-handles-fixture.ts` still "compiles" under it.
- [ ] **Review gate:** every ref consumer reads only `.slot` — no consumer does an exhaustive-shape check — so
  E16's later owner-addressing has room to extend the ref. The ref carries `.slot` plus a build-time `kind`
  (SDK-only, never serialized); consumers tolerate the extra field and only `.slot` reaches the wire. (E16 owns
  its owner shape and any own-property-enumeration hazard; this gate only keeps consumers from
  over-constraining it.)
- [ ] Every row in the Orderings pin table (P1–P13) is asserted by a test: Task 1 for P1–P6, Task 5 for P7–P13.
  P1 is a stored-result equality (`update` vs `slot.add` produce the same evaluated value; wire differs, no
  bound-program comparison — see Task 1). P12 and P13 need the seeded / two-fire harness named in Task 5, not
  the bare `breakable` seeding.
- [ ] A mod that returns `{ stores: [store] }` without calling `defineMod` fails store resolution — the raw
  slot-ref map lacks `namespace`/`schema`, so `defineMod` is load-bearing for flattened-store registration.

## Tasks

### Task 1: Expression bridge + `slot.set` — thin slice

Falsifies the load-bearing assumption that a store slot is readable and generally writable in one expression on
the shipped evaluator, engine-minimal. Add a `slot.set` variant to `ImpactEffectWire` (`sdk/lib/data_script.ts`,
beside the `slot.add` variant at `:212`): `{ primitive: "slot.set"; args: { slot: string; value: RuntimeValue } }`.
Add a Rust match arm in `bind_effect` (`crates/postretro/src/impact_policy.rs`, beside `slot.add` at `:484`):
`"slot.set" if target.is_none() => { let slot = required_string(args, "slot", …)?; let value = args.get("value")…?;
bind_number_write(slot.to_string(), value, scope).map(BoundEffect::Write) }` and a target-present rejection arm
mirroring `:497`. This reuses the shipped binder (`:534-544`) — no new `IrNode` variant.

Add the SDK surface in `data_script.ts`: state refs carry a build-time `kind` field — the slot's declared value
type, never serialized. Task 1 only *consumes* `kind`: `read(ref): NumberRef` / `read(ref): BoolRef`
reads `ref.kind` to pick `numberRef` or `boolRef`, emitting `{ op: "input", name: ref.slot }` and registering the
result in the matching WeakMap (`data_script.ts:287, 306`); and `set(ref: WritableStateRef<number>, value:
NumberValue): Effect` emits `args: { slot: ref.slot, value: numberNode(value) }`, mirroring the shipped
`slot.add` builder's `numberNode(delta)`. `kind`'s *production* belongs to later tasks — Task 2 stamps it on
engine-catalog refs, Task 3's `defineStore` stamps it on store refs — so no real ref carries `kind` until Task 3;
Task 1's `read`/`set` are not exercised by a real ref in Phase 1.

Task 1's Phase-1 proof is therefore the Rust wire-arm equality alone, not SDK authoring (the SDK builders can't
run in Rust yet): a Rust test hand-builds the `slot.set` wire JSON — the way `breakable_threshold`'s `slot_add()`
helper hand-builds `slot.add` JSON at `impact_policy.rs:688` — and seeds the slot table directly
(`ctx.slot_table.borrow_mut().insert(...)`). The `slot.set` value passes through the `bind_number_write` arm
directly, so `set(xp, {op:"add", a:{op:"input",name:"xp"}, b:1})` equals `slot.add(xp, 1)` by stored result — a
comparison of the *evaluated stored value*, not of the bound program or the wire (`slot.set` and `slot.add` are
different wire; `BoundProgram` exposes no equality/serialize). This and companion cases assert pin-table rows
P1–P6 (Orderings, below). If a TS-level `read`/`set` unit test is wanted in Phase 1, it must hand-construct
a `{ slot, kind }` object — no real ref exists yet; otherwise `read`/`set` are wire-proven only, until Task
3 supplies a real ref. Uses the pre-rename type names (`WritableStateRef`); Task 2 renames.

### Task 2: `Ref` / `ComputedRef` rename

Behavior-preserving type rename, keyed on the per-ref writable capability, not owner. Rename
`WritableStateRef<T>` → `Ref<T>` and `ReadonlyStateRef<T>` → `ComputedRef<T>` at the definition
(`sdk/lib/ui/widgets.ts:47-55`) and every reference — derived from a grep of `WritableStateRef|ReadonlyStateRef`
across `sdk/lib/**`: `widgets.ts`, `ui/state.ts`, `ui/reactions.ts`, `ui/tree.ts`, `data_script.ts` (the `slot()`
signature at `:371` and `StoreDefinition`), `prelude.ts` (the public type re-export barrel), and
`data_script.luau` (its own `ReadonlyStateRef`/`WritableStateRef` type declarations and `slot()` signature) —
plus the typedef templates `crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` and `sdk_lib.luau`.
`game_state.ts` references neither name (its types flow through the regenerated `GameStateRefs`) — no edit.
`sdk/lib/index.ts` and `sdk/lib/runtime.ts` reference the `StateRef` union alias by name, unchanged; only its
definition body is affected. Engine-catalog refs (`getGameState()`) carry `kind` from the catalog value type,
same as store refs. The runtime `{slot}` shape and the phantom brands are unchanged; only the exported type
names change. Engine-catalog slots keep their per-slot capability — `ui.textEntry` stays writable
(`Ref<string>`), `player.*`/`screen.*` stay readonly (`ComputedRef`) — so the writable/readonly axis is per-ref,
absorbing the writable-engine-slot exception rather than collapsing onto engine-vs-mod. This also removes the
phantom-brand silent-no-op: a `set`/write against a `ComputedRef` is now a type error at every author site. The
typedef generator's convenience alias `StateValue<T> = WritableStateRef<T>` (emitted literally in
`crates/scripting-core/src/typedef/ts.rs:46` and `luau.rs:42`, outside the grepped `sdk/lib/**` scope above)
rides this rename too — it must read `StateValue<T> = Ref<T>` post-regen, so pick it up in Task 5's typedef
regen; it's easy to miss since the grep above doesn't reach it. Do not regenerate committed typedefs here
(Task 5 owns the single regen).

### Task 3: Store surface flatten

Remove the `.state` hop and the author-facing `.declaration`. Change `defineStore` (`sdk/lib/data_script.ts`,
`StoreDefinition` at `:152`) to return a frozen object whose enumerable keys are the slot refs
(`{ [K in keyof S]: Ref<T> | ComputedRef<T> }` per each slot's `readonly` flag), each stamped with a `kind`
field from its schema `type`, and register the object → `StoreDeclaration` mapping in a new module-level
identity `WeakMap` (mirror `numberNodes`/`boolNodes` at `:272-273`). Extend `defineMod` (`data_script.ts:701`,
currently `return config`) to walk `config.stores` and replace each entry that is a registered store handle with
its declaration data (`{ namespace, schema }` — the shape `drain_store_declarations_js` expects,
`crates/scripting-core/src/store_bridge.rs:225`); a non-store entry passes through unchanged. `defineMod` is now
required for flattened-store registration: a mod that returns `{ stores: [store] }` without calling `defineMod`
fails store resolution, since the raw slot-ref map lacks `namespace`/`schema`; a mod that still passes
`.declaration` explicitly keeps working via pass-through. Mirror both changes in the Luau twins
(`sdk/lib/data_script.luau` `defineStore` and `defineMod` at `:918`). Migrate the `content/dev` store authoring
this breaks so the surface stays loadable:
`stores: [store.declaration]` → `stores: [store]` and `store.state.key` → `store.key` in `start-script.ts:63`,
`run-counter.ts`, `coop-two-button-puzzles.ts`, `typed-handles-fixture.ts`. A third transform is needed beside
those two: a bare `.declaration` *read* (e.g. `const _fixtureStoreDeclaration = opts.declaration;` in
`typed-handles-fixture.ts`) also migrates — the flatten moves `.declaration` off the store object into the
identity `WeakMap`, so nothing is left at that property. Applying only the two named transforms leaves this
read dangling and breaks compilation. `typed-handles-fixture.ts` is a `@ts-expect-error` review-gate fixture (no
`tsc` CI) — its type-error lines are a `tsc`/review gate, not a runtime concern. The binding-name sugar
(`crates/script-compiler`) is untouched — it injects the namespace argument, independent of the return shape.
Consumes the renamed types from Task 2.

### Task 4: Expression-algebra unification + `when`

Close the three-dialect bridge so one algebra spans engine reads, store reads, `runtime.*` values, and impact
facts. Repackage the private `numberRef`/`boolRef` adapters (`sdk/lib/data_script.ts:287, 306`) so a
`RuntimeValue` — from `runtime.*` or from `read` — lifts into the fluent `NumberRef`/`BoolRef` algebra and
registers in the `numberNodes`/`boolNodes` WeakMaps (`:272-273`), resolving the `boolNode(runtimeValue)`
WeakMap-miss (`:283-284`) that today blocks a `runtime.*` value from `GatedEffect.when`. Prefer a
`read`/`fromRuntime` helper family over exporting the raw adapters, to keep the raw-node privacy the comment
at `:169-171` relies on. Add `update(ref: Ref<number>, build: (cur: NumberRef) => NumberValue): Effect` — it
calls `build` with `read(ref)` and emits the returned expression through the `slot.set` wire from Task 1, so
the read-modify-write names the slot once; `update`/`set` supersede the author-facing `slot(ref)` builder, which
stays in place until Task 5 removes it (after content migrates and after `update` exists). Assert the `update`
form's bound wire directly — not only via a `set` surrogate — for AC8. Add
`when(cond: BoolRef, effects: readonly Effect[]): GatedEffect` returning `{ when: cond, do: effects }` (the
shipped shape, `:218`). Mirror all of it in `data_script.luau`, and
document the Luau boolean spelling `ref["and"](ref, other)` in the SDK docs, since `when`'s condition path leans
on boolean composition and `and`/`or`/`not` are Luau keywords (`.luau:237-239`). Consumes Task 1's `read`
and Task 2's renamed types; shares `data_script.ts` with Task 3, so sequence after it.

### Task 5: Typedef regen, docs, content sweep, parity tests

Retire `slot.add`, now that content/tests are its only consumers and `update` exists. `content/` emits no
impact-effect `slot(ref).add()` today (content writes state via UI-reaction `updateState` and `accumulate`
schema hooks) — the only real `slot.add` emitter is Rust-side: migrate the
`breakable_threshold_reads_pre_effect_state_snapshot` test and the `slot_add` test helper it uses
(`impact_policy.rs`) to emit the `slot.set` wire; move the helper with the test, since nothing else consumes it.
Retire or convert Task 1's `set`-vs-`slot.add` equality test to match. Then remove the author-facing `slot(ref)`
builder and its `NumberSlot` type from `data_script.ts` (`:251-253, 371-383`, moved here from Task 4); remove
the `slot.add` match arm from `bind_effect` and its target-rejection arm (`impact_policy.rs:484, 497`); remove
the `slot.add` variant from `ImpactEffectWire` (`data_script.ts:212`). In
`impact_effect_wire_rejects_raw_store_assignment_and_boolean_operands`, drop only the `slot.add` entry from the
shared numeric-operand-rejection loop, keeping its `setHealth`/`setState` cases, and remove the
`slot.add`-specific target-present rejection loop (the `for invalid_target in […]` block) in its entirety.
Complete the `content/dev` migration beyond Task 3's store edits — any consumer still on the old
`slot()`/`.state`/`.declaration` idioms. `hud.ts` needs no change: it reads only `getGameState()` catalog refs
via `bindState`/`stateEquals` (no `slot()`, `.state`, or `.declaration`), so its refs transparently gain `kind`
and nothing wire-affecting moves under it. Rebuild any committed `.js` — `scripts-build` compiles by stripping
types (no type-check), so this "compile" step doesn't catch a TypeScript error; a `@ts-expect-error` fixture
still "compiles" under it. Regenerate and commit `sdk/types/postretro.d.{ts,luau}` via
`cargo run -p postretro --bin gen-script-types`; confirm the drift-detection test
(`committed_sdk_types_match_current_registry`) is green. Update `scripting.md` §5 to the converged surface:
`store.key`, `read`, `set`, `update`, `when`, and the `Ref`/`ComputedRef` vocabulary, removing
`.state`/`.declaration` and `slot().add()` examples. Add cross-runtime parity tests asserting byte-identical
wire for `read`, `set`/`update`/`slot.set`, `when`, the flattened store, and the `runtime.*` bridge, plus
the ordering pin-table rows P7–P13 (Orderings, below). P12 and P13 need a purpose-built harness, not the bare
`breakable` one: P12 (a policy reading `getGameState().player.health` mid-tick) requires seeding `player.health`
into the slot table before the impact freezes — the `breakable` harness never populates it, so seed it
explicitly; P13 (a store-slot re-seed across two in-tick hits in one policy fire) extends the proven `@state`
re-seed pattern (`breakable_threshold_reads_pre_effect_state_snapshot`) to a store slot and needs a two-fire
test. Consumes Tasks 2, 3, 4.

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
unchanged; new names below. The ref itself carries `.slot` plus a build-time `kind` (SDK-only; not on the wire).

| Name | Rust | Wire / serde | TS | Luau |
|---|---|---|---|---|
| absolute slot write | `bind_effect` arm `"slot.set"` → `bind_number_write` | `{ primitive: "slot.set", args: { slot, value: numberNode(value) } }` | `set(ref, value)` | `set(ref, value)` |
| read-modify-write | same `"slot.set"` arm (`cur` = input leaf) | `{ primitive: "slot.set", args: { slot, value: numberNode(value) } }` | `update(ref, cur => expr)` | `update(ref, function(cur) … end)` |
| state read (into expr) | n/a (emits `{op:"input", name}`) | `{ op: "input", name: "<dotted.slot>" }` | `read(ref)` | `read(ref)` |
| effect guard sugar | n/a (existing `GatedEffect`) | `{ when, do }` (unchanged) | `when(cond, effects)` | `when(cond, effects)` |
| writable ref type | `WritableStateRef` (internal) | `{ slot: "<dotted>" }` (unchanged; SDK ref also carries build-time `kind`, not serialized) | `Ref<T>` | `Ref<T>` |
| readonly ref type | `ReadonlyStateRef` (internal) | `{ slot: "<dotted>" }` (unchanged; SDK ref also carries build-time `kind`, not serialized) | `ComputedRef<T>` | `ComputedRef<T>` |
| store handle | n/a | `stores[]` = `{ namespace, schema }` (unchanged) | `defineStore(...)` → slots top-level | same |
| ~~additive slot write~~ (retired) | `slot.add` arm removed (Task 5) | `slot.add` variant removed | `slot(ref).add()` removed | removed |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Converged surface lowers to byte-identical wire as the pre-convergence surface, except the new `slot.set` primitive and the Rust `breakable_threshold` test's `slot.add`→`update` conversion (the only real `slot.add` emitter; semantically equivalent, not byte-identical) | Tasks 2, 3, 4 (surface-only changes) | `defineMod` store resolution must emit the same `{namespace, schema}`; the flatten must yield the same `{slot}` refs; the rename must not touch runtime shape | Task 5's TS↔Luau parity tests + typedef drift test + targeted equality tests (`{namespace, schema}` unchanged; `{slot}` shape unchanged; Task 1's P1 stored-result equality) — no `content/dev` wire golden exists in-tree |
| `slot.set` reuses the shipped binder — no new `IrNode`, no evaluator vocabulary widening | Task 1 (`bind_number_write` reuse) | any temptation to add an IR node for a general write | AC "binds through `bind_number_write`"; `research.md` placement |
| All operands in one impact fire read the pre-fire frozen snapshot (plan-before-apply in `evaluate_dispatch`); `cur` in `update` never observes an earlier same-fire write; same-slot writes are last-writer-wins in do-list order, not accumulating | Shipped substrate (inherited, not changed by this spec) | any same-slot write across `do:` entries, events, or groups | Orderings pin table (below) |
| Ref consumers read only `.slot`; the ref also carries a build-time `kind` (never serialized) and may later carry an owner key (keeps room for E16 owner-addressing) | Task 2 (per-ref `{slot, kind}` shape) | a consumer doing an exhaustive-shape check would over-constrain E16's later ref extension | AC review gate |

## Orderings

Frozen = the pre-fire snapshot (plan-before-apply in `evaluate_dispatch`). All operands in one fire read it;
writes apply last-writer-wins in do-list order.

| # | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | single update ≡ single slot.add | do:[update(xp, x=>x.plus(1))] vs do:[slot.add(xp,1)], seed base | both ⇒ base+1; same stored result — wire differs (`slot.set` vs `slot.add`) |
| P2 | RMW aliasing (two updates, one slot) | do:[update(xp,x=>x.plus(1)), update(xp,x=>x.plus(2))], seed base | ⇒ base+2 (not base+3); both cur read frozen base |
| P3 | aliasing equivalence to legacy | do:[slot.add(xp,1), slot.add(xp,2)], seed base | ⇒ base+2; identical to P2 |
| P4 | set then update, one slot | do:[set(xp,5), update(xp,x=>x.plus(1))], seed base | ⇒ base+1 (not 6); set clobbered |
| P5 | update then set, one slot | do:[update(xp,x=>x.plus(1)), set(xp,5)], seed base | ⇒ 5; last write wins |
| P6 | two set, one slot | do:[set(xp,5), set(xp,7)], seed base | ⇒ 7 |
| P7 | cross-event distinct ids, same slot | A do:[update(xp,x=>x.plus(1))], B do:[set(xp,9)], B registered later, both match | ⇒ 9 |
| P8 | base+override, same id, same slot | base do:[set(xp,1)], override(tag) do:[set(xp,2)], both tags present | ⇒ 2; override evicts base |
| P9 | batching N=0 | do:[] | no write; wire byte-identical pre/post migration |
| P10 | when(cond,[]) empty guard | do:[when(false,[])] and when(true,[]) | cond evaluated, no write either way; wire {when,do:[]} |
| P11 | read of unproduced slot | when(read(store.fresh).ge(1), […]), fresh never written | reads slot default, no error |
| P12 | mid-tick engine read | read(getGameState().player.health) in a policy on an in-tick hit | fire-time frozen stored health (post-damage healthAfter), not end-of-tick |
| P13 | consecutive in-tick hits | same policy fires hit 1 then hit 2 in one tick, update(xp,x=>x.plus(1)) | hit 2 observes hit 1's write (re-seed per fire) ⇒ base+2 across the two fires |

Task 1 asserts P1–P6; Task 5 asserts P7–P13.

## Script syntax examples

**Before (shipped).**
```ts
// Proposed design — contrast only
const progression = defineStore({ xp: { type: "number", default: 0, persist: true }, teamKills: { type: "number", default: 0 } });
export function setupMod() { return defineMod({ /* … */ stores: [progression.declaration] }); }

const reward = defineImpactEvent({ tag: "enemy" }, (impact) => {   // id "reward" from the binding (naming sugar)
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

const reward = defineImpactEvent({ tag: "enemy" }, (impact) => {   // id "reward" from the binding (naming sugar)
  const isKill = impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0));   // BoolRef (computed)
  const bonus  = impact.target.healthAfter.le(-40).select(50, 25);
  return [ when(isKill, [
    update(progression.xp,        (xp) => xp.plus(bonus)),   // read-modify-write; slot named once
    update(progression.teamKills, (n)  => n.plus(1)),
  ])];
});
```

The three write/read verbs and their roles:
```ts
// Proposed design
update(progression.xp, (xp) => xp.plus(1));           // read-modify-write of one slot (slot named once)
set(progression.phase, 2);                            // absolute write, no implicit self-read
when(read(progression.teamKills).ge(3), [ … ]);       // read: a slot into a condition
```

`read(getGameState().player.health)` lifts an engine `ComputedRef` into the same algebra, so a policy can
read engine state, store state, and impact facts in one expression — the incoherence this spec removes. Luau is
the behavioral twin: no compiler sugar, so it names both `defineStore("progression", …)` and
`defineImpactEvent("reward", { tag = "enemy" }, …)` explicitly (single-segment id, no colon), with the
`function(cur) … end` updater callback and the `cond["and"](cond, other)` boolean spelling.
