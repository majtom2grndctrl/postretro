# coop--client-leave-session

Brief · compact · Epic 15 · reads: `context/lib/networking.md` §Role model, §Slot lifecycle, §Admission and content parity, §Version gates · `context/lib/scripting.md` §5 (Durable State Store), §10.4, §12 (Steps after a `wait`) · `context/lib/boot_sequence.md` §1 · `context/research/coop-session-lifecycle.md` · read at 514b5944

## Problem
The owner raised this while gating client level verbs. Basis: defects read in source, not yet reported from play. A connected player has no way out of a session, and nothing catches a client when its session ends under it. On a client, `returnToFrontend()` and `ui.quitToMenu` unload the level locally and keep the connection, before a `wait` or after one. The host's next relevel then pulls the client back into play, and its menu's level buttons do nothing. When the host quits to its menu, clients receive only a hold, so they sit in a stale level with no pawn. A client whose host vanishes, refuses it at admission or exits is left disconnected but still a client, until it relaunches. The cause: one verb, `returnToFrontend`, both navigates and decides membership, and no machine can end its own role mid-run. When this is done, a client's quit leaves the session cleanly and a host's quit ends it for everyone. Every client whose session ends, for any reason, stands at a working single-player frontend, and its saves hold only its own character and world.

## Decisions
- **Membership has its own verbs.** `leaveSession()` and `endSession()` are system reactions. Navigation never changes membership, and the frontend is outside every session (`research/coop-session-lifecycle.md` §5).

  | Role | `leaveSession` | `endSession` | `returnToFrontend` | `ui.quitToMenu` |
  |---|---|---|---|---|
  | Single-player | warn | warn | as today | as today |
  | Host | warn | **end** | refused, warned | `endSession` |
  | Connected client | **leave** | dropped, warned | refused, warned | `leaveSession` |

  - Each warning fires once per source and verb. A refused `returnToFrontend` names the membership verbs.
  - Only role decides, never origin: a `wait` tail, a crossing and a UI press all behave alike. This narrows `done/reaction-body-composition`, whose client landing of a post-`wait` `returnToFrontend` unloaded locally. It now refuses.
- **Leave.** The client closes its connection cleanly, then unloads and presents the frontend as single-player does. The host closes the slot as a disconnect, not a timeout, and the existing seat hold applies (`netcode` `SeatTable::hold_disconnected_client`).
- **End.** The host closes every slot with a session-ended cause, then drops its endpoint and presents the frontend as single-player does. Closing reuses the admission-refusal path, which sends the cause and disconnects once it is acknowledged (`NetServer::reject`), bounded so a silent client cannot hold the host. Ending holds no seats: the session is gone.
- **Any host-side ending leaves.** When an established connection ends, the client takes the leave path, minus the disconnect send. That covers a session-ended cause, a mod-id refusal, a lost host, and any later host close (eviction, password). This narrows `networking.md` §Role model ("Nothing above the transport reacts to that ending") and `scripting.md` §10.4 ("stays a client until relaunch"). A connect that never succeeded keeps its bounded retries.
- **Exit to desktop takes the same path.** A client leaves and a host ends before the event loop exits, so peers learn at once rather than after the keepalive timeout.
- **A role moves one way, to single-player.** This narrows `networking.md` §Role model ("selected once at startup"). Hosting or joining again needs a relaunch until in-app hosting and joining land. Those briefs reverse the same derivation.
- **One derivation of role-bound state.** Every value `Session::build` derives from the endpoint is derived in one place, and leave and end reuse it, so they cannot drift. A machine that has left or ended holds what a session built without `--host` or `--connect` holds, including its local-only seat ledger. Precedent: `authority_seat_table_or_local_only` already demotes a failed host to single-player at boot.
- **Saves stay the player's own.** Before a client drops its endpoint, it saves its own character rows, as its periodic save does. It then reloads its store from its own save, so the result is what a fresh boot from that save would hold. The host's replicated world values never reach the ex-client's saves or its single-player world. Character and world progress stay separate (`research/coop-session-lifecycle.md` §7).
- **The wire gains one closing cause.** `ClosingCause` gains `SessionEnded`, so a client can tell an ended session from a lost host. Appending a variant moves `WIRE_VERSION` (`networking.md` §Version gates). The clean disconnect is a `NetClient` method that also reports why a connection ended. Nothing above the transport touches a socket.
- **Non-goals.**
  - Navigation and the optional lobby: the generalized `navigate(destination)` that replaces `returnToFrontend`, and a host taking clients to a lobby. Brief 2 in the lifecycle map. Until then, a co-op run ends at the frontend through `endSession`.
  - `evictPlayer` and the session password. Each lands a client through this brief's host-side ending path.
  - In-app hosting and joining, and a player-facing notice dialog (see Open questions).
  - Separate character and world save files, level-boundary saves and run saves. Brief 4 in the lifecycle map. This brief keeps today's single `state.json`.

