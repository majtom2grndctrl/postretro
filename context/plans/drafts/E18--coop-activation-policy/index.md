# E18--coop-activation-policy

Brief · compact · Epic 18 · reads: `context/lib/scripting.md` §5, §12 · `context/lib/networking.md` §Admission and content parity, §What gates, and what replicates instead · `context/lib/entity_model.md` §7 · `plans/done/E18--player-life-states/` · `plans/done/E18--trigger-event-params/` · `plans/done/E18--dispatch-scope-params/` · read at 6c862fd

## Problem
Owner-raised, reopened. Co-op puzzles need "both plates held at once" and "every player is in the room", and scripts can read neither. The trigger stage tracks effective occupants per volume but publishes them only as the per-fire `on.occupancy` count, and nothing tells a script how many players are taking part. The 2026-07-14 plan for this was moved to `done/` without code, so `scripting.md` §12, the roadmap and downstream plans (`E18--dispatch-scope-params`' King-of-the-Hill fixture, E25's wait-for-everyone seam) assume facts that do not exist. The engine-state catalog behind `getGameState()` cannot express them: no entry is host-computed and shared with every machine, and none is keyed by data. When done, `getGameState()` carries `session.activePlayers` and a per-tag `occupancy` family on host and clients, and scripts build count-threshold and all-players puzzles from the existing crossing watcher and accumulators.

## Decisions
- **Engine facts stay one API: the catalog behind `getGameState()`** (`scripting.md` §5). The catalog gains two concepts, each with generated typedefs and drift guards like every other entry:
  - **Host-computed shared entries.** The host writes them each authoritative tick; they replicate `SharedGlobal` to every machine through the existing slot replication, late-join baseline included. A crossing reading only them is host-mirrored (`scripting.md` §12).
  - **Keyed families.** An entry whose path holds a data key, declared per level from level data and removed at unload, reached through a generated accessor.
- **`session.activePlayers`**: the number of players the effective rule counts (`plans/done/E18--player-life-states/`): a declared pawn by its state's `occupancy` capability, an undeclared one unless its health is depleted. A seat with no pawn, held or disconnected, is not counted. Host-computed shared entry.
- **`occupancy.tag(tag)`**: a keyed family with two readonly Numbers. `occupiedCount`: volumes carrying the tag with at least one effective occupant. `occupants`: distinct effective players inside any volume carrying the tag. Membership ignores armed and latched state, as the occupant map does. Slots `occupancy.<tag>.occupiedCount` and `occupancy.<tag>.occupants`.
- **One counting rule.** `activePlayers` and `occupants` use the same effective set, so `occupants >= activePlayers` means every counted player is inside.
- **Volumes are trigger volumes**, `switch` included, since a switch is a trigger volume at the component layer. E25's `level_seam` joins through its own inside test (`drafts/E25--landmark-level-change/`).
- **Lifetime.** Each level install declares the family for every tag on an occupancy volume before reactions, crossings and accumulators rebuild; unload removes it. Engine-written, script-readonly: a script write warns and changes nothing.
- **`occupancy` is a reserved engine namespace.** A mod store declaring it, or one overlapping it, is rejected at load whether or not a level is loaded.
- **A tag that cannot form a slot name** (one containing `.` or `:`) warns once at install and gets no family. Its volumes still fire events.
- **Write order.** Both facts are written once per authoritative host tick, after the trigger stage settles and before accumulators and crossing detection, so a set that completes on a tick crosses on that tick (`plans/done/E18--dispatch-scope-params/` ordering).
- **Trigger tags join the level content digest** (epoch bump). Each side derives the family from its own level; a pair whose tags differ is refused at admission instead of dropping every replicated slot at the schema fingerprint check.
- **Unknown tag degrades.** `occupancy.tag(name)` for a tag with no family in the current level is a ref to an undeclared slot: a crossing or accumulator over it warns and skips. No load failure.
- **First consumers ship here:** the two-button co-op puzzle (`content/dev/scripts/coop-two-button-puzzles.ts`, today a design spike on a store workaround), an all-players room gate, and the King-of-the-Hill accumulator `E18--dispatch-scope-params` promised, on one dev map; and an effective-occupancy column beside the raw count in the dev-tools Triggers tab.
- **Non-goals.**
  - Moving defense layers into keyed families. They are keyed by mod vocabulary, owner-private, and already reached through `defineDefenseLayer`; the family concept leaves room for them.
  - Hard-disarm-cancels-pending-exit. `E18--timed-reaction-steps` filed it under this plan's surface; it changes exit obligations, not occupancy.
  - Counting the dead (`includeDead`) or non-player entities.
  - Per-volume occupancy. Addressing stays tags-only, as for trigger commands.
  - Seams: E25 U2 adds them to the volume set.

