// E16--per-player-currency — syntax exploration scratchpad
// NOT part of the spec yet. Iterating here before folding anything into
// context/plans/drafts/E16--per-player-currency/index.md.
//
// Current direction: Variant C (bottom). `impact.source` becomes a plain
// branded frozen value carrying its own token; store slot handles gain
// read+write on one object; `addSlot` is dropped in favor of an
// expression-taking update method.

// ===========================================================================
// Verified facts (line numbers are checkable; everything else here is a
// proposal, not shipped)
// ===========================================================================
//
// 1. `slot.add` is ALREADY a read-modify-write set. impact_policy.rs:457-467:
//
//      "slot.add" if target.is_none() => {
//          let value = json!({ "op": "add",
//                              "a": { "op": "input", "name": slot },
//                              "b": delta });
//          bind_number_write(slot.to_string(), &value, scope).map(BoundEffect::Write)
//      }
//
//    `bind_number_write` is the same function `setState` binds through
//    (impact_policy.rs:455). There is NO additive primitive at the substrate —
//    `add(delta)` is SDK sugar over `set(read(slot) + delta)`. So exposing a
//    `.set(expr)` verb widens no engine surface; it removes a sugar layer.
//    Corollary: `.set()` carries no ordering hazard `.add()` didn't already
//    have, since both lower to the same snapshot-read-then-write.
//
// 2. Expression-in-parameter is already the house style for updates:
//      setHealth(value: NumberValue, opts?)          data_script.ts:228, 344-348
//      setState(name: string, value: NumberValue)    data_script.ts:230, 352-354
//    Both lower via `numberNode(value)`. `slot(ref).add(delta)` (:249-251, :369)
//    is the outlier — the only update verb fixed to one operation.
//
// 3. Read/write are already symmetric for per-entity state and asymmetric for
//    store slots:
//      impact.target.state(name) -> NumberRef   /  .setState(name, v) -> Effect
//      progression.xp            -> {slot:string} (NO read capability at all)
//    `WritableStateRef<number>` is literally `{ slot: string }` plus brands
//    (widgets.ts:47-55; built at data_script.ts:774). It cannot be read from
//    or written to directly — `slot(ref)` (:369) is the only door, and it
//    opens only onto `.add`.
//
// 4. `IMPACT_SOURCE` is a module-private frozen singleton built at SDK load
//    (data_script.ts:357-360), holding only `grantHealth`/`grantAmmo` closures
//    over the wire literal "@impact.source". It exposes no token field.
//    `SourceHandle` (:233-247) is `{ [sourceBrand]: true }` + those two methods.
//
// 5. `ActivatorsTarget` (:18-22) is the reaction-side precedent for a
//    data-free branded handle: checked by identity against a private singleton
//    (`target === ACTIVATORS_TARGET`, :509/521/534), with the wire string
//    baked into the free function rather than read off the handle. Free
//    functions taking a handle (grantHealth(target, amount), :514-523) are
//    already the reaction-side shape.
//
// 6. `id`/namespace strings stay required. The setup VM drops after execution;
//    only serialized data survives. `.override()` (:397-408) mints a second
//    ImpactEvent reusing the same `id` string, which a JS binding cannot do.
//
// 7. defineStore returns `{ declaration, state }` (data_script.ts:776-779).
//    Slots are at `progression.state.xp`. Only `declaration` reaches the engine,
//    and only when returned from `ModManifest.stores` (:758).
//
// 8. Cross-file store access already works — NO new sugar needed.
//    scripting.md:253: "`scripts-build` bundles the entry file with its
//    relative imports, strips TypeScript-only syntax, and removes
//    bare-specifier imports. Engine APIs and SDK library symbols arrive as
//    QuickJS globals, not module imports."
//
//    So this is legal today:
//
//      // stores.ts
//      export const progression = defineStore("player", { ... });
//
//      // weapons.ts
//      import { progression } from "./stores";
//
//    Bare specifiers (`from "postretro"`) are stripped because those symbols
//    are already globals — the prelude rewrites named exports to
//    `globalThis.<name>` (scripting.md:267). Two hazards: scripts-build "does
//    not type-check" (:259), and `const enum` across file boundaries yields
//    `undefined` silently (:271).
//
// 9. State access has a WRITTEN house rule, and it forbids verbs on refs.
//    scripting.md:141: "There is no `.get()`, `.set()` ... Nouns select state.
//    Helpers describe how a reference is used" — helpers enumerated at :143-145
//    (bindState, stateEquals, updateState), restated at :161. Property access
//    "never reads current engine state" (:132).

