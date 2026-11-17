# coop--character-and-run-saves — research

Read at acd5eae8. Facts by symbol; the executor re-verifies.

## Persistence today
- `PersistedState` (`crates/sim/src/scripting/state_persistence.rs`): `version` (`CURRENT_STATE_VERSION` 4, `OLDEST_SUPPORTED_STATE_VERSION` 2, `persisted_state_version_is_supported`), `slots` (globals by durable key), `per_owner` (durable key → 32-hex player id → value), `faction_sentiment` (from → to → f32, with reserved `@postretro.*` keys). `PersistedValue` is untagged.
- An unsupported version or a parse error ignores the file, `session.persisted_state` stays `None`, `mark_restore_completed` still runs, and the next clean-exit save overwrites the file. The brief tightens this for slot files: an unreadable file is never overwritten silently.
- `save_persisted_state` writes `state.json.tmp` and then renames it.
- Reads: `load_persisted_state` (`startup/splash_lifecycle.rs`), once per process, gated by `should_restore_after_mod_init`. A connected client overlays globals only, then its own rows once seated (`overlay_client_local_per_owner_state`).
- Writes:
  - `App::exiting`: `leave_or_end_for_exit` → `collect_persisted_state` → `collect_persisted_faction_sentiment` → `collect_per_owner_state` on Seat(0) → `merge_per_owner_state` → `save_persisted_state`. It keeps only the local id's rows and only declared keys.
  - Client periodic: `maybe_save_connected_client_per_owner_state` every `PER_OWNER_SAVE_INTERVAL` (60 s).
  - Client leave: `save_character_on_leave`.
  - Client writes go through `collected_per_owner_only_state`, which keeps the boot world sections verbatim.
- Save gate: `StateStoreLifecycle::can_save`.
- `PlayerOptions::player_id` (`options/boot.rs`) is one per data directory, not one per mod. The client sends an empty display name today (`session/mod.rs`); `ConnectClaim.display_name` is truncated at `DISPLAY_NAME_MAX_BYTES`.
- Durable keys: `StoreIdentityLedger` reads `identity.json`. A saved key that is no longer declared is skipped on load and dropped by the next clean-exit save.

## Carried loadout
- `CarriedState` (`crates/combat-model/src/carried_loadout.rs`) has no serde. Fields:
  - `health_current`
  - `life_state` (a name)
  - `reserve: AmmoReserve` (ammo name → u32)
  - `wieldables` (canonical descriptor names)
  - positional `magazines`
  - `active_slot`
  - `layers` (layer name → value)