### Scripting surface
```typescript
import { defineReaction, defineStore, getMapEntities, getGameState, runtime } from "postretro";
import { onStateCrossing } from "postretro/ui";

const { session, occupancy } = getGameState();

const hill = defineStore("hill", {
  control: {
    type: "number", default: 100, range: [0, 100],
    accumulate: (t) => runtime.mul(t.dt, runtime.mul(runtime.read(occupancy.tag("hill").occupants), -1)),
  },
});

export function setupLevel() {
  const doors = getMapEntities("mover", { tag: "vault-door" });
  const solve = defineReaction("vault.solve", { sequence: doors.flatMap((d) => d.start()) });
  const airlock = occupancy.tag("airlock");
  const allIn = defineReaction("airlock.allIn", { sequence: doors.flatMap((d) => d.start()) });
  return {
    reactions: [solve, allIn],
    crossings: [
      onStateCrossing(occupancy.tag("vault-plates").occupiedCount, { above: 1 }, [solve]),
      onStateCrossing(runtime.ge(runtime.read(airlock.occupants), runtime.read(session.activePlayers)), [allIn]),
    ],
  };
}
```
Luau mirrors it: `local game = Postretro.getGameState()`, `game.occupancy.tag("airlock").occupants`, `game.session.activePlayers`. Both ship.

## Acceptance
### Automated
**Counting**
- [ ] Two players on one plate: `occupiedCount` 1, `occupants` 2. One player on each of two plates sharing a tag: 2 and 2.
- [ ] One player inside two overlapping volumes sharing a tag counts once in `occupants` and twice in `occupiedCount`.
- [ ] An undeclared pawn at zero health counts in neither `occupants` nor `activePlayers`; a pawn with no health component counts in both; a declared pawn counts in both by its state's `occupancy` capability.
- [ ] A disarmed or latched volume still counts its occupants.
- [ ] A player on a `switch` press volume carrying the tag counts.
- [ ] `activePlayers` excludes a held or disconnected seat and counts a player the tick their pawn spawns.

**Puzzles (headless)**
- [ ] Two-button door: the door opens only while both plates hold an effective occupant; releasing one before the second lands suppresses the solve; one player walking plate to plate never solves.
- [ ] N separated plates sharing a tag solve only when all N are held on one tick.
- [ ] All-players gate with one, two and three players: it opens on the tick the last counted player enters, and not before.
- [ ] All-players gate with a declared downed player outside the room, whose state does not count: it opens when every other player is inside.
- [ ] A player joining while the others wait inside holds the gate closed until they enter; a player disconnecting while outside opens it on the tick their seat stops counting.
- [ ] A set completing on a tick crosses on that tick. Players standing in place at level install cross on the first authoritative tick (pin P1).
- [ ] King-of-the-Hill: control drains at `dt` per tick with one player on the hill, twice that with two, and not at all when empty.
- [ ] Two identical headless runs with scripted inputs produce identical timelines for both facts.

**Catalog and lifetime**
- [ ] A script `setState` on `session.activePlayers` or an occupancy slot warns and leaves it unchanged.
- [ ] A mod store named `occupancy`, or one overlapping it, is rejected at load with and without a level loaded; a store named `occupancyBoard` loads.
- [ ] A tag containing `.` or `:` warns once at install and gets no family; the level's other tags get theirs, and its volumes still fire.
- [ ] A crossing or accumulator over a tag with no volume in the level warns and is skipped; the level loads.
- [ ] After a level change, the new level's tags have families and the old level's tags absent from it have none (pin P2).
- [ ] Typedefs for `session.activePlayers` and the `occupancy` family are generated from the catalog; the catalog drift guards cover both, and the typedef drift check passes.

**Co-op (two-endpoint)**
- [ ] A client's `activePlayers` and occupancy values converge to the host's; its host-mirrored crossing fires once when the set completes; the solve's mover reaches it by replicated phase.
- [ ] A client joining mid-hold reads the current values from its baseline.
- [ ] A host and client whose maps differ only in a trigger tag are refused at admission with a content-parity reason; matching maps are admitted.

