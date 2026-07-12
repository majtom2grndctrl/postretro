# E18-B2 — Cross-Volume Simultaneity (Counter Reactions)

## Goal

Make the classic co-op separation puzzle authorable: several players each holding a switch in a separate room, all at the same instant, to progress. That is a cross-volume AND, which per-trigger activation policy (E18-B) cannot express — occupancy is per single volume. Deliver it with the smallest possible addition: two consequential reaction primitives that add to and subtract from a numeric slot, so N single-occupant plates increment a shared counter on entry and decrement on exit, and an ordinary E18-A crossing fires the solve reaction when the counter reaches N. Design intent: `context/research/co-op-triggers-trap-pools.md` §4.1 (consequential dispatch), §4.3.

## Scope

### In scope

- `incrementState` / `decrementState` reaction primitives: read a numeric slot, add a signed delta (`by`, default 1), clamp to the slot's declared range, write through the one validated slot path. Args `{slot: string, by?: number}`.
- Classification as **consequential** (writes replicated slot state), so a trigger's `on_fire`/`on_exit` binds them at install and executes them in-tick, exactly like the shipped `setState` — reusing E18-A's bind-at-install dispatch and the persistent-state co-op channel.
- SDK builders (`incrementState`/`decrementState`) and typedefs, both runtimes.
- A fixture co-op puzzle: N separated single-occupant `count`/`activation_count = 1` plates (E18-B) each `on_fire = increment`, `on_exit = decrement` a `sharedGlobal` counter slot; a crossing at the count threshold fires the solve reaction. Two-endpoint net-QA proving the counter replicates and each client's crossing fires locally.

### Out of scope

- A dedicated N-of-M "gate" entity or logic-graph node. The AND is expressed as an existing `above`-threshold crossing on the counter slot; no new entity class. (Research doc §7 non-goal: general logic-gate graphs.)
- Absolute `setState` semantics (already shipped) — this spec adds delta writes only.
- Reliable transient co-op stings — the counter and its solve reaction ride the shipped persistent-state channel; transient delivery stays host-local per E18-A.
- Non-numeric slot deltas.

## Acceptance criteria

- [ ] `incrementState` with `by: 1` on a numeric slot raises its value by 1; `decrementState` lowers it; both clamp to the slot's declared range (a decrement below `min` floors at `min`, not negative; an increment past `max` caps).
- [ ] `incrementState` on a non-numeric or read-only slot fails validation at bind time with a warning and binds nothing, matching `setState`'s bind-time validation.
- [ ] Two plates whose `on_fire` increments the same counter on one tick both apply (final value rises by 2) — sequential read-modify-write, order stable, deterministic across two identical headless runs.
- [ ] Fixture: three separated single-occupant plates, each incrementing a `sharedGlobal` counter on entry and decrementing on exit, fire the solve reaction only while all three are held simultaneously; releasing any plate drops the counter below the threshold and the crossing does not fire; re-holding all three fires it again.
- [ ] Two-endpoint test: the host's counter writes land in-tick; a connected client's slot table converges via the P3.5 apply path and the client's crossing fires the client-local solve presentation once per threshold crossing; a late-joining client observes the current counter state, not a replay of every increment.

## Tasks

### Task 1: `incrementState` / `decrementState` primitives + consequential binding

Add the two primitives following the `setState` precedent end-to-end. Register them in the trigger/slot reaction module beside the E18-A slot handling, as `ReactionPrimitiveFn`s that: resolve the `slot` arg, read the current `SlotValue::Number` (missing/non-number ⇒ validation error), compute `clamp(current + by, range)`, and write through `apply_store_slot_batch` (`crates/scripting-core/src/store_bridge.rs`) — the same validated path scripts, netcode, and E18-A bound writes use. Add both names to the E18-A consequential classification const table (the binder that today lists `setState` as consequential) and give the bound form a `BoundTriggerCommand` slot-delta variant (slot name + validated delta + resolved range), so a trigger's `on_fire = increment`/`on_exit = decrement` executes in-tick against the slot table with no VM in the sim seam. Read-only and non-numeric slots reject at bind with a warning, mirroring `setState`. Wire the registrar into the live reaction registry at the same startup site E18-A registers `armTrigger`/`disarmTrigger`.

