# E18--coop-activation-policy

Brief · compact · Epic 18 · reads: `context/lib/scripting.md` §5, §12 · `context/lib/networking.md` §What gates, and what replicates instead, §Slot lifecycle, §Session-state ledger · `context/lib/entity_model.md` §7 · `plans/done/E18--player-life-states/` · `plans/done/E18--trigger-event-params/` · `plans/done/E18--dispatch-scope-params/` · read at 6c862fd

## Problem
Owner-raised, reopened. Co-op puzzles need "both plates held at once" and "every player is in the room", and scripts can read neither. The trigger stage tracks effective occupants per volume but publishes them only as the per-fire `on.occupancy` count, and nothing tells a script how many players are taking part. The 2026-07-14 plan for this was moved to `done/` without code, and downstream plans (`E18--dispatch-scope-params`' King-of-the-Hill fixture, E25's wait-for-everyone seam) still wait on it. The engine-state catalog behind `getGameState()` cannot express them: no entry is host-computed and shared with every machine, and none is keyed by data. When done, `getGameState()` carries `session.activePlayers` and a per-tag `occupancy` family on host and clients, and scripts build count-threshold and all-players puzzles from the existing crossing watcher and accumulators.

## Decisions
- **Engine facts stay one API: the catalog behind `getGameState()`** (`scripting.md` §5). The catalog gains these concepts, each with generated typedefs and drift guards like every other entry:
  - **Host-computed shared entries.** The host writes them each authoritative tick; they replicate `SharedGlobal` to every machine through the existing slot replication, late-join baseline included. A crossing reading only them is host-mirrored (`scripting.md` §12).
  - **Keyed families.** An entry whose path holds a data key, declared per level from level data, reached through a generated accessor.
- **`session.activePlayers`**: the seats taking part in the current level, the host's own player included. A seat counts while it loads the level and, once its pawn spawns, while that pawn counts under the effective rule (`plans/done/E18--player-life-states/`): a declared pawn by its state's `occupancy` capability, an undeclared one unless its health is depleted. A held or disconnected seat does not count. Host-computed shared entry; reads 0 on a machine with no session level, including a client that has left.
- **`occupancy.tag(tag)`**: a keyed family of readonly Numbers. `occupiedCount`: player-occupied volumes, those carrying the tag with at least one effective player inside. `occupants`: distinct effective players inside any volume carrying the tag. Membership ignores armed and latched state, as the occupant map does.
- **`activePlayers` bounds `occupants`.** Every player `occupants` can count is in `activePlayers`, so `occupants >= activePlayers` with `activePlayers > 0` means every taking-part player is inside, and a player still loading holds it false. The documented all-players pattern carries the `> 0` guard; with none counted, the bare comparison is true.
- **Volumes are trigger volumes**, `switch` included, since a switch is a trigger volume at the component layer.
- **Lifetime.** The host's level install declares the family at 0 for every tag on an occupancy volume, and writes `activePlayers`, before reactions, crossings and accumulators rebuild; unload removes the family on every machine. Engine-written, script-readonly: a script write warns and changes nothing.
- **The host sends the tag set** (`networking.md` §What gates: hash only what cannot be replicated). A client never derives it from its own map. The set rides the ordered control channel at host install and at admission, so it precedes the client's first accepted snapshot of the level. The client keeps the latest set and applies it only to the install it names, declaring the family before binds and crossings initialize. A set that arrives after install, or a new set for the same install, replaces the family, and the client's crossings and schema rebuild. The content digest is unchanged.
- **`occupancy` is a reserved engine namespace.** A mod store declaring it, or one overlapping it, is rejected at load whether or not a level is loaded.
- **A tag that cannot form a slot name** (one containing `.` or `:`) warns once at install and gets no family. Its volumes still fire events.
- **Write order.** Both facts are written once per authoritative host tick from that tick's one capture, after the trigger stage settles and before accumulators and crossing detection. A set that completes on a tick crosses in that tick's frame (`plans/done/E18--dispatch-scope-params/` ordering); a hold that starts and breaks inside one catch-up frame may not cross.
- **Unknown tag degrades.** `occupancy.tag(name)` for a tag with no family in the current level is a ref to an undeclared slot: a crossing or accumulator over it warns and skips. No load failure.
- **First consumers ship here:** the two-button co-op puzzle, today a design spike on a store workaround; an all-players room gate; and the King-of-the-Hill accumulator `E18--dispatch-scope-params` promised, on one dev map; and an effective-occupancy column in the dev-tools Triggers tab.
- **Modder docs ship here.** `docs/scripting-reference.md` documents `session.activePlayers`, the `occupancy` family and the co-op patterns, the all-players guard included.
- **Non-goals.**
  - Moving defense layers into keyed families. They are keyed by mod vocabulary, owner-private, and already reached through `defineDefenseLayer`; the family concept leaves room for them.
  - Hard-disarm-cancels-pending-exit. `E18--timed-reaction-steps` filed it under this plan's surface; it changes exit obligations, not occupancy.
  - Counting the dead or non-player entities. One counting rule: a mod that wants a downed state counted declares `occupancy: true` on it.
  - A connected-seat count. `activePlayers` drops when a player goes down; a roster count for scaling difficulty is separate work.
  - Per-volume occupancy. Addressing stays tags-only, as for trigger commands.
  - Seams: E25 U2 adds them to the volume set.

