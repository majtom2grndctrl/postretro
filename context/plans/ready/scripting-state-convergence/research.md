# Scripting State Convergence — research notes

Grounding and design derivation. Decisions live in `index.md`; this is the investigation.

## The organizing frame: "the compile-to-IR shadow of Vue"

The authoring layer is a reactive graph builder. A `defineImpactEvent((impact) => …)` body runs once at
definition time, emits serialized IR, then the VM drops — Rust evaluates that graph every tick. There is no
retained JS/Luau closure and no live value read. This is Vue's composition API with one difference: **effects
execute in Rust, not JS.** Every ergonomic divergence from Vue traces to that single fact.

| Vue | This engine | Constraint it forces |
|---|---|---|
| `ref(0)` — writable reactive source | mod store slot (`store.xp`) | — |
| `computed(() => …)` — readonly derived | engine observation (`player.health`) + author-named intermediates | body is a graph expression, not runnable JS — no native `&&`/`>`, only combinators |
| `.value` live read/write | `read(ref)` (read) / `set(ref, expr)` (write) | the value exists only later, in Rust — no live access |
| `watch(src, cb)` / `v-if` | `when(cond, effects)` | a native `if` tests a frozen node object (always truthy) → silent mis-compile |
| `Ref<T>` vs readonly `ComputedRef<T>` | `Ref<T>` (writable) vs `ComputedRef<T>` (readonly) | keyed on per-ref writable capability, not owner |

## The three dialects being converged (the incoherence)

1. **Engine state** — nouns + free helpers: `updateState(ref, v)`, `bindState(ref, opts)`, `stateEquals(ref, v)`
   (`scripting.md:135-151`). No expression vocabulary of its own.
2. **`runtime.*`** — builders yielding `RuntimeValue` (`runtime.ts`).
3. **Impact policy** — fluent `NumberRef`/`BoolRef` methods (`.gt()`, `.and()`, `.select()`, `.plus()`),
   `slot(ref).add()`, `setState()` (`data_script.ts:181-203, 251-253, 371-383`).

(2) and (3) do not interoperate: `GatedEffect.when` needs a `BoolRef` (`data_script.ts:218`), `runtime.*`
yields a `RuntimeValue` **not registered in the `numberNodes`/`boolNodes` WeakMaps** (`data_script.ts:272-285`),
and the bridging `numberRef()`/`boolRef()` adapters are **unexported** (`data_script.ts:287, 306`). So an impact
policy cannot be written in the documented engine-state dialect today.

## Pressure test — verdicts (full report: originally in scratchpad; key rows captured here)

Constraint → verdict, all grounded in shipped source:

1. **Named-intermediate signposting** — SUPPORTED as author-time sugar. `IrNode`
   (`crates/foundation/src/ir/mod.rs:111-178`) is a pure tree with no let/binding node. The fluent algebra
   lowers by embedding child nodes (`data_script.ts:287-304`); a reused `const` shares a JS object but JSON
   serialization at the FFI boundary inlines/duplicates the subtree. Signposting is free and real at authoring
   time; it is not CSE'd on the wire. Both acceptable — duplication only enlarges the tree, determinism intact.
2. **Read/write on one store-slot ref** — NEEDS-BRIDGE. A store slot is `{slot}` (`widgets.ts:46-55`) with no
   read verb; `slot(ref)` opens only onto `.add` (`data_script.ts:251-253, 371-383`). BUT the Rust substrate
   **already resolves store-slot reads by name**: `slot.add` lowers to `{op:"add", a:{op:"input", name:slot},
   b:delta}` bound via `bind_number_write` → `bind(BakedIr, EntityScope)` (`impact_policy.rs:484-494`), and
   `bind_number_write` accepts any expression, shared with `setState` (`impact_policy.rs:482, 534-544`). So
   reads are SDK-surface-only. Writes are NOT: there is no general store-slot write primitive — only additive
   `slot.add`. A general `set(ref, expr)` needs a new `slot.set` binder arm reusing `bind_number_write`.
