# E18-B2 — Cross-Volume Simultaneity (Co-op Puzzle)

## Goal

Make the classic co-op separation puzzle authorable and prove it end to end: several players each holding a switch in a separate room, all at the same instant, to progress. That is a cross-volume AND, which per-trigger activation policy (E18-B) cannot express — occupancy is per single volume. This spec composes shipped and adjacent pieces into the pattern and a playable fixture; it introduces no new primitive. Design intent: `context/research/co-op-triggers-trap-pools.md` §4.3.

**Depends on:** `E18--state-delta-reactions` (the `incrementState`/`decrementState` reactions) and E18-B (the `count` activation policy). Both must land first.

## Scope

### In scope

- The authoring pattern: N separated single-occupant plates each raise a shared counter on entry and lower it on exit; an ordinary E18-A crossing on the counter fires the host-side solve reaction when it reaches N. Documented as a reusable co-op pattern.
- A fixture level + `setupLevel` script realizing it: a `defineStore` shared-global counter, `increment`/`decrement` reactions, N `count`/`activation_count = 1`, `fire_mode = multiple` plates, and an `above` crossing firing `solve`.
- Headless test proving the AND (fires only at the full set), and two-endpoint net-QA proving the counter replicates and the solve's mover reaches the client via replicated mover phase.

### Out of scope

- The `incrementState`/`decrementState` reactions themselves — `E18--state-delta-reactions` owns them.
- The `count` activation policy — E18-B owns it.
- A dedicated N-of-M gate entity or logic-graph node. The AND is an existing `above`-threshold crossing on the counter; no new entity class.
- A per-client duplicate of the solve reaction: the solve mover is consequential, fires host-side, and reaches clients through replicated mover phase; the per-client crossing channel is for presentation reactions.

## Acceptance criteria

- [ ] Fixture: three separated single-occupant `fire_mode = multiple` plates, each incrementing a shared counter on entry and decrementing on exit, fire the `solve` reaction once at the instant the third plate completes the set (the rising crossing edge); releasing any plate lowers the counter and does not fire `solve`; breaking and re-completing the set fires it again. The counter stays within `[0, N]`.
- [ ] The crossing fires only at the full count: with `above: N-0.5, max: N`, the normalized threshold `(N-0.5)/N` fires at `N/N = 1.0` and not at `(N-1)/N` (for N=3: `above: 2.5, max: 3` → 0.833; fires at 3, not 2).
- [ ] Two-endpoint test: the host's counter writes land in-tick; a connected client's slot table converges the counter via the P3.5 apply path; the `solve` mover fires host-side and the client observes the vault open via replicated mover phase. The client's own crossing also fires `solve` locally, but that fire is inert — clients reconcile replicated mover phase and never author movers — so the vault opens once, via replication. A late-joining client observes the current counter value (and re-fires its local `solve` harmlessly) without replaying every increment.
- [ ] `fire_mode = multiple` on the plates is required and asserted: with `once` plates, a released-and-re-entered plate does not re-increment and the puzzle cannot re-arm.

## Tasks

### Task 1: Fixture, authoring pattern, and QA

Author the cross-room puzzle and prove the channel; no engine code changes (all primitives come from `E18--state-delta-reactions` and E18-B). `setupLevel` uses `defineStore` to declare a shared-global numeric counter slot (schema range `0..=N`, `network: "shared"`), plus reactions `increment` (`[incrementState("puzzle.switchesHeld")]`), `decrement` (`[decrementState(...)]`), and `solve` (`world.query({component: "kinematic_mover", tag: "vault"}).start()`), and a crossing `{ slot, above: N-0.5, max: N, fire: ["solve"] }`. Map: N single-occupant `activation_policy = count`, `activation_count = 1`, `fire_mode = multiple` plates in separate rooms, each `on_fire = "increment"`, `on_exit = "decrement"`; `fire_mode = multiple` is required so re-entry re-increments after a decrement. `solve` fires host-side via the host's crossing detector and reaches clients through replicated mover phase. Headless test: drive N pawns onto the plates and assert `solve` fires once as the last plate completes and not on release, and that the counter clamps to `[0, N]`. Two-endpoint loopback test (E18-A net-QA precedent): assert the host counter writes land, the client converges the counter via `ClientStateApply::apply_snapshot_state` (`crates/postretro/src/netcode/state_slots.rs`), and the vault opens on the client through replicated mover phase; confirm the client's local `solve` fire is inert; a late-join client observes the current counter value without replaying increments. Document the pattern (plate KVPs + counter + crossing) as a reusable co-op recipe. **Held-gate variant (document, do not require):** add a paired `below` crossing firing a `reverse` reaction to auto-close the vault when the set breaks.

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
      // authored on every plate brush: on_fire = "increment", on_exit = "decrement";
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

- `solve` is consequential (a mover command) and fires host-side, reaching clients through replicated mover phase — not through each client's local crossing. Each client's crossing does fire `solve` locally, but a client-side mover-start is inert because clients reconcile replicated mover phase and never author movers.
- The puzzle avoids counter saturation (each plate contributes exactly ±1, range `[0, N]`), so clamp order never matters; determinism rests on E18-B's stable `(trigger, player)` execution order.