### Scripting surface
```typescript
import { defineMod, defineReaction, defineStore, getMapEntities, getGameState, runtime } from "postretro";
import { onStateCrossing } from "postretro/ui";

const { session, occupancy } = getGameState();

const hill = defineStore("hill", {
  control: {
    type: "number", default: 100, range: [0, 100], network: "shared",
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
      onStateCrossing(
        runtime.and(
          runtime.gt(runtime.read(session.activePlayers), 0),
          runtime.ge(runtime.read(airlock.occupants), runtime.read(session.activePlayers)),
        ),
        [allIn],
      ),
    ],
  };
}

export default defineMod({ stores: [hill] });
```
Luau mirrors it: `local game = Postretro.getGameState()`, `game.occupancy:tag("airlock").occupants`, `game.session.activePlayers`. Both ship.

## Acceptance
### Automated
**Counting**
- [ ] Two players on one plate: `occupiedCount` 1, `occupants` 2. One player on each of two plates sharing a tag: 2 and 2.
- [ ] One player inside two overlapping volumes sharing a tag counts once in `occupants` and twice in `occupiedCount`.
- [ ] An undeclared pawn at zero health counts in neither `occupants` nor `activePlayers`; a pawn with no health component counts in both; a declared pawn counts in both by its state's `occupancy` capability.
- [ ] A disarmed or latched volume still counts its occupants.
- [ ] A player on a `switch` press volume carrying the tag counts.
- [ ] A volume whose tags repeat one tag counts once in that tag's `occupiedCount`, and its occupant once in `occupants` (pin P11).
- [ ] A player killed inside the room on a tick, with another player outside, leaves `activePlayers` and `occupants` on the same later tick; the all-players gate never crosses (pin P7).
- [ ] `activePlayers` counts the host's player, counts a client's seat from admission to the level through loading and spawn, and excludes a held or disconnected seat.
- [ ] A client that leaves the session reads `activePlayers` 0 at the frontend, not the host's last count (pin P14).

**Puzzles (headless)**
- [ ] Two-button door: the door opens only while both plates hold an effective occupant; releasing one before the second lands suppresses the solve; one player walking plate to plate never solves.
- [ ] N separated plates sharing a tag solve only when all N are held on one tick.
- [ ] All-players gate with one, two and three players: it opens on the tick the last counted player enters, and not before.
- [ ] All-players gate with a declared downed player outside the room, whose state does not count: it opens when every other player is inside.
- [ ] All-players gate with nobody counted, a lone player dead outside or every player downed outside: it stays closed (pin P12).
- [ ] After a level change, an all-players gate the party spawns inside stays closed while any client still loads, and opens in the frame the last one's pawn counts inside (pin P13).
- [ ] A player joining while the others wait inside holds the gate closed until they enter; a player disconnecting while outside opens it on the tick their seat stops counting.
- [ ] With one tick per frame, a set completing on a tick crosses in that frame. A party standing inside at level install crosses in the first authoritative frame (pin P1).
- [ ] King-of-the-Hill: control drains at `dt` per tick with one player on the hill, twice that with two, and not at all when empty.
- [ ] Two identical headless runs with scripted inputs, on a harness that drives the trigger stage, produce identical timelines for both facts (pin P6).