3. **`.byPlayer` per-owner** — NEEDS-BRIDGE, large, unbuilt. Zero hits under `sdk/lib/`; `slot.add` rejects any
   target (`impact_policy.rs:497, 1517-1527`); its read leg is an open question even in the E16 draft, and the
   two E16 docs disagree whether `byPlayer` sits on the ref or the store handle. **Deferred to
   E16--per-player-currency** (index.md scope).
4. **Three-dialect type bridge** — NEEDS-BRIDGE. All three share one wire type; divergence is surface-only.
   Exporting/repackaging `numberRef`/`boolRef` (or a `read`/`fromRuntime` family) unifies them. `when`
   already accepts an author-named `BoolRef` const verbatim (`data_script.ts:218, 425`). Resolved: the read
   helper is named `read` (short form matching `set`/`update`; `*State` reserved for the UI/reaction family) —
   see `index.md` Scope.
5. **Luau twin parity** — SUPPORTED, one friction. Every fluent method exists and chains in `.luau`
   (`data_script.luau:221-241, 553-611`); `const`-naming is ordinary locals (no compiler pass). But `and`/`or`/
   `not` are Luau keywords, so `BoolRef` declares them as bracket keys `["and"]` (`.luau:237-239, 597-604`) — a
   Luau author writes `ref["and"](ref, other)`, not `ref:and(other)`. The `when`/boolean path leans on this.
6. **Readonly-engine exception** — CONFLICT with the framing (absorb, don't collapse). `ui.textEntry` is a
   **writable engine slot** (`scripting.md:118, 165`; `reactions.ts:396-428`). Writability is a per-slot catalog
   capability, not a function of ownership. Map `Ref<T> := WritableStateRef<T>`, `ComputedRef<T> :=
   ReadonlyStateRef<T>` per ref (`widgets.ts:46-55`) — an engine slot is a `Ref` when the catalog marks it
   writable. This also kills the phantom-brand silent-no-op (a `@ts-ignore` write to a readonly slot warns-and-
   no-ops at runtime instead of erroring).
7. **`when` ≠ `if`** — SUPPORTED, constraint is real. `GatedEffect.when` is a deferred `BoolRef` bound as a Bool
   IR program, errored if not Bool (`impact_policy.rs:386-395`), evaluated per fire. A native `if` decides once
   at trace time on a frozen object. `when(cond, effects)` is trivial sugar over `{ when, do }`.

**Overall: YES-WITH-NAMED-ADDITIONS. No fatal flaw, no one-way door.** SUPPORTED 3 · NEEDS-BRIDGE 3 · CONFLICT 1.

## Store-flatten mechanism (grounded)

Store-declaration drain is Rust-side: `drain_store_declarations_js` / `_lua` (`store_bridge.rs:225, 256`) read
the returned manifest as data. But `defineMod(config)` is an SDK-side identity builder
(`data_script.ts:701-705`, Luau twin `.luau:918`) that processes the manifest **before it crosses the FFI**.
That is the hook. `defineStore` returns slot refs top-level and registers `store → StoreDeclaration` in a module
identity WeakMap (mirroring `numberNodes`/`boolNodes`, `data_script.ts:272-273`); `defineMod` walks
`config.stores`, replaces each store handle with its declaration data (`{namespace, schema}` — the shape the
Rust drain already expects). Result: `store.key` (no `.state`), `stores:[store]` (no author-facing
`.declaration`), no reserved-name collision. The binding-name sugar (`crates/script-compiler`) is unaffected —
it injects the namespace argument, orthogonal to the return shape.

## File-size flags (split-first watch)

- `data_script.ts` 807, `data_script.luau` 1025 — the hub, extended by nearly every task. Cohesive vocabulary
  file. Marginally over; a split of the IR-expression-algebra section from the declaration-builder section is
  optional and would relieve the shared-file sequencing bottleneck. Not gated.
- `widgets.ts` 1013 — holds the state-ref types (`:40-55`). The rename touches only that section; extend in
  place, no split.
- `impact_policy.rs` 1691 — Task 1 adds one match arm (~10 lines) beside `slot.add`. Extend in place.