### Scripting surface
TypeScript in `sdk/lib/ui/reactions.ts`, beside `returnToFrontend()`, exported from the prelude. The Luau mirror in `reactions.luau` uses the same names.

```typescript
import { defineReaction, leaveSession, endSession } from "postretro";

// A client's "Leave game" button.
export const leave = defineReaction("pause.leaveSession", leaveSession());
// A host's "End session" button.
export const end = defineReaction("pause.endSession", endSession());
```

Both take no arguments and return a `SystemReactionStep` (`{ primitive: "leaveSession", args: {} }`, `{ primitive: "endSession", args: {} }`). Both are host consequences in the system-reaction classification, never machine-local.

### Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| leave | `SystemReactionKind::LeaveSession` | `"leaveSession"` | `leaveSession()` | `UI.leaveSession()` |
| end | `SystemReactionKind::EndSession` | `"endSession"` | `endSession()` | `UI.endSession()` |
| session-ended cause | `ClosingCause::SessionEnded` (appended, no fields) | bincode variant index after `ModId` | n/a | n/a |

## Acceptance
### Automated
- [ ] A connected client running `leaveSession` ends with no endpoint, the frontend presented and an unload queued. A later `loadLevel` queues a load and logs no client-drop warning.
- [ ] `ui.quitToMenu` leaves on a connected client, ends the session on a host, and in single-player queues exactly what it queues today.
- [ ] `returnToFrontend` on a connected client and on a host queues nothing, keeps the endpoint, and warns once per source naming the membership verbs, across repeated fires from levelLoad, a crossing and a UI press. In single-player it queues unload, then the backdrop load when one is declared.
- [ ] A UI press whose reaction waits 300 ms and then runs `returnToFrontend` refuses on a connected client when the step lands. The same press with `leaveSession` leaves when it lands.
- [ ] `leaveSession` on a host or in single-player, and `endSession` on a client or in single-player, change nothing and warn once per source.
- [ ] Loopback host and two clients, host runs `endSession`: each client receives the session-ended cause and ends with no endpoint at a frontend whose menu starts a local game. The host ends with no endpoint, and its next `loadLevel` publishes no parity and sends no relevel.
- [ ] Loopback host whose client never acknowledges: `endSession` still finishes within its bound.
- [ ] A client whose established connection ends unloads to the frontend as single-player, without a relaunch. A client whose first connect is still retrying does not.
- [ ] A client refused at admission for a mod-id mismatch ends at the frontend as single-player, and its menu starts a local game.
- [ ] After a client leaves, and after a host ends, every role-bound value equals the value a session built with no network flag holds. One test compares them field by field.
- [ ] Loopback: the client leaves. Within one second of host time, the host closes that slot with the disconnect cause, not timeout, and holds the seat. The same player claim reclaims it within the hold window.
- [ ] A relevel the host sends after the client has left loads nothing on the client.
- [ ] `ui.exitToDesktop` on a connected client sends the disconnect before exit, and the host's close cause is disconnect. On a host, every client receives the session-ended cause before the host exits.
- [ ] A client whose host raised one of its persisted per-owner values during the session saves that value on leave. After leave and a clean exit, the client's save holds none of the host's replicated global values, and its store equals a fresh boot from its own save.
- [ ] The Scripting surface example runs as a fixture: `leaveSession()` and `endSession()` reach the engine as those primitives from both TypeScript and Luau.
### Manual
- [ ] Two-process loopback (`networking.md` §Manual loopback recipe): the client quits to the menu, stays at the frontend through a host level change, and starts a local game. The host's player list drops the client at once.
- [ ] Two-process loopback: the host quits to the menu. The client lands at its frontend, logs that the host ended the session, and starts a local game.