**Catalog and lifetime**
- [ ] A script `setState` on `session.activePlayers` or an occupancy slot warns and leaves it unchanged.
- [ ] A mod store named `occupancy`, or one overlapping it, is rejected at load with and without a level loaded; a store named `occupancyBoard` loads; the engine's own family insert succeeds (pin P5).
- [ ] A tag containing `.` or `:` warns once at install and gets no family; the level's other tags get theirs, and its volumes still fire.
- [ ] A crossing or accumulator over a tag with no volume in the level warns and is skipped; the level loads.
- [ ] After a level change, the new level's tags have families and the old level's tags absent from it have none (pin P2).
- [ ] Typedefs for `session.activePlayers` and the `occupancy` family are generated from the catalog; the catalog drift guards cover both, and the typedef drift check passes.
- [ ] A data script calling `occupancy.tag` at module scope evaluates in the build-time bridge and at install alike, and a drift test holds the two `tag` installers equal (pin P4).

**Co-op (two-endpoint)**
- [ ] A client's `activePlayers` and occupancy values converge to the host's; its host-mirrored crossing fires once when the set completes; the solve's mover reaches it by replicated phase.
- [ ] A client joining mid-hold reads the current values from its baseline; its crossing over them fires once in that frame and the host's does not fire again (pin P10).
- [ ] A client whose tag set for the next level, or for a same-map restart, lands in the same control drain as the relevel declares that install's family and applies state batches from its first snapshot (pin P8).
- [ ] On an uncatalogued level, a host reinstall with a changed tag set leaves the client holding exactly the host's new families, and its state batches apply (pin P9).
- [ ] A client whose map differs from the host's only in trigger tags joins and reads the host's families, not its own.
- [ ] A client whose tag set arrives after its install declares the family then: its crossings over occupancy bind, and state batches resume applying (pin P3).
- [ ] A client demoted and re-promoted within a level keeps its families; after a level change it holds only the new level's (pin P2).

**Surface**
- [ ] The scripting-surface example compiles in TypeScript and Luau, and a TS and a Luau fixture of the two-button puzzle behave identically.
- [ ] `docs/scripting-reference.md` documents `session.activePlayers`, `occupancy.tag` and the all-players pattern with its `> 0` guard.
- [ ] With `dev-tools`, the Triggers tab shows effective occupancy beside raw; without it, no occupancy-exposure UI compiles (build gate: a build without `dev-tools` succeeds and no non-dev-tools module names the column).

### Manual
- [ ] Two-player session on the dev map: each puzzle opens only with both players on its plates; the room gate opens only with both inside; the hill drains faster with both on it.

