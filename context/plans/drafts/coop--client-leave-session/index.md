# coop--client-leave-session

Brief · compact · Epic 15 · reads: `context/lib/networking.md` §Role model, §Slot lifecycle, §Admission and content parity · `context/lib/scripting.md` §10.4, §12 (Steps after a `wait`) · `context/lib/boot_sequence.md` §1 · read at 3fba30ae

## Problem
The owner raised this while gating client level verbs. Basis: a defect read in source, not yet reported from play. On a connected client, `returnToFrontend()` and the `ui.quitToMenu` button unload the level locally and keep the connection, before a `wait` or after one (`done/reaction-body-composition` lands the step on a client). The cause: `App::return_to_frontend` has no role branch, no origin separates a player's own input from game logic that fires for that player, and nothing can end a client's connection or its client role mid-run. The host demotes the slot and despawns the pawn but holds the seat. The client still follows the host, so the host's next relevel pulls it off the menu and back into play. Its own `loadLevel` is dropped (host-only), so "New game" on its menu does nothing. A client whose host vanishes, or whose host refuses it at admission, is left in the same dead state until relaunch. When this is done, a client player's own quit leaves the session: the host sees a clean disconnect, and the client stands at the frontend as a single-player process whose menu works. Any host-side ending lands there too: a lost host, a refusal, and later a kick. Game logic never disconnects a client.

## Decisions
- **Player input is its own machine source.** UI presses and text commits (`app/ui_actions.rs`) leave `MachineSource::OwnState` for a new `MachineSource::LocalInput`: player input on this machine, nothing else. It routes presentation, captures, and lands `wait` tails exactly as `OwnState` does (`scripting.md` §12). Only game-flow verbs read the difference. Named follow-ups and `wait` tails keep it, so a UI press that waits and then returns to the frontend leaves when the step lands.
- **Game-flow verbs follow origin and role.**

  | Origin, on a connected client | `loadLevel` / `restartLevel` | `returnToFrontend` |
  |---|---|---|
  | `LocalInput` | dropped, warned once per source | **leave** |
  | any other | dropped, warned once per source | dropped, warned once per source |

  - `ui.quitToMenu` and `ui.exitToDesktop` are player input by construction and always leave.
  - Host and single-player run every row as today.
  - Map authority is the host's (`networking.md` §Slot lifecycle), so game logic never moves or ejects a client. A level-complete crossing moves the host, and clients follow its demotion. An own-state crossing or movement event does not end a client's session; ending one for game reasons is the host's call.
  - This narrows `done/reaction-body-composition`, whose client landing of a world or own-state `returnToFrontend` unloaded locally. Now it drops.
- **Leave.** Leave closes the connection cleanly, then unloads and presents the frontend as single-player does. The host closes the slot as a disconnect, not a timeout. The existing seat hold applies (`netcode` `SeatTable::hold_disconnected_client`).
- **Any host-side ending leaves too.** When an established connection ends, the client takes the same leave path, minus the disconnect send. That covers a lost host, a `Closing` divergence such as a mod-id refusal, and any later host-initiated close. This narrows `networking.md` §Role model ("Nothing above the transport reacts to that ending") and `scripting.md` §10.4 ("A client whose host is gone stays a client until relaunch"). A connect that never succeeded keeps its bounded retries.
- **The role moves one way, once.** This narrows `networking.md` §Role model ("selected once at startup"). A client may become single-player. It never becomes a host or connects again in the same process. Rejoining means relaunching with `--connect`; no in-app join UI exists to reconnect from.
- **One derivation of role-bound state.** Every value `Session::build` derives from the endpoint is derived in one place, and leave reuses it, so the two cannot drift. A client that has left holds what a session built without `--connect` holds, including its local-only seat ledger. Precedent: `authority_seat_table_or_local_only` already demotes a failed host to single-player at boot.
- **Exit to desktop also disconnects cleanly.** `ui.exitToDesktop` on a client sends the same disconnect before the event loop exits, so the host holds the seat at once rather than after the keepalive timeout. No single-player teardown is needed there.
- **Clean disconnect is a transport method.** `NetClient` gains one disconnect method that sends netcode's disconnect packets, and it surfaces why a connection ended. Nothing above the transport opens or reads a socket (`networking.md` §Transport contract). No wire type changes, so `WIRE_VERSION` and the app protocol id stand.
- **Non-goals.**
  - The host quitting to the menu. The host's unload already demotes every client, and ending a session for all players is a separate product decision.
  - Kicking a client. A kick is a host-executed slot close, because the host owns admission and a held or content-diverged client receives no state a script could act on. This brief builds its client half: any host close lands at a working frontend. The host verb and its policy (who may kick, seat hold, rejoin) belong to Epic 15's lobby authoring surface, where the mod owns lobby policy.
  - A `@player` audience for player-input fires. The default audience already plays on this machine for every machine source, and UI sources publish no `on.player` token. `@player` there would be a second spelling of the default.
  - An in-app join, reconnect, or "connection lost" dialog.

