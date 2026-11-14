# E18--coop-activation-policy

Brief · compact · Epic 18 · reads: `context/lib/scripting.md` §5, §12 · `context/lib/networking.md` §Admission and content parity, §What gates, and what replicates instead · `context/lib/entity_model.md` §7 · `plans/done/E18--player-life-states/` · `plans/done/E18--trigger-event-params/` · `plans/done/E18--dispatch-scope-params/` · read at 6c862fd

## Problem
Owner-raised, reopened. Co-op puzzles need "both plates held at once" and "every player is in the room", and scripts cannot read either: the trigger stage tracks effective occupants per volume, but exposes them only as the per-fire `on.occupancy` count. The 2026-07-14 plan for this was moved to `done/` without code, so `scripting.md` §12, the roadmap and downstream plans (`E18--dispatch-scope-params`' King-of-the-Hill fixture, E25's wait-for-everyone seam) assume refs that do not exist. When done, a script reads per-tag occupancy as two readonly numbers, `occupiedCount(tag)` and `occupants(tag)`, on host and clients, and builds count-threshold and all-players puzzles from the existing crossing watcher and accumulators, with no brush key.

## Decisions
- **Two per-tag refs.** `occupiedCount(tag)`: volumes carrying the tag with at least one effective occupant. `occupants(tag)`: distinct effective players inside any volume carrying the tag, so overlapping volumes never count a player twice. Both are readonly Numbers.
- **Who counts is the trigger stage's effective set** (`plans/done/E18--player-life-states/`): a declared pawn by its state's `occupancy` capability, an undeclared one unless its health is depleted. Membership ignores the volume's armed and latched state, as the occupant map does. No second alive rule.
- **Volumes are trigger volumes**, `switch` included, since a switch is a trigger volume at the component layer. E25's `level_seam` joins through its own inside test (`drafts/E25--landmark-level-change/`).
- **Engine-owned, level-scoped slots.** Each level install declares `occupancy.<tag>.occupiedCount` and `occupancy.<tag>.occupants` for every tag on an occupancy volume, before reactions, crossings and accumulators rebuild; unload removes them. Engine-written, script-readonly: a script write warns and changes nothing.
- **`occupancy` is a reserved engine namespace.** A mod store declaring it, or a namespace overlapping it, is rejected at load whether or not a level is loaded, as `@` is (`scripting.md` §5).
- **A tag that cannot form a slot name** (one containing `.` or `:`) warns once at install and gets no slots. Its volumes still fire events.
- **Written once per authoritative host tick**, after the trigger stage settles and before accumulators and crossing detection, so a set that completes on a tick crosses on that tick (`plans/done/E18--dispatch-scope-params/` ordering). Host and single player compute; clients read the replicated value.
- **Replicated `SharedGlobal`**, through the existing slot replication, late-join baseline included. A crossing reading only these slots is host-mirrored (`scripting.md` §12). No new wire section.
- **Trigger tags join the level content digest** (epoch bump). The slot schema derives from the level's tags on each side; a pair whose tags differ is refused at admission instead of dropping every replicated slot at the schema fingerprint check.
- **Unknown tag degrades.** A ref to a tag with no slot in the current level reads an undeclared slot: a crossing or accumulator over it warns and skips, as for any undeclared slot. No load failure.
- **First consumers ship here:** the two-button co-op puzzle (`content/dev/scripts/coop-two-button-puzzles.ts`, today a design spike on a store workaround) and the King-of-the-Hill accumulator `E18--dispatch-scope-params` promised, on one dev map; and an effective-occupancy column beside the raw count in the dev-tools Triggers tab.
- **Non-goals.**
  - Hard-disarm-cancels-pending-exit. `E18--timed-reaction-steps` filed it under this plan's surface; it changes exit obligations, not occupancy.
  - Counting the dead (`includeDead`) or non-player entities.
  - Per-volume refs. Addressing stays tags-only, as for trigger commands.
  - Seams: E25 U2 adds them to the volume set.

### Scripting surface
```typescript
import { defineReaction, defineStore, getMapEntities, occupiedCount, occupants, runtime } from "postretro";
import { onStateCrossing } from "postretro/ui";

const hill = defineStore("hill", {
  control: {
    type: "number", default: 100, range: [0, 100],
    accumulate: (t) => runtime.mul(t.dt, runtime.mul(runtime.read(occupants("hill")), -1)),
  },
});

export function setupLevel() {
  const doors = getMapEntities("mover", { tag: "vault-door" });
  const solve = defineReaction("vault.solve", { sequence: doors.flatMap((d) => d.start()) });
  return {
    reactions: [solve],
    crossings: [onStateCrossing(occupiedCount("vault-plates"), { above: 1 }, [solve])],
  };
}
```
Luau mirrors it: `Postretro.occupiedCount(tag)`, `Postretro.occupants(tag)`. Both ship.

## Acceptance
### Automated
**Counting**
- [ ] Two players on one plate: `occupiedCount` 1, `occupants` 2. One player on each of two plates sharing a tag: 2 and 2.
- [ ] One player inside two overlapping volumes sharing a tag counts once in `occupants` and twice in `occupiedCount`.
- [ ] An undeclared pawn at zero health counts in neither; a pawn with no health component counts; a declared pawn counts by its state's `occupancy` capability.
- [ ] A disarmed or latched volume still counts its occupants.
- [ ] A player on a `switch` press volume carrying the tag counts.

