# E18--coop-activation-policy

Brief · resumable · Epic 18 · reads: `context/lib/scripting.md` §5, §12 · `context/lib/networking.md` §What gates, and what replicates instead, §Slot lifecycle, §Session-state ledger · `context/lib/ui.md` · `context/research/coop-session-lobby.md` §3, §5 · `context/lib/entity_model.md` §7 · `plans/done/E18--player-life-states/` · `plans/done/E18--trigger-event-params/` · `plans/done/E18--dispatch-scope-params/` · read at 6c862fd

**Sequence:** after the first-command presence fix (direct build: a remote pawn is present for triggers and touch from spawn, while movement stays command-gated).

## Problem
Owner-raised, reopened. Co-op puzzles need "both plates held at once" and "every player is in the room", and co-op HUDs need "who is here, and who are we waiting for". Scripts can read none of it. The trigger stage publishes occupancy only as the per-fire `on.occupancy` count; nothing tells a script how many players are taking part or what each is doing; and a HUD can only read the local player. The 2026-07-14 plan for the occupancy half was moved to `done/` without code, and downstream plans (`E18--dispatch-scope-params`' King-of-the-Hill fixture, E25's wait-for-everyone seam) still wait on it. The engine-state catalog behind `getGameState()` cannot express these facts: no entry is host-computed and shared with every machine, none is keyed by data, and the UI cannot repeat a widget over a list. When done, `getGameState()` carries the session's players (per-player status and name, and counts by status) and a per-tag `occupancy` family on host and clients, and the UI can render one row per player.

## Decisions
- **Engine facts stay one API: the catalog behind `getGameState()`** (`scripting.md` §5). The catalog gains these concepts, each with generated typedefs and drift guards like every other entry:
  - **Host-computed shared entries**, replicated `SharedGlobal` to every machine through the existing slot replication, late-join baseline included. A crossing reading only them is host-mirrored (`scripting.md` §12).
  - **Families**: entries keyed by data, reached through a generated accessor. A **list** is a family keyed by row, `0` to a fixed capacity, each row carrying a `present` flag.
