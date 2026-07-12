# E18-B2 — Cross-Volume Simultaneity (Counter Reactions)

## Goal

Make the classic co-op separation puzzle authorable: several players each holding a switch in a separate room, all at the same instant, to progress. That is a cross-volume AND, which per-trigger activation policy (E18-B) cannot express — occupancy is per single volume. Deliver it with the smallest addition that fits the shipped machinery: two consequential slot-delta reactions that add to and subtract from a numeric counter, so N single-occupant plates raise the counter on entry and lower it on exit, and an ordinary E18-A crossing fires the host-side solve reaction when the counter reaches N. Design intent: `context/research/co-op-triggers-trap-pools.md` §4.1 (consequential dispatch), §4.3.

## Scope

### In scope

- `incrementState` / `decrementState` reactions: read a numeric slot's current value, apply a signed delta, validate-and-clamp to the slot's declared range, write it back. `incrementState` applies `+by`; `decrementState` applies `-by`; `by` is a positive magnitude, default 1; a negative `by` is rejected. Args `{slot: string, by?: number}`.
- Both are **consequential** and mirror `setState`'s two shipped execution surfaces: (a) a **system-command** path (fired from a crossing, `levelLoad`, or any named event; the read-modify-write runs on the app-side system-command drain), and (b) a **bound-in-tick trigger** path (a trigger's `on_fire`/`on_exit` executes the read-modify-write inside the sim tick against the tick-context slot table).
- SDK system-reaction body builders (`incrementState`/`decrementState`), both runtimes, plus typedefs and the drift check.
- A fixture co-op puzzle: N separated single-occupant `count`/`activation_count = 1` plates (E18-B), `fire_mode = multiple`, each `on_fire = increment`, `on_exit = decrement` a shared-global counter declared via `defineStore`; an `above` crossing at the count threshold fires the host-side `solve` reaction. Two-endpoint net-QA proving the counter replicates and the solve's mover reaches the client via replicated mover phase.

### Out of scope

- A dedicated N-of-M "gate" entity or logic-graph node. The AND is expressed as an existing `above`-threshold crossing on the counter slot; no new entity class. (Research doc §7 non-goal: general logic-gate graphs.)
- Absolute `setState` semantics (already shipped) — this spec adds delta writes only.
- A per-client duplicate of the solve reaction. The `solve` mover is consequential: it fires host-side and reaches clients through replicated mover phase. The per-client crossing channel is for presentation reactions (lights/sound); a client-visible sting is not part of this puzzle's correctness and is not scoped here.
- Non-numeric slot deltas; a runtime-authored `by` that varies per fire (bound at install/parse).

## Acceptance criteria

- [ ] `incrementState` with `by: 1` raises a numeric slot by 1; `decrementState` lowers it by 1; both validate-and-clamp to the slot's declared range (a decrement below `min` floors at `min`, not negative; an increment past `max` caps). A negative or non-finite `by` is rejected.
- [ ] `incrementState`/`decrementState` naming a non-numeric or read-only slot in a trigger binding rejects at bind time with a warning and binds nothing, matching `setState`'s bind-time validation; fired as a system reaction on such a slot, it warns and no-ops.
- [ ] Two plates whose `on_fire` increments the same counter on one tick both apply (final value rises by 2) — read-modify-write in stable `(trigger, player)` order, deterministic across two identical headless runs.
- [ ] Fixture: three separated single-occupant `fire_mode = multiple` plates, each incrementing a shared counter on entry and decrementing on exit, fire the `solve` reaction once at the instant the third plate completes the set (the rising crossing edge); releasing any plate lowers the counter and does not fire `solve`; breaking and re-completing the set fires it again. The counter stays within `[0, N]`.
- [ ] Two-endpoint test: the host's counter read-modify-write lands in-tick; a connected client's slot table converges the counter via the P3.5 apply path; the `solve` mover fires host-side and the client observes it open via replicated mover phase (not via the client's local crossing); a late-joining client observes the current counter value, not a replay of every increment.
- [ ] SDK: the typedef drift check passes with regenerated `.d.ts`/`.d.luau`; the same increment/decrement reaction authors identically from a TS mod and a Luau mod.

## Tasks

### Task 1: `incrementState` / `decrementState` — dual-path consequential slot deltas

Mirror `setState`'s two shipped surfaces end-to-end; a slot write cannot be a `ReactionPrimitiveFn` (that handler type has no slot-table access), so this follows `setState`, not `armTrigger`. **System-command path:** register `incrementState`/`decrementState` in `crates/postretro/src/scripting/systems/system_reactions.rs` beside `setState` (~line 207, registered at ~line 33) as system reactions that push a new delta command (e.g. `SystemReactionCommand::AddState { slot, by }`); handle that command in `App::dispatch_system_commands` (`main.rs`, where `SetState` is handled) by reading the current `SlotValue::Number`, adding the signed `by`, and writing through `apply_store_slot_batch` (`crates/scripting-core/src/store_bridge.rs`, which validates and clamps to `NumericRange` on write). This path serves crossing- and event-fired increments. **Bound-in-tick path:** add a `BoundTriggerCommand` delta variant mirroring `StoreSlot` (`crates/postretro/src/trigger_bindings.rs`, the `StoreSlot` variant + its `bind_store_slot` binder), but — unlike `StoreSlot`, which precomputes a validated `SlotValue` at bind — the delta variant stores `{slot, by}` and performs the read-modify-write at **execute** time (the executor holds the tick-context `Rc<RefCell<SlotTable>>`): read current `Number`, add signed `by`, write via `apply_store_slot_batch`. Bind-time validation rejects a non-numeric or read-only target with a warning (mirror `setState`'s bind path). **Classification:** add both names to **both** consequential lists — `CONSEQUENTIAL_PRIMITIVES` (`trigger_bindings.rs`, beside `setState`) and its debug mirror `is_trigger_consequential_primitive` (`crates/scripting-core/src/reaction_dispatch.rs`) — or the residual `debug_assert!` trips.