- `carried_fields` classifies each `CarriedField` exhaustively. `compose_wieldable_inventory_slots` skips an unknown weapon with a warning. `restore_carried_reserve` restores every ammo balance unchecked, and `restore_carried_layers` skips unknown layers silently. Ammo reserves have no engine cap: `AmmoReserve` is a free string map saturating at `u32`.
- `restore_carried_health` ignores health ≤ 0, so a player dead at the boundary arrives at full health and full layers, but with inventory intact. Not checked: whether death strips inventory first.
- Release: only `SeatTable::release_seat` (hold expiry, and `release_remote_seats` on `endSession`). `loadLevel`, `restartLevel` and `returnToFrontend` all keep seat 0's carry, so no single-player fresh start exists.
- The frontend backdrop spawns a seat-0 pawn and binds it to seat 0 whenever its map has a `player_spawn` (the dev mod's `combat-demo` does), and unloading the backdrop harvests it like any level. The brief forbids that harvest.

## Level boundary order
`App::drain_level_requests` runs in this order:
1. `resolve_level_source`. The destination's `LevelLoadEntry.catalog_id` is known here.
2. `cancel_beside_level_load`.
3. `unload_level`: `harvest_bound_pawns` runs before `registry.clear_for_level_unload`, then `clear_pawn_bindings_for_level_unload` clears `life_state`.
4. `begin_level_load`.
5. Install: Seat(0) restores through `spawn_from_player_starts_with_carried_loadout`; remote seats through `spawn_net_slot_pawn_with_carried_loadout`.
6. Settling, then Running.

`restartLevel` requeues `active_level_source`, which may be a `LevelSource::Path` with no catalog id.

## Wire
- `ClientControlMessage::JoinSeed { slots }` carries per-owner `persist` slots only. `apply_join_seed` rejects out-of-range values rather than clamping them.
- `ServerControlMessage::FinalCharacter { slots }` carries slots only.
- The append rule and the `WIRE_VERSION` bump are guarded in `crates/net/src/handshake.rs`.

## UI and manifest
- Dev frontend: `content/dev/scripts/frontend-menu.ts`. PLAY opens `frontend.devLevelSelect`, whose buttons fire the named reactions `frontend.start.<id>`, each one `loadLevel(id)`. `Button.onPress` takes a reaction name. Text binds state through `bindState`. `forEach` rows are 0-based and static (`ui.md` §1).
- Engine readonly slots are `EngineStateCatalogEntry` rows in `crates/entities/src/engine_state_catalog.rs`, and the SDK `getGameState()` types are generated from them.
- The manifest is the `defineMod` default export (`content/dev/start-script.ts`). Its `maps` come from `defineMapCatalog`, and `ModMapEntry.id` is documented as stable.

## Commitments touched
- `player_options.md` §1: options and game saves are separate stores. It names SQLite as a candidate; this brief replaces that with JSON.
- `E16--per-player-persistence`: each player saves only its own per-owner values, and a client never saves globals.
- `coop--client-leave-session`: saves stay the player's own.
- `E15--seat-session-identity-roster` I7: carried state is an enumerable set.
- `E18--coop-activation-policy`: roster names come from the character save.
- `entity_model.md`: magazines carry; heat and cells do not.
- Roadmap: the session-scoped co-op lean says nothing persists server-side. A host's run slot is the host's own single-player-like progress, not a server's.

## Pins
| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Seat-0 carry outside play | Boot restore of the last selection, `selectCharacter`, `createCharacter`, Continue, `newRun` and a client's leave (after its final loadout is saved) each set seat 0's carry from the selected character before any later save, join seed or install reads it | No save point writes an empty or stale carry over a saved character |
| P2 | Backdrop unload vs selection | The backdrop's pawn is never harvested; the verbs set seat 0's carry from the character | The level and its first save see the selected character's loadout, never the backdrop pawn's |
| P3 | End-session harvest | Each participating guest's pawn is harvested before its final character is encoded | The final loadout holds gear picked up after the last level start |
| P4 | Level-start save placement | The save runs after `unload_level` returns, after the life-state clear, and before `begin_level_load` | A character saved downed continues in its initial life state |
| P5 | Level-start write order | Run file before character file | A crash between the two resumes the destination with the earlier gear: it may lose a level's gear and never duplicates it |
| P6 | Verbs after a queued run load | Once Continue or `newRun` queues its level, every later save verb is refused until the frontend is entered again | The first verb takes effect; the second is refused and changes nothing |
| P7 | `newRun` write | `newRun` writes its run file before its level load begins, whether or not a level was installed | A crash in the new run's first level resumes the new run |
| P8 | Client character writes | Every client character write takes the loadout the host last sent, else the joined one, never the boot file's | A periodic save after a level-start message keeps that message's loadout |
| P9 | Migration completion | The file whose presence ends migration is written last | A crash after an earlier migration write migrates again at next boot |
| P10 | Deleting the selection | A delete on the selected index sets that selection to `-1` and stores it | A restart after deleting the selected run shows run `-1` |
| P11 | Continue arriving at a checkpoint | Continue's level load writes the run and character but keeps the run's checkpoint; only a later level start or checkpoint replaces it | A second Continue with no save point between arrives at the same checkpoint |
| P12 | Deleting the selected character | The delete empties seat 0's carry before any later install reads it | A later `loadLevel` with no selection spawns empty and writes no character file |