## Acceptance
### Automated
- [ ] A connected client running a `LocalInput`-origin `returnToFrontend` ends with no endpoint and a frontend unload queued. A later `loadLevel` on it queues a load and logs no client-drop warning.
- [ ] `ui.quitToMenu` on a connected client leaves, the same as a `LocalInput` `returnToFrontend`.
- [ ] On a connected client, `returnToFrontend` from levelLoad, from a shared-slot crossing, from an own-slot crossing and from a movement event each queues nothing, keeps the endpoint, and warns once per source across repeated fires.
- [ ] A named follow-up chained from a UI press keeps `LocalInput`. One chained from levelLoad keeps `HostTimeline`.
- [ ] A UI press whose reaction waits 300 ms and then returns to the frontend: on a connected client, the client leaves when the step lands and the host stays in the level with the slot closed as a disconnect. The same tail fired from levelLoad drops on the client and warns once.
- [ ] A `LocalInput` fire and an `OwnState` fire of the same reaction route presentation, capture and `wait` landing identically.
- [ ] A client whose established connection ends unloads to the frontend as single-player, without a relaunch. A client whose first connect is still retrying does not.
- [ ] A client the host refuses at admission for a mod-id mismatch ends at the frontend as single-player, and its menu starts a local game.
- [ ] After leave, every role-bound value equals the value a session built with no `--connect` holds. One test compares the two field by field.
- [ ] Loopback host and client: the client leaves. Within one second of host time, the host closes that slot with the disconnect cause, not timeout, and holds the seat. The same player claim can reclaim the seat within the hold window.
- [ ] A relevel the host sends after the client has left loads nothing on the client.
- [ ] Single-player and host `returnToFrontend` queue exactly what they queue today: unload, then the backdrop load when one is declared.
- [ ] `ui.exitToDesktop` on a connected client sends the disconnect before exit, and the host's close cause is disconnect.
### Manual
- [ ] Two-process loopback (`networking.md` §Manual loopback recipe): the client quits to the menu, stays at the frontend through a host level change, and starts a local game from the menu. The host's player list drops the client at once.

## Path
- Origin seams: `ui_dispatch_context` (`app/ui_actions.rs`) sets `EveryMachine(OwnState)` for presses and `commit_text_entry`. `UiButtonAction::QuitToMenu` and `ExitToDesktop` bypass the reaction queue (`return_to_frontend`, `pending_exit_to_desktop`). The scheduler collapses origin to `TailClass` (`reaction_scheduler`), which has no input class, so the tail has to keep the source for the landing's fire context to name it. Every exhaustive `MachineSource` match needs an arm.
- Verb seam: the `returnToFrontend` handler (`sim` `scripting_systems/system_reactions`) captures no fire context; the `loadLevel` handler beside it reads `queue.fire_context()`. `ReturnToFrontend` gains the origin and `dispatch_source`. `refuse_client_level_verb` (main.rs) gates on role alone; its once-per-source latch is `client_level_verb_warnings`. `system_reaction_is_machine_local` can keep calling client `returnToFrontend` machine-local: the client tail lands the step and the drain decides.
- Leave runs inside `return_to_frontend`, before it queues `LevelRequest::Unload`. The unload then runs as single-player: `clear_net_level_parity` and `reset_level_scoped_client_state` find no endpoint, so nothing goes to a host that is already closing. Check that `active_level_host_named` and the endpoint's `ClientSpawnPoseHint` do not outlive the endpoint.
- Role state: the endpoint lives in `Session.net_endpoint`; `App::is_connected_client` reads it, and most per-frame client branches re-derive from it. Values cached at build sit in the role block of `Session::build`: the seat table (`None` on a client), `set_runtime_spawn_authority`, `auto_close_timers.set_enabled`, `scheduler.set_role`, `owner_slot_writes_enabled`, and the `published_host_address` slots.
- Host-ending seams: `NetClient::update` logs an established connection's end once (`failure_reported`, `ever_connected`) and keeps netcode's `disconnect_reason()` internal. `client_drain_control` logs a `Closing` divergence. A world-less client's `WorldLessPoll::Failed` is acted on only for a host today. Leave reads the close reason, not only that edge, so a future host-migration path can branch ahead of leave on `CloseCause::HostInitiatedLeave` (`crates/net/src/slots.rs`).
- Host close cause: `close_cause_from` maps `DisconnectedByClient` to `Disconnect`. `ServerPoll.disconnects` drops the cause, and the seat hold runs for both causes; the cause survives in `SlotState::Closed`.
- `follow_relevel_catalog` is what pulls a quit client back into play today; with no endpoint it never runs.
- Rival direction: tag UI sources with a string prefix rather than a typed source. It is cheaper now, but every later origin-sensitive rule becomes a string match a new source can fall through.
- Rival scope: every `OwnState` origin leaves. It lets a mod eject a client through an owner-private slot crossing, but only cooperatively, and a held client never receives the slot. A host-executed close reaches every client, so kick takes that shape.
- Rival shape: rebuild the `Session` without an endpoint. It is heavier, because the session owns the VM, audio and UI, and it buys nothing the shared derivation does not.
- First slice: the transport disconnect plus a loopback test asserting the host's close cause. This falsifies the riskiest assumption, that renet_netcode delivers the disconnect before the endpoint drops.

## Open questions
- Should leave also show the player a "left session" or "disconnected" notice? — **delegated**: the executor decides and reports it in the plan of record. The default is a log line only. Whatever notice ships should also cover a host-named load that fails on the client, which `done/coop--client-spawn-view-handoff` returns to the frontend menu with no message.
