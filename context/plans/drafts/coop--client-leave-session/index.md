# coop--client-leave-session

Brief · compact · reads: `context/lib/networking.md` §Role model, §Slot lifecycle, §Admission and content parity · `context/lib/scripting.md` §10.4 · `context/lib/boot_sequence.md` §1 · read at 7eab3e3

## Problem
The owner raised this while gating client level verbs. Basis: a defect read in source, not yet reported from play. On a connected client, `returnToFrontend()` and the `ui.quitToMenu` button unload the level locally and keep the connection. The cause is that `App::return_to_frontend` has no role branch, no dispatch says whether a player or the shared world fired it, and nothing can end a client's connection or its client role mid-run. The host demotes the slot and despawns the pawn but holds the seat. The client still follows the host, so the host's next relevel pulls it off the menu and back into play. From the menu, the client's own `loadLevel` is now dropped (host-only), so "New game" does nothing. A client whose host vanishes is left in the same dead state until relaunch. When this is done, a client player's own quit leaves the session: the host sees a clean disconnect, and the client stands at the frontend as a single-player process whose menu works. Losing the host lands there too. A world event such as a level-complete crossing never disconnects a client.

## Decisions
- **A typed origin says whom a reaction speaks for.** `FireOrigin` gains `LocalPlayer`, set where a UI press or commit dispatches (`app/ui_actions.rs`). The origin carries through named follow-ups, as origin already does. `EveryMachine` keeps meaning the shared world (levelLoad, crossings, movement and weapon events, observer cues), and `HostOnly` is unchanged. A new dispatch site has to choose an origin. It cannot pick one up from a source string.
- **Game-flow verbs follow origin and role.**

  | Origin, on a connected client | `loadLevel` / `restartLevel` | `returnToFrontend` |
  |---|---|---|
  | `EveryMachine` | dropped, warned once per source | dropped, warned once per source |
  | `LocalPlayer` | dropped, warned once per source | **leave** |

  - `ui.quitToMenu` and `ui.exitToDesktop` are `LocalPlayer` by construction and always leave.
  - Host and single-player run every row as today.
  - Map authority is the host's (`networking.md` §Slot lifecycle), so the world-origin rows drop: the host's copy of the same event is the authoritative one. A level-complete crossing therefore moves the host, and clients follow its demotion. It never disconnects them.
- **Leave.** Leave closes the connection cleanly, then unloads and presents the frontend as single-player does. The host closes the slot as a disconnect, not a timeout, and the existing seat hold applies (`netcode` `SeatTable::hold_disconnected_client`).
- **A lost host leaves too.** When the transport reports that an established connection has ended, the client takes the same leave path, minus the disconnect send. This narrows `networking.md` §Role model, which says "nothing above the transport reacts to that ending". A connect that never succeeded keeps its bounded retries.
- **The role moves one way, once.** This narrows `networking.md` §Role model ("selected once at startup"). A client may become single-player. It never becomes a host or connects again in the same process. Rejoining means relaunching with `--connect`; no in-app join UI exists to reconnect from.
- **One derivation of role-bound state.** Every flag that `Session::build` derives from the endpoint is derived in one place, and leave reuses it, so the two cannot drift. Those flags are runtime-spawn authority, auto-close timers, the reaction scheduler latch, owner-slot writes and the seat table. A client that has left gets the same state a session built without `--connect` gets, including its local-only seat ledger. This is the precedent `authority_seat_table_or_local_only` already uses at boot when it demotes a failed host to single-player.
- **Exit to desktop also disconnects cleanly.** `ui.exitToDesktop` on a client uses the same disconnect before the event loop exits, so the host holds the seat at once rather than after the keepalive timeout. No single-player teardown is needed there.
- **Clean disconnect is a transport method.** `NetClient` gains one disconnect method that sends netcode's disconnect packets. Nothing above the transport opens or reads a socket (`networking.md` §Transport contract). No wire type changes, so `WIRE_VERSION` and the app protocol id stand.
- **Non-goals.**
  - The host quitting to the menu. The host's unload already demotes every client, and ending a session for all players is a separate product decision.
  - Post-`wait` `returnToFrontend`. `drafts/reaction-body-composition` owns it and drops it on a client. That stands until the draft revisits it with leave semantics in view.
  - An in-app join, reconnect, or "connection lost" dialog.

