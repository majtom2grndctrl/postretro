# coop--character-and-run-saves

Brief · resumable · reads: `context/lib/scripting.md` §5 (Durable State Store), §10.4 · `context/lib/networking.md` §Session-state ledger, §Admission and content parity · `context/lib/player_options.md` §1 · `context/lib/ui.md` §1 · read at acd5eae8

## Problem
The owner wants players to keep their characters and runs and to start new ones. Basis: a requested capability, plus one defect. Today one `state.json` per mod holds global slots, character rows and faction sentiment, written in full only at clean exit. The carried loadout lives only in memory and is never saved. Defect: single-player has no fresh start, because seat 0's carried loadout is cleared only when a seat is released, which single-player never does. When this is done, a player picks a character and a run at the frontend. Continue resumes that run at the start of the level it last entered, with the character's gear. New game starts the run over: from nothing, or, in New Game Plus, with the character's current gear. Each character's progress and gear live on its owner's device and go with it to any host, which checks the gear against its own mod. Saves land at every level boundary, not only at exit, so a crash costs at most one level. The durable part of the session-state ledger gets one serialization, which the run and character files use first and which revisited-map state and host migration reuse later.

## Decisions
- **Three save kinds, each its own file under the mod's data directory.** Character slots hold one character each: its per-owner `persist` slots, its display name and its whole carried loadout. The world file is one per mod and holds `persist: true` global slots: progress that outlives any run, such as an unlocked episode. Run slots hold one run each: the catalog id of the level the run last entered, `persist: "run"` global slots and faction sentiment. This applies the decided split of character from world progress (`context/research/coop-session-lifecycle.md` §7). Run state is the third kind, so that New game can reset a run without wiping lasting progress.
- **One serialization for the durable session-state ledger.** The ledger document holds the level's catalog id, global slot values, faction sentiment and a list of seat records; each seat record holds a claim, a display name, per-owner slot values and a carried loadout. A run file is a ledger document with no seat records, because characters own seat state; it records only its character's id. A character file is one seat record. A host-migration handoff will be a document with every seat's record plus a live layer, and revisited-map state a further named section. Readers ignore a named section they do not know and writers keep it, so adding one needs no version bump. Connections, hold deadlines and level-scoped placement are live state and never serialize (`networking.md` §Session-state ledger). This settles the shared format that `coop-session-lifecycle.md` §7 wanted decided before any of the three ships.
- **Format: versioned JSON, written atomically as `state.json` is today.** This replaces `player_options.md` §1's "SQLite candidate". The data is kilobytes, the versioned and atomic writer already exists, and the files stay diffable. Game saves stay a separate store from player options.
- **Migration from `state.json` is one-way and keeps the source.** On first boot without the new files, its local player's rows become character slot 1, `persist: true` globals become the world file, and run-scoped globals and faction sentiment become run slot 1. `state.json` stays on disk, so an older build still reads it. Once the new files exist it is never read again.
- **Each character has its own random id, and that id is the claim it joins with.** The host keys rejoin and per-owner rows by the asserted claim (`networking.md` §Session-state ledger), so two characters on one device must not share one. Migration gives slot 1 the existing `player_id`, which keeps a host's held seat working across the upgrade. The character's display name becomes the claim's display name and so the roster name (`E18--coop-activation-policy`, which deferred it to this file).
- **Gear always belongs to the character.** The whole carried loadout saves with the character; no author switch exists. Gear therefore goes from single-player to co-op and from host to host. This replaces the earlier lean toward an author's choice between character and run (`coop-session-lifecycle.md` §7). Gear kept with a run would make a host's run save hold its guests' gear, which breaks the rule that a host never writes a guest's character. The consequence: a character's runs share its gear, and a player who wants separate gear per run uses a second character. Magazines carry but heat and cells do not, as `entity_model.md` already says.
- **Co-op: the host validates gear and never stores it.** A joining client sends its character's loadout with its join seed. Under the default `joinGear: "accept"`, the host admits each part only if it names a weapon, ammo type or defense layer this mod defines and is within that mod's caps. A part that fails is dropped with a warning, and the rest applies: `apply_join_seed`'s rule for slots. Under `joinGear: "strip"` the host ignores the joiner's loadout and spawns it with nothing carried, so a pistol-start co-op mod stays possible. The client's saved gear is untouched, and what it earns there is saved. The host returns the loadout to its owner at every level boundary and in its final-character message, and the client saves it. This departs from E15's rule that seat contents never cross the wire (`E15--seat-session-identity-roster` I8, `networking.md` §Session-state ledger). That rule keeps the transport blind to the registry and keeps one player's state off others' machines. Both purposes hold: the wire type is a names-only mirror with no dependency on the engine crates, and a loadout travels only between the host and that seat's owner, never on the roster. A host never writes a guest's character (`E16--per-player-persistence`), and validation checks content, not trust (`coop-session-lobby.md` §1).
- **Run scope is a `persist` value on global slots.** `persist: "run"` puts a slot in the run slot. `persist: true` keeps its meaning for global slots (the world file) and for per-owner slots (the character). Per-owner run scope is refused at declaration, because a host never saves a guest's state.
- **New game is an axis that the mod sets.** `newRun` always resets the run slot's run-scoped slots, faction sentiment and level, and starts at the catalog id it names. Two options, both defaulting to keep, say what the selected character keeps: its loadout and its per-owner progress. Fresh and New Game Plus are two settings of these options, not engine modes; a mod offers whichever combinations its menu shows. This follows `scripting.md` §1: expose the axis, not a catalog of points on it. It also builds E15's carry-reset seam, which treats pistol start as policy (`E15--seat-session-identity-roster`).
- **Slot counts are mod-declared and capped by the engine.** Characters and runs each get their own count. Slots are numbered, and an empty one is a row the menu can show.
- **Save points:** after harvest at every level boundary (the run with the destination's catalog id, plus the character), at clean exit, at a join (through the save entry point that `coop--in-app-host-and-join` extracts), and at a leave. In co-op the host writes its run, world and character, and each client writes only its character. A boundary to a level with no catalog id (`restartLevel` of a path-loaded level) saves the character and leaves the run's level unchanged.
- **A run belongs to one character.** A run records the id of the character that started it. Continue selects that character, and `newRun` records the selected one. A run whose character was deleted is refused with a warning, and deleting the run is still permitted. So a run resumes only with the gear its own character carries; another character's fresh start never empties it.
- **Continue resumes at the start of the saved level.** It installs that level with the character's saved loadout and the run's scoped slots. Hosting from a run follows the same path, and guests bring their own characters. A saved catalog id the mod no longer defines refuses Continue with a warning, and the slot stays as it is.
- **Selection happens only at the frontend, before any play.** A character and a run are selected outside any session and any played level. Inside one, every save verb is refused with a warning. The last selection is stored in the mod's save directory, not in player options, so the two stores stay apart (`player_options.md` §1), and the next boot shows the same character.
- **The store reloads at the frontend.** Selecting a character, Continue and `newRun` reset every slot to its declared default, then overlay the world file, the selected character and, for Continue, the run. Slots that never persist therefore start each run at their defaults, which completes the fresh start. This changes `scripting.md` §5, where the store is never cleared and loads once per process: it still survives level changes and hot reload, and reloads only through these verbs, outside any session and played level.
- **Non-goals:**
  - Mid-level saves, quicksave, and any entity or positional state. Revisited-map state stays its own roadmap item and adds its section to the ledger document.
  - A host-migration handoff (`E15--session-lifecycle`).
  - Cloud sync, accounts, anti-cheat, and import or export of save files.
  - Retiring `loadLevel`. The level picker stays a dev path, and a `loadLevel` outside a run saves no run.
- **Ordering:** build after `coop--in-app-host-and-join` lands. That brief creates the save entry point, and this brief rewrites what it writes.

### Scripting surface
```ts
import { defineMod, defineStore, defineReaction } from "postretro";
import { Button, HStack, Text, bindState, forEach, getGameState, stateEquals,
  continueRun, newRun, deleteRun, selectCharacter, createCharacter, deleteCharacter,
} from "postretro/ui";

const meta = defineStore("meta", {
  episode2: { type: "boolean", default: false, persist: true },     // world file
});
const run = defineStore("run", {
  generatorOn: { type: "boolean", default: false, persist: "run" }, // run slot
});
const hero = defineStore("hero", {
  xp: { type: "number", default: 0, perOwner: true, persist: true }, // character slot
});

const saves = getGameState().saves;
const RUN_SLOTS = 3;
const runReactions = Array.from({ length: RUN_SLOTS }, (_, i) => [
  defineReaction(`runs.continue.${i}`, continueRun(i)),
  defineReaction(`runs.fresh.${i}`, newRun(i, "e1m1", { keepLoadout: false })),
  defineReaction(`runs.plus.${i}`, newRun(i, "e1m1")),   // keeps loadout and progress
  defineReaction(`runs.delete.${i}`, deleteRun(i)),
]).flat();
const runRows = forEach(saves.runs, (row, i) =>
  HStack({}, [
    Text({ content: "EMPTY", bind: bindState(row.levelName) }),
    Button({ label: "CONTINUE", onPress: `runs.continue.${i}`,
             visibleWhen: stateEquals(row.occupied, true) }),
    Button({ label: "NEW GAME", onPress: `runs.fresh.${i}` }),
    Button({ label: "NEW GAME+", onPress: `runs.plus.${i}` }),
    Button({ label: "DELETE", onPress: `runs.delete.${i}` }),
  ]),
);
// Characters follow the same pattern: selectCharacter(i), deleteCharacter(i),
// and createCharacter(i), which takes its name from saves.newCharacterName.

export default defineMod({
  saves: { characterSlots: 4, runSlots: RUN_SLOTS, joinGear: "accept" },
  stores: [meta, run, hero],
  reactions: [...runReactions /* , character reactions */],
  // maps, frontend, uiTrees ...
});
```
- `getGameState().saves` (readonly, engine-written): `characters[]` rows `{ occupied, name }`; `runs[]` rows `{ occupied, levelName, characterName, savedAt }`; `character` and `run`, the selected indices, `-1` for none; `newCharacterName`, script-writable text.
- The verbs are reaction primitives in `postretro/ui`, with Luau mirrors (`UI.continueRun(i)`, …). Indices are 0-based in both languages, as `forEach` row ids are. Every declared slot is a present row, and `occupied` says whether it holds a save. `newRun` needs a selected character and an empty or occupied slot; an occupied slot is overwritten.

## Acceptance
### Automated
**Files**
- [ ] Character, world and run files round-trip every field, including every carried-loadout field.
- [ ] A file whose version is outside the supported range is ignored with a warning, and nothing overwrites it until the player writes that slot. A supported older version loads.
- [ ] Migrating a version-4 `state.json` restores the same store and loadout as loading it at acd5eae8 did. `state.json` is left in place. A second boot reads the new files and does not migrate again. Slot 1's character id equals the old `player_id`.
- [ ] A co-op host's ledger with several seats, each holding a full loadout and per-owner values, serializes and restores into a fresh seat table equal in every durable field. Connections, holds and placement are absent from the document.
- [ ] A ledger document holding a named section this build does not know loads with that section ignored, and a rewrite keeps it byte-for-byte.
- [ ] Two character slots and two run slots stay independent. Deleting one leaves the others byte-identical.

**Save points**
- [ ] A level boundary writes the run with the destination's catalog id, and the character, before the next level installs. If the load then fails, Continue still resumes the destination.
- [ ] `restartLevel` of a level loaded by path writes the character and leaves the run's level unchanged.
- [ ] A crash after a boundary save, simulated by dropping the app without the exit path, loses only play since that boundary.
- [ ] Clean exit, join and leave each write the character. Only the world's runner writes the world and run files.

**Runs**
- [ ] Continue installs the saved level with the character's saved loadout, run-scoped slots and faction sentiment.
- [ ] Continue selects the run's own character, whichever was selected before. Continue on a run whose character was deleted is refused with a warning, and deleting that run is permitted.
- [ ] Continue on a run whose catalog id the mod no longer defines is refused with a warning, and the slot is unchanged.
- [ ] Every `newRun` resets run-scoped slots, faction sentiment and the level, and keeps world slots. Each of the four option combinations keeps or clears exactly the loadout and per-owner progress it names. Omitted options keep both.
- [ ] Single-player: after a fresh `newRun`, loading a second level carries the gear picked up in the first. Then a second fresh `newRun` starts with none.
- [ ] Every save verb is refused with a warning, changing nothing, while a played level is installed or loading, while hosting, and while a client. Each verb is permitted at the frontend.
- [ ] `persist: "run"` on a per-owner slot is refused at declaration, and on a global slot it is accepted.
- [ ] After a run that wrote non-persisted global and per-owner slots, `newRun` and Continue each start with those slots at their declared defaults, and with persisted slots at their saved values. A level change inside the run keeps both.
- [ ] Restarting the app restores the last selected character and run, and the player options file holds neither.

**Co-op**
- [ ] Under `accept`, a join seed loadout naming a weapon, ammo type or layer the host lacks, or over a cap, drops that part with a warning. The rest applies. A valid loadout restores whole.
- [ ] Under `strip`, a valid joiner loadout spawns with nothing carried, the client's file keeps its gear until the next save, and what it then earns is saved.
- [ ] After a level boundary and after a host close, the client's character file holds the loadout the host last sent.
- [ ] The host's save files never hold a guest's rows or loadout.
- [ ] Two characters from one device join a host in turn. Each reclaims only its own held seat.

**Surface**
- [ ] The Scripting surface example runs as a `content/dev` fixture in TypeScript and in Luau.

### Manual
- [ ] Dev mod, single-player: create a character, start a fresh run, play two levels, quit. Continue lands at level 2's start with the gear. A New Game Plus run keeps the gear. A fresh run starts empty.
- [ ] Co-op on loopback: a guest brings gear from its own single-player run, plays, leaves, and keeps the gear earned. The host continues its run later and the guest rejoins.
- [ ] Gamepad only: create, select and delete characters and runs.

## Path
- Seams:
  - The exit save in `App::exiting`, which `coop--in-app-host-and-join` extracts.
  - `PersistedState`, `save_persisted_state`, `load_persisted_state`, `collected_per_owner_only_state` and `restore_store_after_session`.
  - Level boundaries: `harvest_bound_pawns` inside `unload_level`, with the destination from `resolve_level_source` (`LevelLoadEntry.catalog_id`).
  - Wire: `JoinSeed` and `ServerControlMessage::FinalCharacter`.
- Carried loadout serde: `CarriedState` stores names, not ids. Restore already warns and skips unknown names (`compose_wieldable_inventory_slots`, `restore_carried_reserve`, `restore_carried_layers`). The positional fields (`magazines`, `active_slot`) need a guard against a changed `WIELDABLE_SLOT_CAPACITY`.
- Ledger seam: the durable fields of `SeatTable` (`connect_claims`, `carried`) plus the slot table's per-owner rows by seat. `session_id`, `client_bindings`, `pawn_bindings`, `hold_deadlines`, placement and roster state are live.
- Shape chosen: three files with numbered slots. Rival: one SQLite database holding all three. It was rejected for a new dependency and opaque files at kilobyte scale.
- First slice: serialize `CarriedState` into a character file and write it at a single-player level boundary, then Continue from it. This falsifies the riskiest premise: that harvested carry restores whole in a later process.
- `main.rs` and `state_persistence.rs` are past 800 lines. Split the save code out first, behavior-preserving, in its own commit.

## Boundary inventory
| Name | Rust | Wire / serde | TS | Luau |
|---|---|---|---|---|
| run scope | `PersistScope::Run` | `"run"` | `persist: "run"` | `persist = "run"` |
| new-run options | `NewRunOptions { keep_loadout, keep_progress }` | n/a | `{ keepLoadout?, keepProgress? }` | `{ keepLoadout = …, keepProgress = … }` |
| join gear policy | `JoinGear::{Accept, Strip}` | n/a | `"accept"`, `"strip"` | same |
| manifest block | `SavesManifest` | `saves` | `saves: { characterSlots, runSlots, joinGear? }` | `saves = { characterSlots = …, runSlots = …, joinGear = … }` |
| engine slots | n/a | `saves.characters`, `saves.runs`, `saves.character`, `saves.run`, `saves.newCharacterName` | `getGameState().saves.*` | same path |
| save files | n/a | `characters/<slot>.json`, `world.json`, `runs/<slot>.json` under `<data_dir>/<mod_id>/` | n/a | n/a |

## Wire format
- `ClientControlMessage::JoinSeed` and `ServerControlMessage::FinalCharacter` each gain an optional carried loadout. A new `ServerControlMessage` variant carries the owner's loadout at each level boundary. Encoding mirrors the existing Control messages: the serde wire codec, with names as strings, never ids. An absent loadout is the option's `None`. The handshake bumps `WIRE_VERSION` by the rule already guarded in `crates/net/src/handshake.rs`.

## Open questions
- The engine caps on character and run slot counts. — **delegated**
- Whether `savedAt` is wall-clock time, and how it reaches a string the menu shows. — **delegated**
