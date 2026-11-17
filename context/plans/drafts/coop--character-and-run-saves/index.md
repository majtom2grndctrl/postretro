# coop--character-and-run-saves

Brief · resumable · reads: `context/lib/scripting.md` §5 (Durable State Store), §10.4 · `context/lib/networking.md` §Session-state ledger, §Slot lifecycle · `context/lib/player_options.md` §1 · `context/lib/ui.md` §1 · `context/lib/entity_model.md` §7b · `context/lib/build_pipeline.md` §Custom FGD · read at acd5eae8

## Problem
The owner wants players to keep characters and runs and to start new ones. Basis: a requested capability, plus one defect. Today one `state.json` per mod holds global slots, character rows and faction sentiment, written in full only at clean exit, and the carried loadout is never saved. Defect: single-player has no fresh start, because seat 0's carried loadout is cleared only when a seat is released, which single-player never does. When this is done, a player picks a character and a run at the frontend, continues the run from its last level start or checkpoint, or starts a new one. Gear and progress belong to the character and go with it to any host. Saves land at every level start, checkpoint and exit, so a crash costs at most the play since the last of them. The durable part of the session-state ledger gets the one serialization that revisited-map state and host migration reuse.

## Decisions
- **Three save kinds, each its own file under the mod's data directory.** A character slot holds one character: its per-owner `persist` slots, display name and whole carried loadout. The world file, one per mod, holds `persist: true` global slots, progress that outlives any run. A run slot holds one run: its character's id, the level and checkpoint it last reached, `persist: "run"` global slots and faction sentiment. Run state is a third kind so a new run never wipes lasting progress. Faction sentiment moves from world to run progress (`scripting.md` §5).
- **One serialization for the durable session-state ledger.** A ledger document holds a level, global slot values, faction sentiment and seat records. A seat record holds a claim, a display name, per-owner slot values and a carried loadout. A run file is a ledger document with no seat records; a character file is one seat record. Readers ignore a named section they do not know and writers keep it, so revisited-map state and a host-migration handoff each add a section without a version bump. Connections, holds and placement are live state and never serialize.
- **Format: versioned JSON, written atomically as `state.json` is today.** This replaces the SQLite candidate in `player_options.md` §1. Game saves stay a separate store from player options.
- **Migration from `state.json` is one-way and keeps the source.** On first boot without the new files, the local player's rows become character slot 0, with the existing `player_id` as its id, and the global slots become the world file. `state.json` stores no level, so migration creates no run; its faction sentiment goes to the implicit run of a mod with no `saves` block and is dropped otherwise. `state.json` stays on disk for older builds and is never read once migration completes.
- **Each character has its own random id, and that id is the claim it joins with.** The host keys rejoin and per-owner rows by the claim, so two characters on one device must not share one. The display name becomes the claim's display name and so the roster name (`E18--coop-activation-policy`).
- **Gear always belongs to the character.** No author switch exists, so gear goes from single-player to co-op and from host to host. This replaces the earlier lean toward an author's choice (`coop-session-lifecycle.md` §7): gear kept with a run would make a host's run save hold guests' gear, and a host never writes a guest's character. A character's runs share its gear.
- **A fresh start is a new character.** `newRun` starts a run with the selected character as it stands, so a new run with an existing character is New Game Plus. A mod that wants pistol starts offers New game only after `createCharacter`. No verb empties an existing character, so one run can never take another run's gear.
- **A run belongs to one character.** Continue selects the run's character; while hosting, Continue on another character's run is refused. `newRun` writes only into an empty slot or a run of the selected character. A run whose character was deleted is refused with a warning, and deleting the run is still permitted.
- **Co-op: the host checks a joiner's gear and never stores it.** A joining client sends its character's loadout with its join seed. The host keeps each weapon, ammo type and defense layer only if this mod defines it, clamps magazines to the weapon's magazine size and health to the player's maximum, and drops the rest with a warning. On a rejoin into a held seat, the held seat's live loadout wins over the join seed's, as live slot values already do. The host returns the loadout to its owner at every level start and checkpoint, and in a final-character message at a host close and at the client's own leave. This rewrites the rule that seat contents never cross the wire (`networking.md` §Session-state ledger, `E15--seat-session-identity-roster` I8). Its purposes hold: the wire type is a names-only mirror with no engine-crate dependency, and a loadout goes only between the host and that seat's owner.
- **The character saves live; the run saves at level starts and checkpoints.** Every save point writes the character as it is now: per-owner slots and the loadout harvested at that moment. The run file changes only when a level starts or a checkpoint fires. A character file never stores a downed or dead state: it saves the initial life state, and health at or below zero restores to full, as a level change does today. A connected client writes its character whenever a loadout message arrives, and its periodic save keeps its progress slots current.
- **Seat 0's carry always comes from the selected character outside play.** Booting with a selection, `selectCharacter`, `createCharacter`, Continue, `newRun` and a client's leave set seat 0's carry from the selected character's file. Deleting the selected character empties it. A frontend backdrop's pawn is never harvested.
- **Authored checkpoints name a spawn set.** A `checkpoint(name)` step, run by the host or single player, saves the run with that checkpoint and saves the character. A `player_spawn` may carry a `checkpoint` key, and spawns sharing a name form that checkpoint's arrival set. Continue from a checkpoint assigns seats within its set by the ordinary spawn rule (`networking.md` §Slot lifecycle), so co-op players do not stack. Once a checkpoint fires or Continue arrives at one, respawns and late joiners also use its set. Spawns without the key form the level-start set, and keyed spawns are skipped at a level start. A level whose spawns all carry the key uses all of them at its start, with a warning. An unknown name falls back to the level-start set with a warning. A later level start clears the run's checkpoint; Continue's own arrival keeps it. A checkpoint outside an active run saves only the character, and one in the frontend backdrop saves nothing.
- **Run scope is a `persist` value on global slots.** `persist: "run"` puts a slot in the run slot. `persist: true` means the world file on a global slot and the character on a per-owner slot. Per-owner run scope is refused at declaration, because a host never saves a guest's state.
- **Slot counts are mod-declared and capped by the engine.** A mod with no `saves` block gets one character slot and one run slot, created and selected automatically, and its run is active for the whole process, so every existing mod keeps its progress, faction sentiment included. A slot whose file is unreadable or from an unsupported version stays occupied, with `readable` false, until deleted, so no build silently replaces a save it cannot read. Every verb but delete refuses an unreadable slot. A stored selection naming an empty, unreadable or undeclared slot boots as `-1` with a warning. With a `saves` block, hosting, joining and `newRun` are refused until a character is selected.
- **A run is active from Continue or `newRun` until the frontend is entered again by any path.** Save points while it is active: every level start (the run, with the destination's catalog id, then the character), every checkpoint, and every clean exit, join, leave and return to the frontend (the character). The host or single player also writes the world file at each of its save points. A `loadLevel` outside an active run saves only the character. A level with no catalog id (`restartLevel` of a path-loaded level) saves the character and leaves the run's level unchanged. Only the host or single player writes world and run files.
- **Save verbs run only at the frontend.** Inside a played level, and as a connected client, every save verb is refused with a warning. While hosting at the frontend, Continue and `newRun` are permitted, and clients follow into the level. Verbs that change the selection, create or delete are refused while hosting. `createCharacter` into an occupied slot is refused. The last selection is stored in the mod's save directory, not in player options (`player_options.md` §1).
- **The store reloads at the frontend.** Selecting a character, Continue and `newRun` reset every mod-owned slot to its declared default, then overlay the world file, the selected character and, for Continue, the run. Engine-owned slots are untouched, and while hosting only seat 0's per-owner slots reload, so admitted guests keep what they joined with. This changes `scripting.md` §5, where the store loads once per process. It still survives level changes and hot reload.
- **First consumer: the dev mod frontend.** It gains character select, create and delete, a run list with Continue, New game and delete, and a host lobby screen. The lobby shows players joining before the host enters a level, and its Cancel ends the session. The level picker stays as a dev path.
- **Non-goals:**
  - Saving live world state anywhere (save scumming): enemies, doors, pickups and positions. It needs per-entity serialization, so it is its own roadmap item. It adds a live-world section to the ledger document.
  - Revisited-map state and a host-migration handoff (`E15--session-lifecycle`). Each adds its section later.
  - Cloud sync, accounts, anti-cheat, and import or export of save files. The host's gear check keeps data sane; it does not authenticate a client.
  - Retiring `loadLevel`: it stays the dev picker and a mod-authored path.
- **Ordering:** build after `coop--in-app-host-and-join` lands. That brief creates the save entry point and `hostSession()`, and this brief rewrites the one and extends the other.

### Scripting surface
```ts
import { defineMod, defineStore, defineReaction } from "postretro";
import { Button, HStack, Text, bindState, forEach, getGameState, stateEquals,
  continueRun, newRun, deleteRun, checkpoint,
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
  defineReaction(`runs.new.${i}`, newRun(i, "campaign-test")),
  defineReaction(`runs.delete.${i}`, deleteRun(i)),
]).flat();
const runRows = forEach(saves.runs, (row, i) =>
  HStack({}, [
    Text({ content: "EMPTY", bind: bindState(row.levelName) }),
    Text({ content: "", bind: bindState(row.characterName) }),
    Button({ id: "continue", label: "CONTINUE", onPress: `runs.continue.${i}`,
             visibleWhen: stateEquals(row.occupied, true) }),
    Button({ id: "new", label: "NEW GAME", onPress: `runs.new.${i}` }),
    Button({ id: "delete", label: "DELETE", onPress: `runs.delete.${i}` }),
  ]),
);
// Characters follow the same pattern: selectCharacter(i), deleteCharacter(i),
// and createCharacter(i), which takes its name from ui.textEntry.

// In a level script, the host saves a checkpoint; its player_spawns carry checkpoint=bridge.
const reachBridge = defineReaction("level.bridge", checkpoint("bridge"));

export default defineMod({
  saves: { characterSlots: 4, runSlots: RUN_SLOTS },
  stores: [meta, run, hero],
  reactions: [...runReactions, reachBridge /* , character reactions */],
  // maps, frontend, uiTrees ...
});
```
- `getGameState().saves` (readonly, engine-written): `characters[]` rows `{ present, occupied, readable, name }`; `runs[]` rows `{ present, occupied, readable, levelName, characterName, savedAt }`; `character` and `run`, the selected index or `-1`. Every declared slot is a present row.
- The verbs are reaction primitives in `postretro/ui` with Luau mirrors (`UI.continueRun(i)`, …). Each takes the row index as that language's `forEach` gives it: 0-based in TypeScript, 1-based in Luau. `persist` gains a declared type on global slots, so `persist: "run"` is checked, not passed through the open record.

## Acceptance
### Automated
**Files**
- [ ] Character, world and run files round-trip every field, including every carried-loadout field and the checkpoint.
- [ ] A co-op host's ledger with several seats, each with a full loadout and per-owner values, serializes and restores into a fresh seat table equal in every durable field. Connections, holds and placement are absent from the document.
- [ ] A ledger document with a named section this build does not know loads with it ignored, and a rewrite keeps it byte-for-byte.
- [ ] A file whose version is outside the supported range, or that does not parse, is ignored with a warning and never written by this build. Its slot reads occupied and not readable until deleted; `selectCharacter`, Continue and `newRun` on it are refused with a warning and change nothing, and deleting it is permitted. A supported older version loads.
- [ ] A character, run or world save killed before its rename leaves the previous file loadable and byte-identical.
- [ ] Migrating a version-4 `state.json` fixture restores the store recorded from loading it at acd5eae8, with an empty loadout and no run. Character slot 0's id equals the old `player_id`. `state.json` is left in place, and a second boot does not migrate again.
- [ ] A migration killed after each of its writes in turn migrates again at the next boot and ends with the same files as an uninterrupted one (P9).
- [ ] A character file with more wieldable or magazine entries, or a higher active slot, than this build's capacity loads the in-range part, drops the rest with a warning, and selects slot 0 when the active slot is out of range.
- [ ] Two character slots and two run slots stay independent. Deleting one leaves the others byte-identical.

**Save points**
- [ ] A level start writes the run with the destination's catalog id, then the character, after the level-change harvest and before the next level installs. If the load then fails, Continue still resumes the destination.
- [ ] A crash injected between a level start's two writes never resumes the earlier level with the later level's gear (P5).
- [ ] A player crossing a level start in a non-initial life state continues the run in its initial state (P4).
- [ ] After a weapon pickup mid-level, quitting and then Continue starts at the last level start or checkpoint holding that weapon.
- [ ] A crash, simulated by dropping the app without the exit path, loses only play since the last level start or checkpoint.
- [ ] After Continue and `returnToFrontend`, a dev-picker `loadLevel` and its level start leave the run file byte-identical and write the character. So does `restartLevel` of a level loaded by path.
- [ ] A player saved while downed or dead continues in the initial life state at full health.
- [ ] Only the host or single player writes world and run files.

**Checkpoints**
- [ ] A `checkpoint` step saves the run with that checkpoint and saves the character. Continue arrives at a spawn in that checkpoint's set, and in co-op each seat takes a different spawn of the set while one is free.
- [ ] A checkpoint naming no spawn set falls back to the level-start set with a warning. A later level start clears the checkpoint.
- [ ] After a checkpoint fires, and after Continue arrives at one, a respawn and a late joiner take spawns from its set.
- [ ] Continue arrives at checkpoint `bridge`; quitting and Continue again arrives at `bridge`, not the level start (P11). At a level start, keyed spawns are skipped; a level whose spawns are all keyed uses all of them and warns.
- [ ] A `checkpoint` step on a connected client changes nothing. Outside an active run it writes only the character, and in the frontend backdrop it writes nothing.
- [ ] `checkpoint` then `loadLevel`, and `loadLevel` then `checkpoint`, in one reaction: Continue resumes the destination's level start in both. Two `checkpoint` steps in one tick leave the later one saved.

**Runs and characters**
- [ ] Continue selects the run's character, installs the saved level with that character's loadout and the run's scoped slots and sentiment.
- [ ] Continue on a run whose character was deleted, also after a new character is created in the same slot, or whose catalog id the mod no longer defines, is refused with a warning and leaves the slot unchanged. Deleting such a run is permitted.
- [ ] `newRun` into a slot holding another character's run is refused with a warning. Into an empty slot or the selected character's own run it is permitted.
- [ ] `newRun` resets run-scoped slots, faction sentiment and the level, keeps world slots, and keeps the selected character's loadout and progress.
- [ ] `newRun` writes its run file before its level load begins. A crash in its first level resumes the new run there (P7).
- [ ] Single-player, with the dev mod's backdrop declared: a new character's first run carries gear from its first level into its second. A second new character's run starts with none (P2).
- [ ] With the backdrop installed, select character A with gear, then character B with none, then Continue B's run: B spawns empty and A's file is unchanged (P2).
- [ ] Selecting character B after A leaves every per-owner slot at B's saved value or its declared default, and none at A's.
- [ ] After a run that wrote non-persisted mod slots, `newRun` and Continue each start with those slots at their declared defaults and persisted slots at their saved values. Engine-owned slots are unchanged. A level change inside the run keeps both.
- [ ] After `deleteCharacter` on the selected character, a `loadLevel` spawns with an empty loadout and writes no character file (P12).
- [ ] A `persist: true` global written in a run, then `returnToFrontend` and `selectCharacter`, Continue or `newRun`, keeps its value, and a restart keeps it.
- [ ] A boot that restores the last selection and then joins, or loads a level, sends and spawns with that character's loadout, and the join save does not empty it (P1).
- [ ] Restarting the app restores the last selected character and run, and the player options file holds neither. A stored selection naming an empty, unreadable or undeclared slot boots as `-1` with a warning, and no save point writes that slot.
- [ ] Deleting the selected run or character sets `saves.run` or `saves.character` to `-1`, and a restart keeps `-1` (P10).
- [ ] `createCharacter` into an occupied slot is refused with a warning.
- [ ] `saves.characters` and `saves.runs` each hold exactly the declared count of rows, empty ones with `occupied` false. A count above the engine cap is clamped with a warning. Files for slots past a lowered count are kept and not shown.
- [ ] A mod with no `saves` block keeps per-owner and global progress and faction sentiment across a restart, with one character and one run created, selected and active for the whole process, also after returning to the frontend and loading another level. Migrating its `state.json` keeps its faction sentiment.

**Verb gate**
- [ ] Every save verb is refused with a warning, changing nothing, while a played level is installed or loading and while a client. Each is permitted at the frontend with no backdrop, with the backdrop installed, and while the backdrop loads.
- [ ] While hosting at the frontend, Continue on the host character's run and `newRun` are permitted, and clients follow into the level. Continue on another character's run, `selectCharacter`, `createCharacter` and the deletes are refused. With a `saves` block and no selected character, hosting, joining and `newRun` are refused; with one selected, each is permitted.
- [ ] With a guest admitted at the host's lobby, the host's Continue or `newRun` keeps the guest's per-owner values and loadout, and the session's final character returns them unchanged.
- [ ] One reaction firing `continueRun(0)` then `selectCharacter(1)`, and one firing `newRun(0, …)` then `deleteRun(0)`: the first verb takes effect and the second is refused, changing nothing (P6).
- [ ] `persist: "run"` on a per-owner slot is refused at declaration and accepted on a global slot.

**Co-op**
- [ ] A join seed loadout naming a weapon, ammo type or layer the host lacks drops that part with a warning, and a magazine over its weapon's size and health over the player's maximum are clamped. A life state other than the initial one arrives initial, and an active slot past the kept wieldables selects slot 0, each with a warning. A valid loadout restores whole.
- [ ] A guest that drops mid-level and rejoins within the hold spawns with the held seat's loadout, not its file's.
- [ ] A guest that picks up a weapon after the last level start, then sees the host end the session, saves that weapon (P3). A guest that leaves on its own after a pickup also saves it.
- [ ] A client writes its character on each level-start and checkpoint loadout message, and its periodic save afterwards keeps that loadout (P8).
- [ ] A leaving client whose host never answers closes at the bound and saves the loadout its host last sent.
- [ ] With two guests, each loadout message reaches only its own seat's client, and no roster frame carries a loadout.
- [ ] A joining client's roster entry shows its character's name, truncated at the claim's byte cap.
- [ ] The host's save files never hold a guest's rows or loadout.
- [ ] Two characters from one device join a host in turn. Each reclaims only its own held seat.
- [ ] Grep gate: `crates/net` depends on no engine crate, and the loadout wire type holds only strings and integers.

**Surface and docs**
- [ ] `coop--in-app-host-and-join`'s join-then-leave seat-0 carry test, which expects a fresh ledger's carry, is replaced by one where seat 0's carry equals the selected character's saved loadout.
- [ ] `docs/` and the SDK doc comments cover the `saves` block, `persist: "run"`, the save verbs, `checkpoint` and `getGameState().saves`. The FGD gives `player_spawn` a `checkpoint` key.
- [ ] The Scripting surface example runs as a `content/dev` fixture in TypeScript and in Luau, with its slots minted in `identity.json`.

### Manual
- [ ] Dev mod, single-player: create a character, start a run, play two levels, quit. Continue lands at level 2's start with the gear. A run reaching a checkpoint continues from it.
- [ ] Co-op on loopback: the host opens the lobby screen, sees a guest join, and continues a run; the guest brings gear from its own single-player run, plays and leaves, and keeps what it earned. The lobby's Cancel ends the session.
- [ ] Gamepad only: create, select and delete characters and runs.

## Path
- Seams:
  - The save entry point that `coop--in-app-host-and-join` extracts from `App::exiting`.
  - `PersistedState`, `save_persisted_state`, `load_persisted_state`, `collected_per_owner_only_state` and `restore_store_after_session`.
  - Level starts: `harvest_bound_pawns` in `unload_level`, with the destination from `resolve_level_source` (`LevelLoadEntry.catalog_id`).
  - Backdrop: its pawn spawns and binds to seat 0 through the ordinary install (`spawn_from_player_starts_with_carried_loadout`).
  - Wire: `JoinSeed`, `ServerControlMessage::FinalCharacter`, and `end_host_session`, which today encodes the final character before harvesting.
  - Spawns: the seat assignment rule in `SeatTable`, the spawn-pose resolver in `crates/postretro/src/spawn_pose.rs`, and the first-player order and `coop_only` fallback in `crates/sim/src/scripting/builtins/player_spawn.rs`. Respawn shares the joiner rule; each filters to the active set.
- `CarriedState` stores names, not ids, but restore checks names unevenly (research.md §Carried loadout). Host-side checks need their own pass.
- Shape chosen: three files with numbered slots. Rival: one SQLite database holding all three, rejected for a new dependency and opaque files at kilobyte scale.
- First slice: serialize `CarriedState` into a character file, write it at a single-player level start, and Continue from it. This falsifies the riskiest premise: that harvested carry restores whole in a later process.
- `main.rs` and `state_persistence.rs` are large. Split the save code out first, behavior-preserving, in its own commit.

## Boundary inventory
| Name | Rust | Wire / serde | TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| run scope | `PersistScope::Run` | `"run"` | `persist: "run"` | `persist = "run"` | n/a |
| manifest block | `SavesManifest` | `saves` | `saves: { characterSlots, runSlots }` | `saves = { characterSlots = …, runSlots = … }` | n/a |
| engine slots | n/a | `saves.characters`, `saves.runs`, `saves.character`, `saves.run` | `getGameState().saves.*` | same path | n/a |
| save verbs | `SystemReactionCommand::<PascalName>` (`ContinueRun`, `NewRun`, `DeleteRun`, `SelectCharacter`, `CreateCharacter`, `DeleteCharacter`) | primitive string = TS name | `continueRun(i)`, `newRun(i, level)`, `deleteRun(i)`, `selectCharacter(i)`, `createCharacter(i)`, `deleteCharacter(i)` | `UI.<same name>` | n/a |
| row index | `usize`, 0-based | n/a | 0-based | 1-based | n/a |
| checkpoint | `SystemReactionCommand::Checkpoint` | `"checkpoint"` | `checkpoint(name)` | `UI.checkpoint(name)` | `checkpoint` on `player_spawn` |
| save files | n/a | `characters/<index>.json`, `world.json`, `runs/<index>.json`, `selection.json` under `<data_dir>/<mod_id>/` | n/a | n/a | n/a |

## Wire format
- `ClientControlMessage::JoinSeed` and `ServerControlMessage::FinalCharacter` each gain an optional carried loadout. A new server message carries the owner's loadout at each level start and checkpoint. A new client message asks for the final character at the client's own leave, and the client waits a bounded time for it before closing. Encoding mirrors the existing Control messages: native bitcode `Encode`/`Decode`, as every wire type derives, names as strings, never ids, and an absent loadout as `None`. The handshake bumps `WIRE_VERSION` by the rule guarded in `crates/net/src/handshake.rs`.

## Open questions
- The engine caps on character and run slot counts. — **delegated**
- Whether `savedAt` is wall-clock time, and how it reaches a string the menu shows. — **delegated**
- The bounded wait for a leaving client's final character. — **delegated**