## Acceptance
### Automated
- [ ] A connected client running a `LocalPlayer`-origin `returnToFrontend` ends with no endpoint and a frontend unload queued. A later `loadLevel` on it queues a load and logs no client-drop warning.
- [ ] `ui.quitToMenu` on a connected client leaves, the same as a `LocalPlayer` `returnToFrontend`.
- [ ] An `EveryMachine`-origin `returnToFrontend` (levelLoad, a crossing) on a connected client queues nothing, keeps the endpoint, and warns once per source across repeated fires.
- [ ] A named follow-up chained from a UI press keeps `LocalPlayer` origin. One chained from levelLoad keeps `EveryMachine`.
- [ ] A client whose established connection ends unloads to the frontend as single-player, without a relaunch. A client whose first connect is still retrying does not.
- [ ] After leave, every role-bound flag equals the value a session built with no `--connect` holds. One test compares the two field by field.
- [ ] Loopback host and client: the client leaves. The host closes that slot with the disconnect cause, not timeout, within one second of host time, and holds the seat. The seat is reclaimable by the same player claim within the hold window.
- [ ] A relevel the host sends after the client has left loads nothing on the client.
- [ ] Single-player and host `returnToFrontend` queue exactly what they queue today: unload, then the backdrop load when one is declared.
- [ ] `ui.exitToDesktop` on a connected client sends the disconnect before exit, and the host's close cause is disconnect.
### Manual
- [ ] Two-process loopback (`networking.md` §Manual loopback recipe): the client quits to the menu, stays at the frontend through a host level change, and starts a local game from the menu. The host's player list drops the client at once.

## Path
- Seams: `App::return_to_frontend` (main.rs), `UiButtonAction::QuitToMenu` in `app/ui_actions.rs`, the role-flag block in `Session::build` (`session/mod.rs`) that calls `set_runtime_spawn_authority`, `auto_close_timers.set_enabled`, `scheduler.set_enabled` and `owner_slot_writes_enabled`, then `NetClient` in `crates/net/src/transport.rs`, and the host's `close_cause_from`.
- Leave runs inside `return_to_frontend`, before it queues `LevelRequest::Unload`. The unload then runs as single-player: `clear_net_level_parity` finds no endpoint, so no `Revealed(None)` or parity re-declaration goes to a host that is already closing.
- `App::is_connected_client` reads the endpoint, so dropping the endpoint flips every per-frame client branch. Grep for `NetEndpoint::Client` matches that cache the role elsewhere, such as scheduler or seat state.
- Origin seam: `SystemCommandFireContext.origin`, filled by `fire_named_event_with_sequences` from its dispatch context. The UI sites pass `None` today, so their source reads `named:<name>`, the same shape levelLoad has. `ReturnToFrontend` gains the `dispatch_source` field that `LoadLevel` already carries, plus the origin. The once-per-source latch is the one the level-verb gate uses (`ScriptingCore::client_level_verb_warnings`).
- Lost-host seam: `NetClient::update` already reports an established connection ending, once ("lost connection to host").
- Rival direction: tag UI sources with a string prefix rather than a typed origin. It is cheaper now, but every later origin-sensitive rule becomes a string match a new source can fall through.
- Rival shape: rebuild the `Session` without an endpoint. It is heavier, because the session owns the VM, audio and UI, and it buys nothing the shared derivation does not.
- First slice: the transport disconnect plus a loopback test asserting the host's close cause. This falsifies the riskiest assumption, that renet_netcode delivers the disconnect before the endpoint drops.

## Open questions
- Should leave also show the player a "left session" or "disconnected" notice? — **delegated**: the executor decides and reports it in the plan of record. The default is a log line only.
