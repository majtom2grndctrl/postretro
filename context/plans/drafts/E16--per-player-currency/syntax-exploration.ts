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
// Variant C — current direction
// ===========================================================================
//
// Three moves, each independently justified above:
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
//   (b) Store slot handles carry read AND write on one object, keeping the
//       `slot: string` field so `runtime.read(ref)` (runtime.ts:54-56) and
//       `stateSlot(ref)` (reactions.ts:57-62) keep working unchanged:
//
//         export interface NumberSlotRef extends NumberRef {
//           readonly slot: string;
//           set(value: NumberValue): Effect;
//           add(delta: NumberValue): Effect;   // sugar, kept for legibility
//         }
//
//       Extending NumberRef is what buys the dry read: the handle IS the
//       readable expression, so `.gt()`, `.plus()`, `.select()` compose off it
//       directly. No `slot()` wrapper, no separate read door.
//
//   (c) Owner scoping is a method on the STORE handle taking the owner token,
//       not a property path off the owner. `impact.source.entityStore.
//       progression` is not constructible: IMPACT_SOURCE is frozen at SDK load
//       (fact 4) and `progression` is declared later by author code, so the
//       singleton cannot carry a property named after it. Inverting preserves
//       the reading order almost exactly and is buildable:
//
//         progression.byPlayer(impact.source).xp

// --- Declarations (proposed shapes, for the examples below) ----------------

const progression = defineStore("player", {
  xp: { type: "number", default: 0, perOwner: true, network: "ownerPrivate" },
  teamKills: { type: "number", default: 0 },
});

// --- Read and write, owner-scoped -----------------------------------------

const reward = defineImpactEvent("dev:reward", { tag: "enemy" }, (impact) => {
  const attackerProgression = progression.byPlayer(impact.source);

  const targetIsKilled = impact.target.healthBefore.gt(0).and(impact.target.healthAfter.le(0));
  const base = impact.target.healthAfter.le(-40).select(50, 25);

  // READ: `attackerProgression.xp` is itself a NumberRef — arithmetic and
  // comparison compose straight off it, no wrapper.
  const bonus = attackerProgression.xp.gt(100).select(base.times(2), base);

  return [
    {
      when: targetIsKilled,
      do: [
        // WRITE, expression parameter — the general form. Arbitrary IR in,
        // including a read of the same slot.
        attackerProgression.xp.set(attackerProgression.xp.plus(bonus).clamp(0, 9999)),

        // The common case stays short. Identical lowering to the above minus
        // the clamp, per fact 1.
        progression.teamKills.add(1),
      ],
    },
  ];
});

// Note what (b) bought: `progression.teamKills` and `attackerProgression.xp`
// are the same interface. Global and owner-scoped writes are one code path,
// differing only in whether `.byPlayer()` was applied. This is the
// shared-write problem Variant B could not resolve.

// --- What `.byPlayer()` returns -----------------------------------------
//
//   interface StoreDefinition<S> {
//     byPlayer(owner: SourceHandle): OwnedSlots<S>;   // owner-addressed view
//     // ...plus the bare per-slot NumberSlotRefs already exposed today
//   }
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
// - Keep `.add()` alongside `.set()`? It is pure sugar (fact 1) but it is the
//   overwhelmingly common case and reads better than `x.set(x.plus(n))`. Cost
//   is two verbs where one would do.
//
// - Does `slot(ref)` (:369) survive? If NumberSlotRef carries `.set`/`.add`
//   directly, the wrapper has no remaining job. Removing it is a breaking SDK
//   change to a shipped export — probably out of scope for this spec, so it
//   likely stays as a deprecated alias.
//
// - Does `NumberSlotRef extends NumberRef` create a read where the substrate
//   has none? Global store slots resolve through StoreScope, so a bare read
//   is fine. Owner-scoped reads are exactly what the spec's per-owner input
//   work exists to provide — this syntax assumes that work lands, and should
//   be checked against the spec's own AC list before folding in.
//
// - `WritableStateRef<T>` is generic over number|boolean|string, but `.plus`,
//   `.clamp` etc. only make sense for numbers. Needs either a conditional type
//   or a separate BoolSlotRef/StringSlotRef with the appropriate verbs.