## Path
- Verb seams: the `returnToFrontend` handler (`sim` `scripting_systems/system_reactions`) beside `loadLevel`'s, which reads `queue.fire_context()` for its source. The drain arm in `App::dispatch_system_commands` (main.rs). `refuse_client_level_verb` and its latch `client_level_verb_warnings`. `SystemReactionKind::class` and `system_reaction_is_machine_local`, whose client `returnToFrontend` clause can go. `UiButtonAction::QuitToMenu` and `ExitToDesktop` (`app/ui_actions.rs`) bypass the reaction queue.
- Reaction install and validation enumerate system reaction kinds; grep `SystemReactionKind::ReturnToFrontend` for every match to extend.
- Leave and end run before the unload is queued. The unload then runs as single-player: `clear_net_level_parity` and `reset_level_scoped_client_state` find no endpoint, so nothing goes to a peer already closing. Check that `active_level_host_named`, `last_relevel` and the endpoint's `ClientSpawnPoseHint` do not outlive the endpoint.
- Role state: the endpoint lives in `Session.net_endpoint`, and `App::is_connected_client` reads it; most per-frame client branches re-derive from it. Values cached at build sit in the role block of `Session::build`: the seat table (`None` on a client), `set_runtime_spawn_authority`, `auto_close_timers.set_enabled`, `scheduler.set_role`, `owner_slot_writes_enabled`, and the `published_host_address` slots.
- Host-side ending seams: `NetClient::update` logs an established connection's end once (`failure_reported`, `ever_connected`) and keeps netcode's `disconnect_reason()` internal. `client_drain_control` logs a `Closing` divergence. A world-less client's `WorldLessPoll::Failed` is acted on only for a host. Leave reads the close reason, not only the edge, so a future host-migration path can branch first on `CloseCause::HostInitiatedLeave` (`crates/net/src/slots.rs`).
- Host close cause: `close_cause_from` maps `DisconnectedByClient` to `Disconnect`. `ServerPoll.disconnects` drops the cause. The seat hold runs for both causes, and the cause survives in `SlotState::Closed`. End reuses `reject`'s `pending_disconnects`.
- Saves: `save_connected_client_per_owner_state` is the client's character save. `should_save_persisted_state` gates global saves on `is_connected_client`, which is the leak once the endpoint drops. `load_persisted_state` and `overlay_persisted_state` run once after mod init today (`StateStoreLifecycle`). The client's own per-owner projection is `sync_client_per_owner_projection`.
- Content: the dev pause menu's quit confirmation already presses `ui.quitToMenu` (`content/dev/scripts/frontend-menu.ts`). `content/dev/scripts/frontend-level-select-fixture.{ts,luau}` binds `returnToFrontend()` to a named reaction and is the natural fixture for the new verbs.
- Rival direction: one verb whose meaning depends on its origin, so a UI press leaves and game logic follows the host. It was the earlier draft. Validation found it strands clients when the host returns to its menu, and the owner chose explicit verbs.
- Rival shape: restart the process as single-player on leave. It keeps "role selected once at startup", but it replays the splash, loses the window and display mode, and in-app joining would discard it.
- Rival shape: rebuild the `Session` without an endpoint. It is heavier, because the session owns the VM, audio and UI, and it buys nothing the shared derivation does not.
- First slice: the transport disconnect plus a loopback test asserting the host's close cause. This falsifies the riskiest assumption, that renet_netcode delivers the disconnect before the endpoint drops.

## Open questions
- Should leaving or an ended session show the player a notice? — **delegated**: the executor decides and reports it in the plan of record. The default is a log line only. Whatever notice ships should also cover a host-named load that fails on the client, which `done/coop--client-spawn-view-handoff` returns to the frontend menu with no message.