// ===========================================================================
// Superseded (kept only so the reasoning isn't re-derived)
// ===========================================================================
//
// Variant A — `progression.for(impact.source)` returning an owner-scoped store
//   instance, Pinia's useStore() flavor. Superseded by C, which keeps the same
//   scoping move but fixes the read/write asymmetry at the slot level rather
//   than wrapping the whole store.
//
// Variant B — SourceHandle as a token-carrying value consumed by free
//   functions `grantHealth(source, amount)` / `addSlot(source, ref, delta)`.
//   The token-carrying half survives into C. The free-function half is dropped:
//   it left shared (non-owner) writes with no handle to pass, forcing either
//   two `addSlot` signatures or a split between owned and global write paths.
//   C removes that problem by putting the verb on the slot handle, where the
//   owner is an addressing detail rather than a required argument.

// ===========================================================================
// Variant C — WITHDRAWN in part. Its (b) violates a written rule.
// ===========================================================================
//
// scripting.md:141 states the house rule for state access outright:
//
//   "There is no `.get()`, `.set()`, `gameState` global, `playerState` global,
//    `gameState.query()`, or `postretro/game-state` module. Nouns select
//    state. Helpers describe how a reference is used"
//
// with the sanctioned helpers enumerated at :143-145 — `bindState(ref, opts)`,
// `stateEquals(ref, value)`, `updateState(ref, value)` — and reinforced at
// :161 ("do not call `.get()` on state refs"). scripting.md:132 adds that
// property access "never reads current engine state."
//
// C(b) proposed `NumberSlotRef extends NumberRef` with `.set()` / `.add()`
// methods on the ref. That is the forbidden shape. Variant B's free functions
// were the orthodox one; C traded that away to solve the shared-write problem,
// which does not justify breaking a documented rule.
//
// C(a) and C(c) survive:
//
//   (a) `impact.source` becomes a plain branded frozen value carrying its own
//       token. This makes "the handle IS the ID" hold on the owner side the
//       way it already holds on the slot side, AND makes the token
//       load-bearing — today nothing reads it, so it is decorative.
//
//         export type SourceHandle = Readonly<{
//           readonly token: "@impact.source";
//           readonly [sourceBrand]: true;
//         }>;
//
//   (c) Owner scoping is a method on the STORE handle taking the owner token,
//       not a property path off the owner. `impact.source.entityStore.
//       progression` is not constructible: IMPACT_SOURCE is frozen at SDK load
//       (fact 4) and `progression` is declared later by author code, so the
//       singleton cannot carry a property named after it. Inverting preserves
//       the reading order almost exactly and is buildable. Critically, this is
//       SELECTION, which :141 expressly permits ("Nouns select state") — it
//       returns another noun, it does not act.
//
//         progression.state.xp   /   progression.byPlayer(impact.source).xp

// ===========================================================================
// The dialect problem (larger than this spec — do not solve it here)
// ===========================================================================
//
// Three non-converging dialects exist for building state expressions:
//
//   1. Engine state — nouns plus free helpers: `updateState(ref, v)`,
//      `bindState(ref, opts)`, `stateEquals(ref, v)`.   scripting.md:132-146
//   2. runtime.*    — nested builders: `runtime.add(a, b)`.  runtime.ts:48-109
//   3. Impact policy — fluent methods: `.plus()`, `.setState()`,
//      `slot(ref).add()`.        data_script.ts:179-201, :221-251
//
// (2) and (3) do not interoperate: `GatedEffect.when` requires a BoolRef
// (data_script.ts:216), `runtime.*` yields RuntimeValue, and the bridging
// `numberRef()` (data_script.ts:285) is not exported. So an impact policy
// CANNOT be written in the documented dialect (1) today.
//
// This is the incoherence, and converging it is its own spec. E16's job is to
// avoid adding a FOURTH dialect while it lands per-owner access.

