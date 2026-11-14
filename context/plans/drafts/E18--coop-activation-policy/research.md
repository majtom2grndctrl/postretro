# Co-op activation policy — research

Read at `6c862fd`. Facts behind the brief, cited by symbol.

## 1. History

- The 2026-07-14 plan (E18-B, "Trigger Occupancy Exposure") was never built. Commit `b2fa2189` ("docs: mark E18 co-op activation policy done") renamed the plan into `done/` and ticked the roadmap, with no code, hours after PR #261 (branch `feature/E18-B`), which shipped `E18--ir-valued-reactions`. No commit or PR adds the occupancy slots, SDK refs, fixtures or dev-tools column.
- What shipped of its substance came from `E18--trigger-event-params` (PR #271): an effective-occupant filter feeding `on.occupancy`. `E18--player-life-states` later moved the alive rule into capabilities.
- Downstream assumptions: `E18--dispatch-scope-params` shipped without its King-of-the-Hill occupancy fixture (its parity test drains at a constant `dt * -1`); `drafts/E25--map-transitions` D5/D11 and `drafts/E25--landmark-level-change` build wait-for-everyone on these refs. Records corrected on 2026-11-14: roadmap, Q3 archive, `research/co-op-triggers-trap-pools.md`, `scripting.md` §12.

## 2. Occupancy today

- `TriggerSystem.occupants: BTreeMap<EntityId, BTreeSet<PlayerId>>` (`crates/sim/src/trigger_system.rs`): physical overlap, unfiltered by arm or latch. `TriggerSystem::occupancy` returns the raw count; only dev-tools reads it.
- `TriggerStageCapabilities::capture` (`crates/sim/src/sim/player_life.rs`) builds the effective set once per tick: a declared pawn by `Capability::Occupancy`, an undeclared one unless its health is depleted (`player_is_alive_for_trigger_occupancy`). `TriggerSystem::effective_occupants(trigger, alive_players)` filters a volume's occupants through it.
- Consumers: the per-fire `@occupancy` (`trigger_commands.rs`, `TriggerEventParams.occupancy`) and trigger residuals. Nothing aggregates per tag.
- Tags: `_tags` KVP → `TriggerVolumeRecord.tags` → `TriggerVolumeBridge::populate_from_level`, read through `EntityRegistry::get_tags` / `query_by_component_and_tag`. The bridge populates on every role; only the host evaluates triggers.
- `switch` resolves to a trigger volume (`build_pipeline.md` §Custom FGD), so component queries include it.

## 3. Slots and replication

- Engine slots come from `BUILTIN_ENGINE_STATE` (`crates/entities/src/engine_state_catalog.rs`), installed by `SlotTable::default` at process start. `SlotTable` has no teardown; mod stores only grow (`plan_reconcile`). The one runtime replace is `replace_player_defense_slots` (retain by prefix, reinsert), driven by the mod vocabulary. No slot lives for one level today.
- Namespace rules: `validate_namespace_records` rejects `@` and `:`; `namespaces_overlap` rejects equal and dotted-prefix overlap. Engine catalog namespaces: `player.`, `session.`, `screen.`, `input.`, `ui.`, `accessibility.`. Nothing reserves `occupancy` or `trigger`; a sim test writes a store slot `trigger.flag`.
- Engine writes: `write_store_slot` / `apply_store_slot_batch` (`store_bridge.rs`) validate type and range but skip readonly. Script writes (`write_state_slot_json`, `write_script_store_slot`, `apply_text_edit`) warn and drop on readonly.
- `ReplicationScope::{None, SharedGlobal, OwnerPrivatePlayer}`; no engine slot is `SharedGlobal` today, but the path is generic: `ReplicatedSlotSchema::build` (`crates/netcode/src/state_slots.rs`) includes engine slots by dotted name, `HostStateReplication::ingest_frame` reads them, `ClientStateApply::apply_snapshot_state` writes them. Late join starts from baseline 0.
- Schema lifetime: built lazily on each side; reset at unload (`reset_level_scoped_host_state` via `clear_net_level_parity`; `reset_level_scoped_client_state`) and at each staged manifest commit (`reset_state_slot_schema`). A fingerprint mismatch (`validate_state_records` → `SchemaFingerprintMismatch`) drops the client's whole state batch, health included, with no refresh.
- Content parity: `level_content_digest` (`crates/postretro/src/runtime_movers.rs`, `FINGERPRINT_EPOCH` 4) hashes movers, waypoints, static collision, environment volumes, push forces and teleporters. Trigger volumes are absent, so differing tags would surface only as the fingerprint drop.

## 4. Install order

`unload_level` clears the crossing detector and accumulator bindings. Install: trigger populate (`lifecycle_world_cpu`) → data script and `populate_level_with_trigger_events` → `rebuild_reaction_subscribers(…, LevelInstall)` (crossing detector cleared and initialized against the live `SlotTable`) → `slot_accumulator_bindings.rebuild`. `CrossingDetector::initialize` warns and skips an undeclared slot, so level slots must exist before the rebuild.

## 5. Crossings and accumulators

- `onStateCrossing` threshold form takes a `ComputedRef<number>`; predicate form a Bool runtime value. `crossing_machine_source` returns `HostMirrored` when every read is a non-per-owner `SharedGlobal` slot: each machine fires over its own view, the host decides consequences (`scripting.md` §12 source-class table).
- Number slots may declare `accumulate: (t: TickParams) => delta`, rebuilt per level install; `runtime.read(ref)` reads a slot inside it.

## 6. Pins

| Pin | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Players already on the plates at install | install declares the family at 0 and writes the player counts → crossings initialize → first authoritative tick writes the counts | The crossing fires in the first authoritative frame; the set needs no exit and re-entry |
| P2 | Level change across tags | level A declares `occupancy.a.*` → unload → level B declares `occupancy.b.*` | Only B's tags have slots on every machine; a crossing over `a` in B warns and skips |
| P3 | Tag set after client install | client reveals an uncatalogued level and promotes in its admission batch → participation marker → snapshots → tag set arrives | Batches drop until the family is declared; then the schema matches, the host's baseline applies, and crossings over occupancy bind |
| P4 | Data script calls `occupancy.tag(t)` at module scope | scripts-build evaluates the script against the build-time bridge → runtime VM evaluates it at install | Both trees serve `occupancy.tag` from installers a drift test compares, as `build_time_by_player_installers_match_runtime_bridge` does for `byPlayer`; the leaf guard keeps path = slot for static entries only |
| P5 | Mod declares `occupancy` or `occupancy.x` with no level loaded, then a level installs a family | mod declaration validates (`StoreDeclarationSet::add`, `plan_reconcile`) → level install inserts `occupancy.<tag>` | Both mod spellings rejected on the mod-declaration paths; the engine family insert succeeds; `occupancyBoard` loads |
| P6 | Headless determinism run | observability batch driver ticks with no trigger stage | Determinism and puzzle rows run on a harness that drives the trigger stage |
| P7 | A counted player dies after the trigger stage's capture on tick T | capture → trigger reactions → damage → write both facts → accumulators | Both facts on T come from the one capture: the dead player still counts in both on T and leaves both on T+1; with player A outside and player B dying inside, `occupants >= inPlay` is never 1 >= 1 on T |
| P8 | Tag set for the next install arrives before the client unloads the current one | host installs B (or restarts, life L2) → one control drain: relevel, tag set naming B → client unloads A → client installs B | B's set survives A's unload and declares B's family at B's install; only a set naming another install is dropped. A same-map restart declares the family again at 0 |
| P9 | A second set names the client's current install with a different list | host reinstalls an uncatalogued level (no life) with edited tags → client keeps its install → set arrives → re-promotion → baseline | The family becomes exactly the new list: absent tags removed, new tags declared, schema and crossings rebuilt; batches apply. A merge would mismatch the fingerprint for good |
| P10 | Client joins while a set is held | client install declares the family at 0 → crossings arm at 0 → baseline applies the held counts → crossing pass | The joiner's host-mirrored crossing fires once in the baseline frame; the host fires nothing again |
| P11 | One volume lists the same tag twice | install declares the tag once → a player stands in that volume | `occupiedCount` counts the volume once and `occupants` the player once |
| P12 | Nobody in play | a lone player dies outside the room, or every co-op player is downed outside it | `inPlay` 0 and `occupants` 0; the all-players predicate stays false |
| P13 | Level change or same-map restart with a party | host installs B, spawns its player inside the room → clients demoted, loading B → each client spawns inside and re-promotes | `loading` counts the clients from install, so the gate stays closed until the last client's pawn counts inside |
| P14 | Client leaves mid-level | role change → level stops → frontend | every `session.playerCounts` entry reads 0, not the host's last replicated value |
| P15 | Client never loads, or disconnects while loading | admitted → relevel sent → never reveals; or disconnects mid-load | It stays `loading` until it disconnects; a disconnect leaves `loading` and `connected` on the next tick, not after the reconnect hold |
| P16 | Joiner before its first command | pawn spawns at promotion → first command arrives about a round trip later | With the first-command presence fix, the pawn counts `inPlay` and occupies from spawn |
| P17 | Dead client seat | an undeclared client pawn reaches zero health | It reads `down` and leaves `inPlay`; the row needs a client case, not only the host's pawn |
| P18 | Joiner after the gate opened | gate crossed → a new seat is admitted, `loading` rises → it spawns outside → enters | The predicate turns false on admission and fires again when the joiner enters |
| P19 | Flapping life state | a script toggles a pawn's state every tick | Status-only roster updates go out no faster than the rate limit; the last status lands |

## 7. Engine-state catalog

- `BUILTIN_ENGINE_STATE` (`crates/entities/src/engine_state_catalog.rs`) owns every engine slot's wire name, SDK path, type, default and capability (`scripting.md` §5). Namespaces: `accessibility`, `player`, `session`, `screen`, `input`, `options`, `window`, `loading`, `ui`. Scopes: `None` (each machine writes its own) or `OwnerPrivatePlayer` (host-authoritative, per player, to the owner). No entry is `SharedGlobal`; only mod stores (`defineStore` with `network: "shared"`) reach that scope.
- Authors read engine state one way: `getGameState()` returns a frozen tree of `{ slot, kind }` refs per catalog namespace (`ComputedRef<T>` readonly, `Ref<T>` writable), consumed by `bindState`, `stateEquals`, `onStateCrossing`, `runtime.read`. Examples: `content/dev/scripts/hud.ts` (`player.ammo`), `frontend-menu.ts` (`session.hostAddress`, `session.hosting`), `player-events.luau`.
- Typedefs and runtime trees generate from the catalog: `emit_ts_game_state_refs` and its Luau twin, `game_state_refs.rs`. Guards: `build_time_game_state_mirror.rs` (`MIRRORED_GAME_STATE_LEAVES`), `BY_PLAYER_OWNER_TOKENS`, the committed-typedef drift test.
- `session.*`: `hosting` is computed locally from role; `openSeats` arrives on a client through the roster Control message (`apply_client_session_roster`) and is absent on the host. Neither is host-computed shared state.
- Keyed slots exist only outside the catalog: `player.defense.<name>` (`replace_player_defense_slots`), owner-private, reached through `defineDefenseLayer(name).current` built on `storeRef`.
- No player-count fact exists. A mod cannot derive one: player events fire nothing when a player stops being observed, and the `occupancy` capability is not readable as a slot.

## 8. Host-sent tag set

- Control is the one ordered, reliable channel (`ServerControlFrame { participation_epoch, payload }`, resent every 100 ms); snapshots are unreliable and epoch-gated. A client accepts a snapshot only after draining the participation marker, and control drains before snapshot apply, so a control message sent before the marker lands before the epoch's first accepted snapshot. A level change always demotes, so every level starts a new epoch.
- `SpawnPoseHint` is sent at host install (`send_install_spawn_pose_hints`) and at admission, after `Relevel` and before the marker, gated on a `player_spawn`; the client holds it tagged with identity and life (`ClientSpawnPoseHint`). Exception: a client that already revealed an uncatalogued level and promotes in its admission batch gets the marker first. `Tuning` lands after the marker.
- The schema never travels; snapshots carry only `compute_fingerprint` (version, count, and per entry slot id, identity, type, enum values, wire shape, range, scope; sorted by identity). `ReplicatedSlotSchema::build` reads the local `SlotTable`. A mismatched batch drops whole, warns per batch, requests nothing; the host keeps sending `FullBaseline` for unacked slots, so the client recovers once its schema matches.
- Client binding: crossings run on clients; `CrossingDetector::initialize` skips an undeclared threshold slot and a predicate fails to bind. Rebinds happen only at level install and staged manifest commit (`rebuild_active_reaction_subscribers`, which also calls `reset_state_slot_schema`). `bindState` looks slots up each frame and only warns once at tree build (`warn_unknown_slot_binds`). Accumulators evaluate on the host only.
- Nothing today declares slots from host data; `replace_player_defense_slots` runs from each peer's own mod commit.

## 9. Players: status, roster and UI

- Status sources on the host (`SeatTable`, `crates/netcode/src/seat.rs`): a seat with a `client_bindings` entry and no `pawn_bindings` entry is loading; a bound pawn is in play or down by the life-state rule; a seat in `hold_deadlines` (`HOLD_WINDOW` 30 s) is reconnecting. Transport "holding" causes mean level load or parity, not the reconnect hold; a client that will never load looks the same as one loading.
- `SessionRosterMessage { session_id, your_seat, open_seats, entries: [RosterEntry { seat: u16, connected: bool }] }` goes reliable-ordered on Control to every Admitted or Participating peer (`publish_dirty_roster`), coalesced to one publish per poll. Seat 0 (the host) is always listed and connected. Nothing publishes in single player; the host never projects the roster to itself. A client keeps the whole message in `ClientSessionStatus` and writes only `session.openSeats`.
- Names: `ConnectClaim { player_id, display_name }` rides the connect token; the host keeps claims for remote seats only. `DISPLAY_NAME_MAX_BYTES` 200 is the only limit. Every name is empty today (`Session::build` sends `String::new()`). The host-local contract lives in `networking.md` §Session-state ledger, `scripting.md` §5 and `player_options.md`; `roster_entry_fields_stay_claim_free` destructures `RosterEntry` exhaustively. `RosterEntry` encodes positionally, so a new field bumps `WIRE_VERSION` (39).
- "Down" already reaches clients per pawn in the movement record's life-state id; the roster status is a second, display-only path.
- UI: trees are static descriptors, rebuilt only when the descriptor changes (a rebuild snaps tweens); no list or repeat widget exists; `visibleWhen` hides a node without removing it, and focus skips hidden nodes. `Switch` in `sdk/lib/ui/state.ts` unrolls cases at authoring time. Presentation overlays instance a template per entity but are passive, world-anchored and outside tree layout. `repeat` is an existing container prop (hold-to-repeat). Catalog value types include `String` and `Enum`.
- Seat numbers increase monotonically up to a u16 namespace, so list rows are dense indices, not seats. `MAX_CLIENTS` is 8.