- **Player status.** Each seat in the session has one status, derived on the host: `loading` (connected, no pawn yet, including a client that never finishes loading), `inPlay` (pawn spawned and counted by the effective rule, `plans/done/E18--player-life-states/`: a declared pawn by its state's `occupancy` capability, an undeclared one unless its health is depleted), `down` (pawn spawned, not counted), `reconnecting` (held after a disconnect). The host's own player is a seat like any other.
- **`session.playerCounts`**: host-computed shared entries `connected` (`loading` + `inPlay` + `down`), `loading`, `inPlay`, `down`, `reconnecting`. Gameplay reads these. They read 0 at unload and on a machine with no session level, including a client that has left.
- **The lobby's session facts start here** (roadmap Phase 3.75, `research/coop-session-lobby.md`). `session.playerCounts` is the player-count fact its session scope reads, and `session.players` with `forEach` answers its open roster-display question. The lobby surface adds facts beside these, not a second count.
- **`session.players`**: a list, one row per seat (dense, ordered by seat, capacity the session's seat limit), each row `present`, `name`, `status`, `isLocal`. Each machine fills it: clients from the session roster, the host and single player from the seat table. It is for display; gameplay reads `playerCounts`.
- **Names.** A row's `name` is the player's display name, which will come from the character save file; until that exists, and whenever a name is empty, it reads `Player N`, N being the row number from 1. Names and statuses ride the session roster to every peer, rewriting the contract that names stay host-local (`networking.md` §Session-state ledger). Status-only roster changes are rate-limited so a flapping life state cannot flood the reliable channel.
- **`occupancy.tag(tag)`**: a family of readonly Numbers, host-computed and shared. `occupiedCount`: player-occupied volumes, those carrying the tag with at least one effective player inside. `occupants`: distinct effective players inside any volume carrying the tag. Membership ignores armed and latched state, as the occupant map does. Volumes are trigger volumes, `switch` included, since a switch is a trigger volume at the component layer.
- **All-players condition.** `occupants <= inPlay` on every tick. "Everyone is in" is `loading == 0 && inPlay > 0 && occupants >= inPlay`: it waits for loading teammates, is not blocked by a downed one, and is false with nobody in play. The docs and the reference fixture carry exactly this pattern.
- **`forEach(list, (row, i) => widget)`**: a UI helper that unrolls a widget once per row up to the list's capacity, shows a row only while `present`, and makes authored ids unique per row. Trees stay static: rows appearing or leaving never rebuild them. Both runtimes. The HUD player list is a reference implementation; rendering stays the mod's.
- **Referenced tags.** A family exists for each tag the mod's scripts reference through `occupancy.tag`, collected from the evaluated descriptors (crossings, accumulators, reactions, UI binds), never from source text, so a tag built at run time counts. Mod-scope references are declared at each mod commit and live as long as the mod's other store slots (`scripting.md` §5). A level's `setupLevel` references are declared at its install, before binds, crossings and accumulators bind, and removed at unload. Host and clients run the same scripts, so each derives the same set at the same point, a late joiner included; no message carries it. A family reads 0 where no trigger volume in the level carries its tag. Engine-written, script-readonly: a script write warns and changes nothing.
- **A level reference to an absent tag warns.** A `setupLevel` reference to a tag no trigger volume in that level carries warns once at install; a mod-scope reference reads 0 silently on levels without the tag.
- **`occupancy` is a reserved engine namespace.** A mod store declaring it, or one overlapping it, is rejected at load whether or not a level is loaded.
- **A tag that cannot form a slot name** (one containing `.` or `:`) warns once where it is collected and gets no family.
- **Write order.** Counts and occupancy are written once per authoritative host tick, from that tick's trigger-stage capture and the seat table, after the trigger stage settles and before accumulators and crossing detection. A set that completes on a tick crosses in that tick's frame (`plans/done/E18--dispatch-scope-params/` ordering); a hold that starts and breaks inside one catch-up frame may not cross.
- **First consumers ship here:** the two-button co-op puzzle, today a design spike on a store workaround; an all-players room gate; the King-of-the-Hill accumulator `E18--dispatch-scope-params` promised; a HUD player list showing who is loading and who is down; all on one dev map; and an effective-occupancy column in the dev-tools Triggers tab.
- **Modder docs ship here.** `docs/scripting-reference.md` documents `session.players`, `session.playerCounts`, the `occupancy` family, `forEach` and the co-op patterns.
- **Non-goals.**
  - The character save file. Names fall back until it exists.
  - Per-player status in gameplay conditions. Gameplay reads `playerCounts` and host-side player events; the list is for display.
  - Shared per-player slot replication. The roster carries per-seat facts, including seats with no pawn, which slots cannot.
  - Moving defense layers into families. They are keyed by mod vocabulary, owner-private, and reached through `defineDefenseLayer`.
  - Hard-disarm-cancels-pending-exit. `E18--timed-reaction-steps` filed it under this plan's surface; it changes exit obligations, not occupancy.
  - Counting the dead or non-player entities. A mod that wants a downed state counted declares `occupancy: true` on it.
  - Per-volume occupancy. Addressing stays tags-only, as for trigger commands.
  - Seams: E25 U2 adds them to the volume set.
  - Level-script parity. `networking.md` §What gates leaves level-local script content unhashed; a host and client running different scripts diverge here as they do elsewhere.

### Scripting surface
```typescript
// mod script
import { defineMod, defineStore, getGameState, runtime } from "postretro";
const { occupancy } = getGameState();
export const hill = defineStore("hill", {
  control: {
    type: "number", default: 100, range: [0, 100], network: "shared",
    accumulate: (t) => runtime.mul(t.dt, runtime.mul(runtime.read(occupancy.tag("hill").occupants), -1)),
  },
});
export default defineMod({ stores: [hill] });

// level script
import { defineReaction, getMapEntities, getGameState, runtime } from "postretro";
import { onStateCrossing } from "postretro/ui";
const { session, occupancy } = getGameState();
const counts = session.playerCounts;

export function setupLevel() {
  const doors = getMapEntities("mover", { tag: "vault-door" });
  const solve = defineReaction("vault.solve", { sequence: doors.flatMap((d) => d.start()) });
  const allIn = defineReaction("airlock.allIn", { sequence: doors.flatMap((d) => d.start()) });
  const room = runtime.read(occupancy.tag("airlock").occupants);
  const inPlay = runtime.read(counts.inPlay);
  return {
    reactions: [solve, allIn],
    crossings: [
      onStateCrossing(occupancy.tag("vault-plates").occupiedCount, { above: 1 }, [solve]),
      onStateCrossing(
        runtime.and(runtime.eq(runtime.read(counts.loading), 0),
          runtime.and(runtime.gt(inPlay, 0), runtime.ge(room, inPlay))),
        [allIn],
      ),
    ],
  };
}

// HUD
import { VStack, HStack, Text, bindState, stateEquals, forEach } from "postretro/ui";
const roster = VStack({ gap: 4 }, ...forEach(session.players, (p) =>
  HStack({ gap: 8 },
    Text({ bind: bindState(p.name) }),
    Text({ text: "loading", visibleWhen: stateEquals(p.status, "loading") }),
    Text({ text: "down", visibleWhen: stateEquals(p.status, "down") }),
  )));
```
Luau mirrors it: `game.occupancy:tag("airlock").occupants`, `game.session.playerCounts.inPlay`, `forEach(game.session.players, function(p, i) … end)`. Both ship.

## Acceptance
### Automated
**Status and counts**
- [ ] A host with two clients reads `inPlay` 3; one client loading reads `loading` 1, `inPlay` 2, `connected` 3; a downed declared pawn reads `down` and leaves `inPlay`; a disconnected client reads `reconnecting` until its hold expires, then leaves every count.
- [ ] A client that never finishes loading stays `loading` until it disconnects; a client disconnecting while loading leaves `loading` and `connected` on the next tick (pin P15).
- [ ] A joiner's pawn counts `inPlay` from the tick it spawns, before its first command arrives (pin P16).
- [ ] An undeclared client pawn at zero health reads `down` and is no occupant; a pawn with no health component counts; a declared pawn counts by its state's `occupancy` capability (pin P17).
- [ ] Single player reads `inPlay` 1, or `down` with `inPlay` 0 when dead; a client that leaves reads every count 0 at the frontend (pin P14).
- [ ] Every tick, each tag's `occupants` is at most `inPlay`.
- [ ] A client demoted mid-level reads `loading` from its pawn's despawn until its new pawn spawns; the host's seat reads `loading` between install and its pawn spawning (pin P26).

**Occupancy**
- [ ] Two players on one plate: `occupiedCount` 1, `occupants` 2. One player on each of two plates sharing a tag: 2 and 2.
- [ ] One player inside two overlapping volumes sharing a tag counts once in `occupants` and twice in `occupiedCount`.
- [ ] A disarmed or latched volume still counts its occupants; a player on a `switch` press volume carrying the tag counts.
- [ ] A volume whose tags repeat one tag counts once in that tag's `occupiedCount`, and its occupant once in `occupants` (pin P11).
- [ ] A player killed inside the room on a tick, with another player outside, leaves `inPlay` and `occupants` on the same later tick; the all-players gate never crosses (pin P7).

**Puzzles (headless)**
- [ ] Two-button door: the door opens only while both plates hold an effective occupant; releasing one before the second lands suppresses the solve; one player walking plate to plate never solves.
- [ ] N separated plates sharing a tag solve only when all N are held on one tick.
- [ ] All-players gate with one, two and three players: it opens in the frame the last in-play player enters, and not before.
- [ ] All-players gate with a downed player outside: it opens when every in-play player is inside. With nobody in play: it stays closed (pin P12).
- [ ] After a level change and after a same-map restart, with real client endpoints, a gate the party spawns inside stays closed while any client still loads and opens in the frame the last one counts inside (pin P13).
- [ ] A player admitted after the gate opened turns it false; it fires again when they enter (pin P18).
- [ ] With one tick per frame, a set completing on a tick crosses in that frame; a lone host standing inside at install crosses in the first authoritative frame (pin P1).
- [ ] King-of-the-Hill: control drains at `dt` per tick with one player on the hill, twice that with two, and not at all when empty.
- [ ] Two identical headless runs with scripted inputs, on a harness that drives the trigger stage, produce identical timelines for every count (pin P6).

**Player list**
- [ ] Each machine's `session.players` rows match the session's seats in seat order: `present`, `status` and `isLocal` correct, absent rows `present` false; a seat leaving shifts later rows up, and `isLocal` follows the local seat to its new row (pin P25).
- [ ] An empty name reads `Player N` by row; a non-empty name reads as sent.
- [ ] A life state flipping every tick publishes status-only roster updates no faster than the rate limit; every client's row shows the final status within one interval of the last flip, and another seat's status change during the flapping reaches clients within one interval (pin P19).
- [ ] While a level is installed, the list's rows by status equal the counts on the host every tick, and on each client within one rate-limit interval of the last status change; this holds after an undeclared client pawn dies at zero health and respawns, and after a joiner's pawn binds (pin P24).
- [ ] `forEach` over `session.players` builds one row per capacity index, shows only present rows, gives each row's authored ids a unique suffix, resolves a `labelledBy` or `focusNeighbors` reference to an id inside the same row to that row's copy, and never rebuilds the tree as players join and leave.

**Catalog and lifetime**
- [ ] A script `setState` on any count, row field or occupancy slot warns and leaves it unchanged.
- [ ] A mod store named `occupancy`, or one overlapping it, is rejected at load with and without a level loaded; a store named `occupancyBoard` loads; the engine's own family insert succeeds (pin P5).
- [ ] A referenced tag containing `.` or `:` warns once and gets no family; the other referenced tags get theirs.
- [ ] A `setupLevel` reference to a tag no volume in the level carries reads 0 and warns once at install; the King-of-the-Hill accumulator, a mod-scope reference, reads 0 on a level without a hill and warns nothing.
- [ ] A tag built at run time from `getMapEntities` in `setupLevel` gets its family.
- [ ] A level UI tree binding a tag only its `setupLevel` references installs without an unknown-slot warning, and its bind reads the family.
- [ ] After a level change, the families are the mod-scope ones plus the new level's references; the old level's references absent from both have none (pin P2).
- [ ] A staged reload that changes the mod-scope references redeclares them on host and clients, and state batches keep applying (pin P20).
- [ ] A staged reload mid-level keeps the installed level's families and every kept family's value; a mod reference the reload adds is declared before crossings rebind and its crossing binds; a crossing held true across the commit does not fire again (pin P22).
- [ ] A tag that both the mod and the unloaded level reference keeps its family after unload, and the King-of-the-Hill accumulator binds on the next level (pin P23).
- [ ] After a level change and after a same-map restart, every family, mod-scope included, reads 0 at install on host and client, and every count reads 0 from unload until the host's install writes it; a threshold crossing over a family with the lone host on its volume crosses in the first authoritative frame, and a below crossing over an empty mod-scope family does not fire (pin P21).
- [ ] Typedefs for the counts, the player list and the `occupancy` family are generated from the catalog; the drift guards cover them; a data script calling `occupancy.tag` at module scope evaluates in the build-time bridge and at install alike (pin P4).

**Co-op (two-endpoint)**
- [ ] A client's counts and occupancy converge to the host's; its host-mirrored crossing fires once when the set completes; the solve's mover reaches it by replicated phase.
- [ ] A client joining mid-hold reads the current values from its baseline; its crossing fires once in that frame and the host's does not fire again (pin P10).
- [ ] A client joining after the host has visited other levels derives exactly the host's families, and its state batches apply from its first snapshot (pin P8).

**Surface**
- [ ] The scripting-surface example compiles in TypeScript and Luau, and TS and Luau fixtures of the two-button puzzle and the HUD list behave identically.
- [ ] `docs/scripting-reference.md` documents the counts, the player list, `forEach`, the `occupancy` family and the all-players pattern.
- [ ] With `dev-tools`, the Triggers tab shows effective occupancy beside raw; a build without `dev-tools` succeeds and no non-dev-tools module names the column.

### Manual
- [ ] Two-player session on the dev map: each puzzle opens only with both players on its plates; the room gate opens only with both inside; the hill drains faster with both on it; the HUD list shows both players, a joiner as loading, and a downed player as down.

## Path
- Shape: catalog entries, so the facts arrive with `getGameState()` typing, generation and drift guards. Rivals: top-level `occupiedCount(tag)` functions over hand-declared slots, a third way to read engine state; mod stores fed by trigger events, which go stale because a player downed on a plate fires no exit; shared per-owner slots for status, weeks of wire work that still cannot represent pawnless seats.
- Catalog: `BUILTIN_ENGINE_STATE`; generation `emit_ts_game_state_refs` and its Luau twin, runtime trees in `game_state_refs.rs` (interior nodes are method-free today; `byPlayer` is the accessor precedent); guards `build_time_game_state_mirror.rs`, `build_time_by_player_installers_match_runtime_bridge`.
- Status: derive in one host function from `SeatTable` (`client_bindings`, `pawn_bindings`, `hold_deadlines`) and the life-state rule. Hold causes in transport mean level load or parity, not the seat-level reconnect hold.
- Roster: `RosterEntry` / `SessionRosterMessage`, `publish_dirty_roster`, `apply_client_session_roster` (the client keeps the message in `ClientSessionStatus`); mark dirty on pawn bind and unbind and life-state change. Appending fields changes positional bitcode: bump `WIRE_VERSION` (39); update `roster_entry_fields_stay_claim_free`. Names: `ConnectClaim.display_name`, capped at `DISPLAY_NAME_MAX_BYTES`; the host's seat has no claim.
- `forEach`: `Switch` in `sdk/lib/ui/state.ts` is the unroll precedent; `repeat` is already a container prop. Row capacity is the session seat limit (`MAX_CLIENTS` plus the host).
- Occupancy: `TriggerStageCapabilities::capture`, `TriggerSystem::effective_occupants`; aggregate per tag. Families: `replace_player_defense_slots` (retain by prefix, reinsert), called at mod commit (`defense_vocabulary.rs`). Collect references from lowered descriptors: crossing `slot` and `IrNode::slot_read_names`, accumulator IR, reaction `for_each_input`, and UI bind JSON as `warn_unknown_slot_binds` walks it. At mod commit walk every committed global and the store accumulators, not the level-filtered active set. Insert level references after the data script runs and before `rebuild_reaction_subscribers` and `slot_accumulator_bindings.rebuild`; the host's pawn spawns after both, so the install count comes from seats. The state schema already resets at level end and declaration change (`reset_schema`). Reservation goes on the mod-declaration paths (`StoreDeclarationSet::add`, `plan_reconcile`), not `validate_namespace_records`, which also serves engine inserts.
- Phases: (1) shared catalog entries, status and counts, replicated; (2) roster names and statuses, the player list and `forEach`, with the HUD reference; (3) occupancy families and the puzzles; (4) docs, dev-tools, manual check. Each phase ends green and committed.
- Split first: `trigger_system.rs` is past ~800 lines; move occupant tracking out, behavior-preserving, own commit.
- Docs at promotion: §5 states families, lists and level-scoped engine slots; §12 the occupancy and player contracts; `networking.md` the roster's names and statuses and level-scoped shared slots; `ui.md` `forEach`.
- Research: `research.md`.

## Wire format
- `RosterEntry` appends `name` (UTF-8 string, at most `DISPLAY_NAME_MAX_BYTES`, empty when unknown) and `status` (`u8`: 0 loading, 1 inPlay, 2 down, 3 reconnecting) after `connected`, encoded as its siblings are; `WIRE_VERSION` bumps.
- No snapshot or PRL change.

## Boundary inventory
| Name | Rust | Slot / wire | TypeScript | Luau | FGD KVP |
|---|---|---|---|---|---|
| player counts | catalog, host-computed, `SharedGlobal`, readonly | `"session.playerCounts.connected"`, `.loading`, `.inPlay`, `.down`, `.reconnecting` | `getGameState().session.playerCounts.inPlay: ComputedRef<number>` | `game.session.playerCounts.inPlay` | n/a |
| player list | catalog list, each machine writes, readonly | `"session.players.<row>.present"`, `.name`, `.status`, `.isLocal`; roster `name`, `status` | `getGameState().session.players`, row `p.name` etc. | `game.session.players` | n/a |
| status values | enum | `"loading"`, `"inPlay"`, `"down"`, `"reconnecting"` | same strings | same | n/a |
| row repeat | — | — | `forEach(list, (row, i) => widget)` from `"postretro/ui"` | `forEach(list, function(row, i) … end)` | n/a |
| occupancy family | catalog family keyed by referenced tag, `SharedGlobal`, readonly | `"occupancy.<tag>.occupiedCount"`, `"occupancy.<tag>.occupants"` | `getGameState().occupancy.tag(tag).occupants: ComputedRef<number>` | `game.occupancy:tag(tag)` | n/a |
| namespace | reserved `occupancy` | — | store name rejected | same | n/a |

## Open questions
- The status-only roster rate limit's value — **delegated**.
- Whether the dev map is new or extends an existing co-op test map — **delegated**.
- Whether a mod with no occupancy reference declares an empty `occupancy` namespace — **delegated**.