### Task 2: SDK surface + typedefs

Add `incrementState`/`decrementState` as system-reaction body builders following the `updateState` precedent — location `sdk/lib/ui/reactions.{ts,luau}` (where `setState`/`updateState` live), **not** `world.*`. They are slot-targeted system reactions returning a `PrimitiveReactionDescriptor` `{primitive, args: {slot, by}}`; they are **not** tag-targeted primitives and **not** sequence steps, so document them beside `setState`/`updateState`, not in the tag-targeted list or the `SequenceStep` union. Add the primitive names to the typedef surface the template exposes for reaction-body primitives, regenerate via `gen-script-types`, and update the committed/snapshot typedef tests. Cover TS and Luau parity with a fixture authoring the same increment/decrement reaction in both runtimes.

### Task 3: Simultaneous-switch fixture + net QA

Author the cross-room puzzle and prove the channel. `setupLevel` uses `defineStore` to declare a shared-global numeric counter slot (schema range `0..=N`, `network: "shared"`), plus reactions `increment`/`decrement` and a crossing `{ slot, above: N-0.5, max: N, fire: ["solve"] }` (the crossing divides `above` by `max`, so the normalized threshold `(N-0.5)/N` fires only at the full count — for N=3, `above: 2.5, max: 3` → 0.833, fires at 3/3 = 1.0, not at 2/3 ≈ 0.667). Map: N single-occupant `count`/`activation_count = 1`, `fire_mode = multiple` plates in separate rooms, each `on_fire = "increment"`, `on_exit = "decrement"`; `fire_mode = multiple` is required so re-entry re-increments after a decrement. `solve` is `world.query({component: "kinematic_mover", tag: "vault"}).start()`, fired host-side by the host's crossing detector and reaching clients via replicated mover phase. Headless test: drive N pawns onto the plates and assert `solve` fires once as the last plate completes the set and not on release. Two-endpoint loopback test (E18-A net-QA precedent): assert the host counter read-modify-write lands, the client converges the counter via `ClientStateApply::apply_snapshot_state` (`crates/postretro/src/netcode/state_slots.rs`), and the vault opens on the client through replicated mover phase; a late-join client observes the current counter value without replaying increments. Extend the determinism harness with a two-plate same-tick increment sequence. **Held-gate variant (document, do not require):** add a paired `below` crossing firing a `reverse` reaction to auto-close the vault when the set breaks.

## Sequencing

**Phase 1 (sequential):** Task 1 — both execution paths; Tasks 2 and 3 consume them.
**Phase 2 (concurrent):** Task 2 (SDK/typedefs) and Task 3 (fixture + net QA) — disjoint surfaces once the primitives exist.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| increment reaction | `SystemReactionCommand::AddState{slot, by}` (system path) + a `BoundTriggerCommand` delta variant (in-tick) | reaction primitive name `"incrementState"`; `args: {slot, by}` | `incrementState(slot, by?)` builder in `sdk/lib/ui/reactions.ts` | `.luau` twin | n/a (authored in a reaction) |
| decrement reaction | ditto, negated `by` | `"decrementState"`; `args: {slot, by}` | `decrementState(slot, by?)` | `.luau` twin | n/a |

`by` defaults to 1 (a positive magnitude); the slot's range/clamp is enforced on write by `apply_store_slot_batch`'s validation. The counter slot is an ordinary shared-global numeric slot declared via `defineStore` — no new wire surface. `incrementState` applies `+by`, `decrementState` applies `-by`.

## Script syntax examples

```ts
// setupLevel — three separated plates AND together via a shared counter
import { world, defineStore, defineReaction,
         incrementState, decrementState } from "postretro";

// shared-global counter, replicated to every client (exact defineStore shape
// per the shipped store-declaration surface)
defineStore("puzzle", { switchesHeld: { type: "number", range: [0, 3],
                                         network: "shared", default: 0 } });

export function setupLevel(ctx) {
  return {
    reactions: [
      // authored on every plate brush: on_fire = "increment", on_exit = "decrement"
      // plates are activation_policy = count, activation_count = 1, fire_mode = multiple
      defineReaction("increment", () => [incrementState("puzzle.switchesHeld")]),
      defineReaction("decrement", () => [decrementState("puzzle.switchesHeld")]),
      defineReaction("solve", () => [...world.query({ component: "kinematic_mover",
                                                      tag: "vault" }).start()]),
    ],
    crossings: [
      // above 2.5 of max 3 (normalized 0.833) fires only when all three are held
      { slot: "puzzle.switchesHeld", above: 2.5, max: 3, fire: ["solve"] },
    ],
  };
}
```

## Open questions

None blocking.

- The counter is host-authoritative and replicates as state; clients converge the value. `solve` is consequential (a mover command) and fires host-side, reaching clients through replicated mover phase — not through each client's local crossing.
- Same-tick increments from multiple plates apply sequentially in E18-B's stable `(trigger, player)` order, so the result is deterministic. Determinism rests on stable execution order, not on delta commutativity (clamped deltas are not commutative at saturation).
- Delta writes are count-sensitive, so E18-A's exactly-once in-tick guarantee and E18-B's paired enter/exit gating (each increment paired to exactly one decrement) are load-bearing here in a way they are not for idempotent setters — a doubled or dropped edge desynchronizes the counter.