### Task 2: SDK surface + typedefs

Add `incrementState`/`decrementState` to the reaction step vocabulary following the `setState`/`updateState` SDK precedent: TS and Luau builders producing `{primitive: "incrementState" | "decrementState", args: {slot, by}}`, delegated from `sdk/lib/world.{ts,luau}`; add the literal step types to the typedef template beside the existing state-write step types and extend the tag-targeted primitive doc list; regenerate via `gen-script-types` and update the committed/snapshot typedef tests. Cover both runtimes with a fixture authoring the same increment/decrement reaction in TS and Luau.

### Task 3: Simultaneous-switch fixture + net QA

Author the cross-room puzzle fixture and prove the channel. `setupLevel` declares a `sharedGlobal` numeric counter slot (range `0..=N`), reactions `increment`/`decrement`, and a crossing `{ slot: counter, condition: above N-of-max fraction, fire: [solve] }`. Map: N single-occupant `count`/`activation_count = 1` plates in separate rooms, each `on_fire = "increment"`, `on_exit = "decrement"`. Headless test: drive N pawns onto the plates and assert the solve reaction fires only when the last plate is held and un-fires when any releases. Two-endpoint loopback test (E18-A net-QA precedent): assert host counter writes land in-tick, the client converges the counter via `ClientStateApply::apply_snapshot_state`, the client crossing fires the local solve presentation once, and a late-join client observes current counter state without replaying increments. Extend the determinism harness with a two-plate same-tick increment sequence.

## Sequencing

**Phase 1 (sequential):** Task 1 — primitives + binding; Tasks 2 and 3 consume them.
**Phase 2 (concurrent):** Task 2 (SDK/typedefs) and Task 3 (fixture + net QA) — disjoint surfaces once the primitives exist.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| increment primitive | handler in the slot reaction module | reaction step `"incrementState"` | `"incrementState"` builder + step type | `"incrementState"` | n/a (authored in `setupLevel`) |
| decrement primitive | ditto | `"decrementState"` | `"decrementState"` builder + step type | `"decrementState"` | n/a |

`by` defaults to 1; the delta and the slot's range are resolved and validated at bind time, not per tick. The counter slot is an ordinary `sharedGlobal` numeric slot — no new wire surface.

## Script syntax examples

```ts
// setupLevel — three separated plates AND together via a shared counter
import { world, defineReaction, incrementState, decrementState } from "postretro";

export function setupLevel(ctx) {
  return {
    state: [{ slot: "puzzle.switchesHeld", type: "number", range: [0, 3],
              network: "shared", default: 0 }],
    reactions: [
      // authored on every plate brush: on_fire = "increment", on_exit = "decrement"
      defineReaction("increment", () => [incrementState("puzzle.switchesHeld")]),
      defineReaction("decrement", () => [decrementState("puzzle.switchesHeld")]),
      defineReaction("solve", () => [...world.query({ component: "kinematic_mover",
                                                      tag: "vault" }).start()]),
    ],
    crossings: [
      // above 2.5 of max 3 ⇒ fires only when all three are held at once
      { slot: "puzzle.switchesHeld", condition: { above: 2.5 }, max: 3,
        fire: ["solve"] },
    ],
  };
}
```

## Open questions

None blocking.

- The counter is host-authoritative and replicates as state, so clients converge the value and their crossings fire locally — the shipped E18-A persistent-state discipline (idempotent setters, state-not-pulses) holds because a clamped counter is a value, not an edge.
- Same-tick increments from multiple plates apply sequentially in `(trigger, player)` order (E18-B's stable stream), so the final value is order-independent for commutative deltas and deterministic regardless.