**Surface**
- [ ] The scripting-surface example compiles in TypeScript and Luau, and a TS and a Luau fixture of the two-button puzzle behave identically.
- [ ] With `dev-tools`, the Triggers tab shows effective occupancy beside raw; without it, no occupancy-exposure UI compiles.

### Manual
- [ ] Two-player session on the dev map: each puzzle opens only with both players on its plates; the room gate opens only with both inside; the hill drains faster with both on it.

## Path
- Shape: catalog entries, so the facts arrive with `getGameState()` typing, generation and drift guards. Rival: top-level `occupiedCount(tag)` / `occupants(tag)` functions over hand-declared slots; cheaper, but a third way to read engine state beside `getGameState()` and the defense-layer handles. Script-only rival: mod stores fed by trigger enter and exit reactions writing `on.occupancy`; it goes stale, because exit fires on physical overlap and a player downed on a plate fires no exit.
- Catalog: `BUILTIN_ENGINE_STATE` / `EngineStateCatalogEntry` (`crates/entities/src/engine_state_catalog.rs`); no entry is `SharedGlobal` today. Generation: `emit_ts_game_state_refs` and its Luau twin, runtime trees in `game_state_refs.rs`. Guards: `build_time_game_state_mirror.rs` (`MIRRORED_GAME_STATE_LEAVES`), the committed-typedef drift test; regenerate with `gen-script-types`.
- Keyed family precedent: `SlotTable::replace_player_defense_slots` (retain by prefix, reinsert); `SlotTable` has no teardown. Insert after `trigger_volume_bridge.populate_from_level` and before `rebuild_reaction_subscribers` and `slot_accumulator_bindings.rebuild` in install; later, watchers skip the slot silently. Both sides reset the replicated schema at unload (`reset_level_scoped_host_state`, `reset_level_scoped_client_state`).
- Shared engine writes: `write_store_slot` / `apply_store_slot_batch` skip the readonly check; script paths enforce it. Replication reads engine slots by dotted name (`ReplicatedSlotSchema::build`, `HostStateReplication::ingest_frame`).
- Who counts: `TriggerStageCapabilities::capture` builds the effective set each tick; `TriggerSystem::effective_occupants` filters a volume's occupants through it. Aggregate per tag, and count the set for `activePlayers`.
- Reservation: `validate_namespace_records` rejects `@` today; `occupancy` joins it beside `namespaces_overlap`.
- Digest: `level_content_digest` (`runtime_movers.rs`) and its `FINGERPRINT_EPOCH`; clients populate trigger volumes themselves, so both sides hash their own.
- Dev-tools: `collect_trigger_diagnostics_rows`, `TriggerDiagnosticsRow`.
- First slice: a host-computed shared catalog entry (`session.activePlayers`) end to end, replicated to a client; then the keyed family and the headless two-button test; then the digest.
- Split first: `trigger_system.rs` is past ~800 lines; move occupant tracking out, behavior-preserving, own commit.
- Docs: `docs/scripting-reference.md` gains the facts and the co-op patterns. At promotion: §5 states the two catalog concepts and level-scoped engine slots beside the engine-global store; §12 states the occupancy contract; `networking.md`'s digest rule covers inputs that shape the replicated slot schema, not only prediction inputs.
- Research: `research.md`.

## Boundary inventory
| Name | Rust | Slot / wire | TypeScript | Luau | FGD KVP |
|---|---|---|---|---|---|
| active players | catalog entry, host-computed, `SharedGlobal`, readonly | `"session.activePlayers"` | `getGameState().session.activePlayers: ComputedRef<number>` | `Postretro.getGameState().session.activePlayers` | n/a |
| occupancy family | catalog keyed family, level-scoped, `SharedGlobal`, readonly | `"occupancy.<tag>.occupiedCount"`, `"occupancy.<tag>.occupants"` | `getGameState().occupancy.tag(tag).occupiedCount` / `.occupants: ComputedRef<number>` | `Postretro.getGameState().occupancy.tag(tag)` | n/a |
| namespace | reserved `occupancy` | — | store name rejected | same | n/a |

## Open questions
- Whether the dev map is new or extends an existing co-op test map — **delegated**.
- Whether a level with no tagged occupancy volume declares an empty `occupancy` namespace — **delegated**.