// ===========================================================================
// Variant D — current direction. Minimal, adds no new dialect.
// ===========================================================================
//
//   - Keep C(a): SourceHandle carries its own token.
//   - Keep C(c): `byPlayer(owner)` SELECTS, returning a ref — a `{slot, owner}`
//     noun. No verbs added to refs, so :141 holds.
//   - Writes keep using the shipped door, `slot(ref).add(delta)`
//     (data_script.ts:249-251, :369), widened to accept an owner-addressed ref.
//     Reads keep using whatever the policy dialect already provides.
//
// Cost: the read/write symmetry C(b) bought is given up, and `slot(ref)`
// survives rather than being retired. Benefit: nothing new to unify when the
// dialect convergence spec lands.

// --- Declarations ----------------------------------------------------------
// Note the `.state` hop: defineStore returns `{ declaration, state }`
// (data_script.ts:776-779), so slots live at `progression.state.xp`, NOT
// `progression.xp`. Earlier drafts of this file had that wrong throughout.
//
// Cross-file: this can live in its own `stores.ts` and be imported. See the
// import note below.

const progression = defineStore("player", {
  xp: { type: "number", default: 0, perOwner: true, network: "ownerPrivate" },
  teamKills: { type: "number", default: 0 },
});

// --- Read and write, owner-scoped (Variant D) ------------------------------

const reward = defineImpactEvent("dev:reward", { tag: "enemy" }, (impact) => {
  // SELECT: returns a ref — a noun — per scripting.md:141. No verbs on it.
  const attackerXp = progression.byPlayer(impact.source).xp;

  const targetIsKilled = impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0));
  const base = impact.target.healthAfter.le(-40).select(50, 25);
  const bonus = base; // owner-addressed READ deferred — see open questions

  return [
    {
      when: targetIsKilled,
      do: [
        // WRITE through the shipped door, widened to accept an owner-addressed
        // ref. Adds no dialect: `slot(ref).add()` already exists
        // (data_script.ts:249-251, :369) and already lowers to a
        // read-modify-write set (fact 1).
        slot(attackerXp).add(bonus),

        // Global slot, same door, no owner selection applied.
        slot(progression.state.teamKills).add(1),
      ],
    },
  ];
});

// --- What `.byPlayer()` returns --------------------------------------------
//
//   interface StoreDefinition<S> {
//     readonly declaration: ...;
//     readonly state: { readonly [K in keyof S]: StateValueForSlot<S[K]> };
//     byPlayer(owner: SourceHandle): { readonly [K in keyof S]: ... };
//   }
//
// The returned per-slot values are refs of the SAME kind `state` already
// yields — `{ slot: string }` plus an owner token — so every existing consumer
// that reads `.slot` (runtime.read, runtime.ts:54-56; stateSlot,
// reactions.ts:57-62) keeps working, and nothing gains a verb.
//
// It reads `owner.token` and stamps it into each returned slot handle's
// lowered IR — the owner is an addressing detail carried in the wire data,
// not a separate argument threaded through every call site. Whether the
// lowered form is an owner field on the input/write node or an encoded slot
// string is an engine-side question the spec already takes a position on;
// this file does not re-decide it.

// --- How a store associates with an entity ---------------------------------
//
// Script never makes the association. The author holds no link and attaches
// none; they write a name for a resolution path the host walks at apply time:
//
//   author names slot + owner token
//     -> lowered IR carries "player.xp" + "@impact.source"
//     -> token resolves to EntityId
//     -> EntityId -> Seat        (registry mirror, per the spec's B1 work)
//     -> (slot, Seat) -> record  (index.md:11 keys by Seat; :64 host-only)
//
// This is the constraint any name here has to respect, and the reason `get`
// misleads: it suggests the author ends up holding the record. Nothing in the
// authoring surface ever does.

// --- Reaction side, unchanged ---------------------------------------------
// Reactions have no `impact.source`. They stay flat top-level calls addressed
// by activators/tag, using the shipped free-function shape (fact 5).