## Path
- Shape: catalog entries, so the facts arrive with `getGameState()` typing, generation and drift guards. Rival: top-level `occupiedCount(tag)` / `occupants(tag)` functions over hand-declared slots; cheaper, but a third way to read engine state beside `getGameState()` and the defense-layer handles. Script-only rival: mod stores fed by trigger enter and exit reactions writing `on.occupancy`; it goes stale, because exit fires on physical overlap and a player downed on a plate fires no exit.
- Catalog: `BUILTIN_ENGINE_STATE` / `EngineStateCatalogEntry` (`crates/entities/src/engine_state_catalog.rs`); no entry is `SharedGlobal` today. Generation: `emit_ts_game_state_refs` and its Luau twin, runtime trees in `game_state_refs.rs`. Guards: `build_time_game_state_mirror.rs` (`MIRRORED_GAME_STATE_LEAVES`), the committed-typedef drift test; regenerate with `gen-script-types`.
- Keyed family precedent: `SlotTable::replace_player_defense_slots` (retain by prefix, reinsert); `SlotTable` has no teardown. Insert after `trigger_volume_bridge.populate_from_level` and before `rebuild_reaction_subscribers` and `slot_accumulator_bindings.rebuild` in install; a crossing over a slot declared later warns and skips until a rebuild. Both sides reset the replicated schema at unload (`reset_level_scoped_host_state`, `reset_level_scoped_client_state`).
- Shared engine writes: `write_store_slot` / `apply_store_slot_batch` skip the readonly check; script paths enforce it. Replication reads engine slots by dotted name (`ReplicatedSlotSchema::build`, `HostStateReplication::ingest_frame`).
- Who counts: `TriggerStageCapabilities::capture` builds the effective set each tick; `TriggerSystem::effective_occupants` filters a volume's occupants through it. Aggregate per tag from it. `activePlayers` adds seats admitted to the level whose pawn is not yet in the capture (loading, or awaiting a first command; see the first-command assessment) to the capture's counted players.
- Consumers: the two-button spike is `content/dev/scripts/coop-two-button-puzzles.ts`; the dev-tools column sits beside the raw count.
- Reservation: `validate_namespace_records` rejects `@` today; `occupancy` joins it beside `namespaces_overlap`.
- Tag set: append a `ServerControlMessage` variant carrying level identity, level life and the tags (application-protocol bump; append keeps `WIRE_VERSION`). Send it beside the spawn-pose hint, where `send_install_spawn_pose_hints` and admission send it, without the `player_spawn` gate; hold it as `ClientSpawnPoseHint` is held. Declare before `warn_unknown_slot_binds` and `rebuild_reaction_subscribers`. A late set redeclares, then `reset_state_slot_schema` and `rebuild_active_reaction_subscribers`; check the Recompose path's other side effects. Accumulators evaluate on the host only (`evaluate_slot_accumulators`).
- Fingerprint: `compute_fingerprint` covers slot identity, type, range and scope, sorted by identity; identical declarations on both sides match. A mismatched batch drops whole and the host keeps sending `FullBaseline`, so the client recovers once declared.
- Dev-tools: `collect_trigger_diagnostics_rows`, `TriggerDiagnosticsRow`.
- First slice: a host-computed shared catalog entry (`session.activePlayers`) end to end, replicated to a client; then the keyed family and the headless two-button test; then the host-sent tag set.
- Split first: `trigger_system.rs` is past ~800 lines; move occupant tracking out, behavior-preserving, own commit.
- Docs at promotion: §5 states the two catalog concepts and level-scoped engine slots beside the engine-global store; §12 states the occupancy contract; `networking.md` gains the tag-set control message and level-scoped shared slots in §Session-state ledger.
- Research: `research.md`.

## Wire format
One `ServerControlMessage` variant, appended to the enum on the control channel and encoded as its siblings are: level identity (catalog id or path), level life (`Option<u64>`), then the tags as a length-prefixed list of UTF-8 strings in the host's sorted order; an empty list means the level has no family. No snapshot or PRL change.

## Boundary inventory
| Name | Rust | Slot / wire | TypeScript | Luau | FGD KVP |
|---|---|---|---|---|---|
| active players | catalog entry, host-computed, `SharedGlobal`, readonly | `"session.activePlayers"` | `getGameState().session.activePlayers: ComputedRef<number>` | `Postretro.getGameState().session.activePlayers` | n/a |
| occupancy family | catalog keyed family, level-scoped, `SharedGlobal`, readonly | `"occupancy.<tag>.occupiedCount"`, `"occupancy.<tag>.occupants"` | `getGameState().occupancy.tag(tag).occupiedCount` / `.occupants: ComputedRef<number>` | `Postretro.getGameState().occupancy:tag(tag)` | n/a |
| namespace | reserved `occupancy` | — | store name rejected | same | n/a |

## Open questions
- Whether the dev map is new or extends an existing co-op test map — **delegated**.
- Whether a level with no tagged occupancy volume declares an empty `occupancy` namespace — **delegated**.
