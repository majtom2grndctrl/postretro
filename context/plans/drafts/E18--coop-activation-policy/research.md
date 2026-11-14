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
| P1 | Players already on the plates at install | install declares slots at 0 → crossings initialize at 0 → first authoritative tick writes the count | The crossing fires on the first tick; the set needs no exit and re-entry |
| P2 | Level change across tags | level A declares `occupancy.a.*` → unload → level B declares `occupancy.b.*` | Only B's tags have slots; a crossing over `a` in B warns and skips |

## 7. Engine-state catalog

- `BUILTIN_ENGINE_STATE` (`crates/entities/src/engine_state_catalog.rs`) owns every engine slot's wire name, SDK path, type, default and capability (`scripting.md` §5). Namespaces: `accessibility`, `player`, `session`, `screen`, `input`, `options`, `window`, `loading`, `ui`. Scopes: `None` (each machine writes its own) or `OwnerPrivatePlayer` (host-authoritative, per player, to the owner). No entry is `SharedGlobal`; only mod stores (`defineStore` with `network: "shared"`) reach that scope.
- Authors read engine state one way: `getGameState()` returns a frozen tree of `{ slot, kind }` refs per catalog namespace (`ComputedRef<T>` readonly, `Ref<T>` writable), consumed by `bindState`, `stateEquals`, `onStateCrossing`, `runtime.read`. Examples: `content/dev/scripts/hud.ts` (`player.ammo`), `frontend-menu.ts` (`session.hostAddress`, `session.hosting`), `player-events.luau`.
- Typedefs and runtime trees generate from the catalog: `emit_ts_game_state_refs` and its Luau twin, `game_state_refs.rs`. Guards: `build_time_game_state_mirror.rs` (`MIRRORED_GAME_STATE_LEAVES`), `BY_PLAYER_OWNER_TOKENS`, the committed-typedef drift test.
- `session.*`: `hosting` is computed locally from role; `openSeats` arrives on a client through the roster Control message (`apply_client_session_roster`) and is absent on the host. Neither is host-computed shared state.
- Keyed slots exist only outside the catalog: `player.defense.<name>` (`replace_player_defense_slots`), owner-private, reached through `defineDefenseLayer(name).current` built on `storeRef`.
- No player-count fact exists. A mod cannot derive one: player events fire nothing when a player stops being observed, and the `occupancy` capability is not readable as a slot.