onTriggerEvent({ tag: "objective" }, "enter", [
  defineReaction((on: TriggerEventParams) => grantHealth(on.activators, 25)),
]);

// ===========================================================================
// Open questions
// ===========================================================================
//
// - Name for the scoping method. Settled on `.byPlayer(owner)`. The reasoning,
//   since it is not obvious and should not be re-derived:
//
//     * It is not a read verb. There is no reading in this SDK at all. Every
//       "read" constructs a named input leaf and stops — `healthBefore` is
//       `{op:"input", name:"@impact.healthBefore"}` (data_script.ts:335),
//       `state(name)` is `{op:"input", name:"@state.<name>"}` (:349-351), and
//       even `runtime.read` only names an input (runtime.ts:54-56). Rust
//       resolves the name per-tick via BindingScope::resolve_input. So the
//       method addresses a record; it never obtains one.
//
//     * Rejected `getByPlayer` on that basis: `get` implies the author ends up
//       holding the thing, which is precisely what does not happen. House
//       prefixes are `define*` / `on*`, and the shipped read accessor carries
//       no verb at all — `state(name)` / `setState(name, v)`, noun as accessor.
//
//     * `by<Key>` is the vocabulary engineers already share for "keyed by
//       this": groupBy, keyBy, findBy, sortBy, LINQ GroupBy, SQL GROUP BY.
//       Per-owner storage is keyed exactly that way, `(slot, Seat)`
//       (index.md:11). It names the key — the complaint against `.of()` — and
//       implies no fetch.
//
//     * "player" is correct, not aspirational. index.md:31 scopes owners to
//       players outright: "Seats belong to players. An enemy's own counter
//       stays per-entity state."
//
//   An earlier draft of this file objected that `byPlayer(aTurret)` would
//   type-check and silently no-op. That objection was wrong: index.md:70 and
//   AC index.md:85 already have a seatless recipient write nothing, WARN, and
//   leave sibling effects applying — explicitly distinguished from the shipped
//   silent skip for an absent source. The mis-scoping case is already loud.
//
// - THE OPEN ONE: how does a policy READ an owner-addressed slot? Variant D
//   gives up C(b)'s answer (ref extends NumberRef) because :141 forbids it, and
//   leaves nothing in its place — the example above sidesteps the read. The
//   spec requires owner-addressed reads (index.md:22, "a policy reads a
//   per-owner slot against an explicit owner token"), so this cannot stay open.
//
//   The obstacle is the dialect split, not the per-owner work. Impact policies
//   need NumberRef/BoolRef (GatedEffect.when, data_script.ts:216). The
//   documented dialect (1) has no expression vocabulary of its own, and dialect
//   (2)'s output does not convert (numberRef not exported, :285). Candidates:
//
//     * Export `numberRef()` so runtime.* output bridges into policy
//       expressions. Smallest change; makes dialect (1)+(2) usable in policies;
//       arguably belongs to the convergence spec, not E16.
//     * A read helper in the :143-145 family, e.g. `readState(ref)` returning a
//       NumberRef. Fits "helpers describe how a reference is used" exactly, and
//       is the same move `bindState`/`stateEquals` already make.
//     * Let `slot(ref)` yield reads as well as writes, keeping one door.
//
//   Recommend the second: it adds a helper to a documented family rather than
//   a verb to a ref, and it works for global and owner-addressed refs alike.
//
// - Does `slot(ref)` (:369) survive? Under Variant D, yes — it stays the write
//   door and needs only to accept an owner-addressed ref. Nothing is retired,
//   nothing is deprecated.
//
// - Is per-slot type-narrowing needed? `WritableStateRef<T>` is generic over
//   number|boolean|string. Variant D adds no arithmetic to refs, so this
//   pressure mostly disappears — but whatever read helper lands must still
//   reject non-number slots at the type level.
//
// - Should the dialect convergence be its own spec? Almost certainly yes, and
//   E16 should avoid prejudging it. Worth naming explicitly in E16's
//   out-of-scope list so the omission reads as deliberate.