**Puzzles (headless)**
- [ ] Two-button door: the door opens only while both plates hold an effective occupant; releasing one before the second lands suppresses the solve; one player walking plate to plate never solves.
- [ ] N separated plates sharing a tag solve only when all N are held on one tick.
- [ ] A set completing on a tick crosses on that tick. A player standing on both plates at level install solves on the first authoritative tick (pin P1).
- [ ] King-of-the-Hill: control drains at `dt` per tick with one player on the hill, twice that with two, and not at all when empty.
- [ ] Two identical headless runs with scripted inputs produce identical occupancy-slot timelines.

**Slots and lifetime**
- [ ] A script `setState` on an occupancy slot warns and leaves it unchanged.
- [ ] A mod store named `occupancy`, or one overlapping it, is rejected at load with and without a level loaded; a store named `occupancyBoard` loads.
- [ ] A tag containing `.` or `:` warns once at install and gets no slots; the level's other tags get theirs, and its volumes still fire.
- [ ] A crossing or accumulator over a tag with no volume in the level warns and is skipped; the level loads.
- [ ] After a level change, the new level's tags have slots and the old level's tags absent from it have none (pin P2).

**Co-op (two-endpoint)**
- [ ] A client's occupancy slots converge to the host's; its host-mirrored crossing fires once when the set completes; the solve's mover reaches it by replicated phase.
- [ ] A client joining mid-hold reads the current counts from its baseline.
- [ ] A host and client whose maps differ only in a trigger tag are refused at admission with a content-parity reason; matching maps are admitted.

**Surface**
- [ ] The scripting-surface example compiles in TypeScript and Luau, and a TS and a Luau fixture of the two-button puzzle behave identically.
- [ ] The typedef drift check passes with both refs in `postretro.d.ts` and `.d.luau`.
- [ ] With `dev-tools`, the Triggers tab shows effective occupancy beside raw; without it, no occupancy-exposure UI compiles.

### Manual
- [ ] Two-player session on the dev map: each puzzle opens only with both players on its plates; the hill drains faster with both players on it.

## Path
- Shape: engine slots declared per level from the trigger tag set. Rival: mod stores fed by `defineTriggerEvent` enter and exit reactions writing `on.occupancy`. It goes stale: exit fires on physical overlap, so a player downed on a plate fires no exit and the store keeps counting them.
- Who counts: `TriggerStageCapabilities::capture` builds the effective set each tick; `TriggerSystem::effective_occupants` filters a volume's occupants through it. Aggregate per tag from those sets.
- Declaration: `SlotTable` has no teardown; `replace_player_defense_slots` (retain by prefix, reinsert) is the precedent. Insert after `trigger_volume_bridge.populate_from_level` and before `rebuild_reaction_subscribers` and `slot_accumulator_bindings.rebuild` in install; later, watchers skip the slot silently. Both sides already reset the replicated schema at unload (`reset_level_scoped_host_state`, `reset_level_scoped_client_state`).
- Reservation: `validate_namespace_records` rejects `@` today; `occupancy` joins it beside `namespaces_overlap`.
- Engine writes: `write_store_slot` / `apply_store_slot_batch` skip the readonly check; script paths enforce it.
- Refs: `storeRef` (`sdk/lib/data_script.ts`, `.luau`) builds a `{ slot, kind }` ref, as `defineDefenseLayer` does for its engine-owned `player.defense.*` slots; readonly lives in the slot schema, enforced on script writes. Typedef templates `sdk_lib.d.ts` / `sdk_lib.luau`; regenerate with `gen-script-types`.
- Digest: `level_content_digest` (`runtime_movers.rs`) and its `FINGERPRINT_EPOCH`; clients populate trigger volumes themselves, so both sides hash their own.
- Dev-tools: `collect_trigger_diagnostics_rows`, `TriggerDiagnosticsRow`.
- First slice: per-tag aggregation and level-scoped slots on the host with the headless two-button test; then replication and the digest.
- Split first: `trigger_system.rs` is past ~800 lines; move occupant tracking out, behavior-preserving, own commit.
- Docs: `docs/scripting-reference.md` gains the refs and the co-op pattern. At promotion: §12 states the contract; §5 notes level-scoped engine slots beside the engine-global store; `networking.md`'s digest rule covers inputs that shape the replicated slot schema, not only prediction inputs.
- Research: `research.md`.

## Boundary inventory
| Name | Rust | Slot / wire | TypeScript | Luau | FGD KVP |
|---|---|---|---|---|---|
| occupied count | engine slot, readonly, `SharedGlobal` | `"occupancy.<tag>.occupiedCount"` | `occupiedCount(tag): ComputedRef<number>` | `Postretro.occupiedCount(tag)` | n/a |
| occupants | engine slot, readonly, `SharedGlobal` | `"occupancy.<tag>.occupants"` | `occupants(tag): ComputedRef<number>` | `Postretro.occupants(tag)` | n/a |
| namespace | reserved `occupancy` | — | store name rejected | same | n/a |

## Open questions
- Whether the dev map is new or extends an existing co-op test map — **delegated**.
- Whether a level with no tagged occupancy volume declares an empty `occupancy` namespace — **delegated**.
