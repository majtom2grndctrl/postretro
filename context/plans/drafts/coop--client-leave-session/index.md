# coop--client-leave-session

Brief · resumable · Epic 15 · reads: `context/lib/networking.md` §Role model, §Slot lifecycle, §Admission and content parity, §Version gates · `context/lib/scripting.md` §5 (Durable State Store), §10.4, §12 (Steps after a `wait`) · `context/lib/boot_sequence.md` §1 · `context/research/coop-session-lifecycle.md` · read at 514b5944

## Problem
The owner raised this while gating client level verbs. Basis: defects read in source, not yet reported from play. A connected player has no way out of a session, and nothing catches a client when its session ends under it. On a client, `returnToFrontend()` and `ui.quitToMenu` unload the level locally and keep the connection, before a `wait` or after one. The host's next relevel then pulls the client back into play, and its menu's level buttons do nothing. When the host quits to its menu, clients receive only a hold, so they sit in a stale level with no pawn. A client whose host vanishes, refuses it at admission or exits is left disconnected but still a client, until it relaunches. Meanwhile each save a client makes erases the single-player world it would return to. The cause: one verb, `returnToFrontend`, both navigates and decides membership, no machine can end its own role mid-run, and nothing keeps a client's own world for its return. When this is done, a client's quit leaves the session cleanly and a host's quit ends it for everyone. Every client whose session ends, for any reason, stands at a working single-player frontend, and its saves hold only its own character and world.

## Decisions
- **Membership and navigation are separate verbs.** `leaveSession()` and `endSession()` change membership and never navigate. Navigation never changes membership (`research/coop-session-lifecycle.md` §5). `returnToFrontend` stays the navigation verb outside a session until a generalized navigation function replaces it, and nothing here assumes a mod has only one frontend. A run's end composes the two, such as `endSession()` then `returnToFrontend()`, and that sequence runs cleanly in every role.

  | Role | `leaveSession` | `endSession` | `returnToFrontend` | `ui.quitToMenu` |
  |---|---|---|---|---|
  | Single-player | no-op, debug log | no-op, debug log | as today | as today |
  | Host | warn: a host ends instead | **end**, staying where it is, now single-player | refused, warned | `endSession`, then frontend |
  | Connected client | **leave** | refused | refused | `leaveSession` |

  - Warnings flag mistakes. Each fires once per source and verb, and a refused `returnToFrontend` names the membership verbs. On a client, a refused step logs at debug instead when its host fires the same step (levelLoad, a shared crossing) or when the reaction already ran a membership verb. Otherwise it warns, naming `leaveSession`.
  - Only role decides, never origin: a `wait` tail, a crossing and a UI press behave alike. This narrows `done/reaction-body-composition`, whose client landing of a post-`wait` `returnToFrontend` unloaded locally. It now refuses.
  - The frontend is outside every session. This narrows `research/coop-session-lobby.md` §2 ("A lobby is the Frontend state with a live endpoint"): a lobby becomes its own optional scene. Until that brief lands, a command-line host still boots to its frontend with an endpoint and admits clients there.
- **Leave.** The client closes its connection cleanly, then unloads and presents the mod's declared frontend as single-player does. It is the one membership change that lands somewhere, because a client cannot keep a world it never owned. Steps after `leaveSession` in the same reaction drop with a debug log. The host closes the slot as a disconnect, not a timeout, and the existing seat hold applies (`netcode` `SeatTable::hold_disconnected_client`).
- **End.** The host closes every slot with a session-ended cause and tears down every remote pawn. Ending holds no seats: the session is gone. Its level keeps running as single-player. The role changes at once, so a navigation step after `endSession` in the same reaction runs as single-player. The host keeps its transport only to deliver the closes, and drops it once every client acknowledges or 2 s of host time pass. Meanwhile that transport admits no one and sends nothing after a client's cause.
- **Any host-side ending leaves.** When an established connection ends, the client takes the leave path, minus the disconnect send. That covers a session-ended cause, a mod-id refusal, a lost host, and any later host close (eviction, password). A client whose every connect attempt fails takes it too. `--connect` is a developer path until in-app joining lands. This narrows `networking.md` §Role model ("Nothing above the transport reacts to that ending") and `scripting.md` §10.4 ("stays a client until relaunch").
- **A client whose session ends stops the host's level at once.** From the moment its role changes, that level runs no tick, crossing or command drain, and loads nothing the host named, until its unload.
- **Exit to desktop takes the same path.** On every exit path, a client leaves and a host ends before the event loop exits, so peers learn at once rather than after the keepalive timeout.
- **A role moves one way, to single-player.** This narrows `networking.md` §Role model ("selected once at startup"). Hosting or joining again needs a relaunch until in-app hosting and joining land.
- **One derivation of role-bound state.** Every value `Session::build` derives from the role is derived in one place, and leave and end reuse it, so they cannot drift. A machine that has left or ended holds what a session built without `--host` or `--connect` holds. The one exception is an ended host, which keeps its own seat, pawn binding and carried loadout, because its level keeps running.
- **Saves stay the player's own.**
  - While connected, a client's saves keep the world it booted with, unchanged on disk.
  - A host close of a participating client first sends that client its own final character values, so a reward written the frame the host ends still reaches the player. A client that never participated keeps its saved character untouched. Leave, whether its own or host-side, saves the client's character before the endpoint drops.
  - Then the client restores its store: every slot the host replicated, and faction sentiment, hold what a fresh boot from its boot world would hold, with its latest character applied. Slots that never replicate keep the values the client wrote.
  - The host's replicated values never reach the ex-client's saves or its single-player world. Character and world progress stay separate (`research/coop-session-lifecycle.md` §7).
- **The wire gains a closing cause and a final-character message** (Wire format). `SessionEnded` lets a client tell an ended session from a lost host. `FinalCharacter` carries its own character values ahead of any host close, an eviction's included. Both are appended variants, so the application protocol advances (`networking.md` §Version gates).
- **Non-goals.**
  - Navigation and the optional lobby. The generalized navigation function that replaces `returnToFrontend`, and a host taking clients to a lobby, belong to the optional-lobby brief (order 2 in the lifecycle map). Until then, a co-op run ends at the frontend through `endSession`.
  - `evictPlayer` belongs to the Phase 3.75 lobby authoring surface, and the session password to its own brief.
  - In-app hosting belongs to the Phase 3.75 lobby authoring surface, and in-app joining to the in-app join brief (order 3).
  - Separate character and world save files, level-boundary saves and run saves belong to the saves brief (order 4). This brief keeps today's single `state.json`.

### Scripting surface
TypeScript in `sdk/lib/ui/reactions.ts`, beside `returnToFrontend()`, exported from `postretro/ui` and declared in the generated `sdk/types` `.d.ts` and `.d.luau`. The Luau mirror in `reactions.luau` uses the same names.

```typescript
import { defineReaction } from "postretro";
import { endSession, leaveSession, returnToFrontend } from "postretro/ui";

// A client's "Leave game" button.
export const leave = defineReaction("pause.leaveSession", leaveSession());
// A run's end on a map-end crossing: every role runs the same steps.
export const runEnd = defineReaction("run.end", [endSession(), returnToFrontend()]);
```

Both take no arguments and return a `SystemReactionStep` (`{ primitive: "leaveSession", args: {} }`, `{ primitive: "endSession", args: {} }`). `endSession` is a host consequence. `leaveSession` is machine-local on a client, as `returnToFrontend` is today, so a client lands it after a `wait`. A late joiner's catch-up skips a `leaveSession` already due, as it skips `returnToFrontend`.

### Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| leave | `SystemReactionKind::LeaveSession` | `"leaveSession"` | `leaveSession()` | `UI.leaveSession()` |
| end | `SystemReactionKind::EndSession` | `"endSession"` | `endSession()` | `UI.endSession()` |
| session-ended cause | `ClosingCause::SessionEnded` (no fields) | appended after `ModId` | n/a | n/a |
| final character | `ServerControlMessage::FinalCharacter { slots }` | appended last | n/a | n/a |

### Wire format
`ServerControlMessage::FinalCharacter { slots: BTreeMap<String, JoinSeedValue> }`, bitcode like every Control payload, sent on Control immediately before the closing divergence of a participating slot. It mirrors `ClientControlMessage::JoinSeed { slots }`: the same durable-key strings, the same `JoinSeedValue` variants, and an empty map when the client has no persisted per-owner values. The client applies only keys its own declarations persist per owner, as the host's join-seed apply does. A separate message keeps `ClosingCause` free of `f64`, so it keeps deriving `Eq`. `ClosingCause::SessionEnded` carries no fields.

## Acceptance
### Role and verbs
- [ ] A connected client running `leaveSession`, directly or when it lands 300 ms after a UI press, ends with no endpoint, the frontend presented, an unload queued, then the backdrop load when one is declared. A later `loadLevel` queues a load and logs no client-drop warning.
- [ ] `ui.quitToMenu` leaves on a connected client, ends the session on a host, and in single-player queues exactly what it queues today.
- [ ] `returnToFrontend` on a host queues nothing, keeps the endpoint, and warns once per source naming the membership verbs, across repeated fires from levelLoad, a crossing and a UI press. On a connected client, directly or when it lands 300 ms after a UI press, it queues nothing and keeps the endpoint: it warns the same from a UI press, and logs at debug from levelLoad, a shared crossing, or after a membership verb in the same reaction. In single-player it queues unload, then the backdrop load when one is declared.
- [ ] `leaveSession` on a host changes nothing and warns once per source. `leaveSession` and `endSession` in single-player change nothing and log at debug only. `endSession` on a client changes nothing: from a UI press it warns once per source naming `leaveSession`, and from levelLoad or a shared crossing it logs at debug only.
- [ ] A client reaction `[leaveSession(), returnToFrontend()]`, `[leaveSession(), loadLevel(L)]`, or `[fire(F), leaveSession()]` whose target F runs `loadLevel(L)`, leaves and queues one frontend return. L never loads, and nothing warns (pin P15).
- [ ] A late joiner whose catch-up holds a `leaveSession` already due stays connected.
- [ ] A reaction running `endSession` then `returnToFrontend`, directly and across a `wait(500)`: in single-player it reaches the frontend with no warning. On a host it ends the session and reaches the frontend when the step lands. On a connected client, fired from levelLoad or a shared crossing, it keeps the endpoint, logs no warning, and lands at the frontend when the host's session-ended cause arrives. From a UI press on a connected client it keeps the endpoint and warns once, naming `leaveSession`. No tail a client parked before it leaves lands after it (pin P4).
- [ ] The Scripting surface example runs as a fixture: `leaveSession()` and `endSession()` reach the engine as those primitives from both TypeScript and Luau.
### Endings
- [ ] A host running `endSession` alone stays in its level as single-player, with every client's pawn gone. Its next `loadLevel` publishes no parity, sends no relevel, and spawns its pawn on that seat with the loadout it carried.
- [ ] Loopback host and two clients, host presses `ui.quitToMenu`: each client receives the session-ended cause and ends with no endpoint at a frontend whose menu starts a local game. The host ends with no endpoint, at its frontend.
- [ ] Loopback: a host at its frontend with one admitted client runs `endSession` from a UI press. The client receives the session-ended cause and ends at its frontend as single-player, and the host holds no endpoint (pin P7).
- [ ] Loopback host whose client never acknowledges: `endSession` drops the transport within 2 s of host time, and an acknowledging client still receives the cause. A host that exits during that window, or exits with no prior end, exits within 2 s of wall time, after every acknowledging client has the cause (pin P12).
- [ ] Loopback host with two clients ends. During the next 2 s, one client's own leave reaches the host, a third client connects, and the host runs `loadLevel`. The host admits no one. After each cause it sends no snapshot, hold or relevel. It holds no seat, and once the window closes its pawns and seat table equal those right after the end (pin P14).
- [ ] A client whose established connection ends, by a lost host or a mod-id refusal at admission, unloads to the frontend as single-player without a relaunch, and its menu starts a local game. A client whose first connect is still retrying does not. A client whose every connect attempt fails ends the same way (pin P11).
- [ ] A client whose host ends the session logs once that the host ended it, and logs no incompatible-host or lost-connection error. A client whose host vanishes logs a lost host once (pin P6).
- [ ] Loopback: the client leaves. Within one second of host time, the host closes that slot with the disconnect cause, not timeout, and holds the seat. The same player claim reclaims it within the hold window.
- [ ] `ui.exitToDesktop`, window close and the quit chord, in a level and at the frontend: on a connected client each sends the disconnect before exit, and the host's close cause is disconnect. On a host, every client receives the session-ended cause before the host exits.
- [ ] A peer on the application protocol before `SessionEnded` is refused at handshake. With `SessionEnded` appended, the shipped `Protocol` and `ModId` closing encodings decode unchanged, and an old peer refused by a new host logs the protocol mismatch.
### After an ending
- [ ] After a client leaves, and after a host ends, every role-bound value equals the value a session built with no network flag holds, apart from an ended host's own seat, pawn binding and carried loadout. One test compares the whole derived role state, so a field added to it later is compared without editing the test.
- [ ] After a client leaves, nothing it received from the host steers it: its next own load is not host-named, and no relevel record or spawn-pose hint from the session applies. A relevel the host sends after the leave loads nothing.
- [ ] A client whose session ends, by a `leaveSession` landed in the frame's first command drain or by a host-side ending, in a frame that then runs fixed ticks, crossings or a command drain with the host's level still installed: no trigger, spawner, auto-close timer, crossing reaction or level verb runs in that level as single-player, nothing queues behind the unload, and it lands at the frontend (pin P1).
- [ ] A relevel received earlier in the frame the client leaves, in the same Control drain as the session-ended cause, or still loading when the session ends, starts or finishes no load of the host's level. The client lands at the frontend with no host-named level installed (pin P3).
- [ ] The host's session-ended cause and the client's own `leaveSession` in one frame, or two `leaveSession` steps in one drain: the client saves once, closes once and queues one frontend return. The second logs at debug only (pin P8).
### Saves
- [ ] A client that booted with its own world value W joins and saves at least once while connected: its save on disk still holds W. Its host raises one of its persisted per-owner values, then the client leaves: the save written at the leave holds that value, and its store holds W, its boot faction sentiment and that value on the single-player seat. After a clean exit its save holds W, that sentiment and its own character rows holding that value, and none of the host's replicated global values or sentiment (pins P9, P19).
- [ ] A client with saved per-owner value V is admitted but not yet participating: world-less at a host's frontend, or held for content parity. The host ends the session, and in a second run the client leaves. In both, its save and store afterwards hold V, not the declared default (pin P13).
- [ ] A client whose own leave is sent in the frame the host ends, before the cause arrives, saves once with the character values it last received, logs no error, and lands at the frontend. The host, whichever arrives first, holds no seat for it (pin P16).
- [ ] A client writes a persisted global and a persisted per-owner slot that never replicate during the session, then leaves and exits: its store holds both on the single-player seat, and its save holds the values it wrote.
- [ ] A client whose leave-time save fails still holds its final character values in its store, and its next clean exit saves them (pin P17).
- [ ] Loopback: a host reaction writes a persisted per-owner value for each player, then runs `endSession` in the same step list. Each client's save after the session ends holds the written value, also when the cause and the last snapshot arrive in one client frame (pin P10).
- [ ] A client that receives `FinalCharacter` in one frame and its session-ended cause in a later one, with a snapshot the host sent before the end applied between them, saves the `FinalCharacter` values (pin P20).
- [ ] A connected client whose host set a persisted global exits to desktop with no prior leave: its save holds its own value of that global, its own faction sentiment and its own character rows, none of the host's (pin P5).
- [ ] A client with a crossing watcher on a slot whose host value differs from its own saved value leaves, by `leaveSession` and by a host-side ending: the restore fires no crossing, the watcher's reaction does not run, and the client lands at the frontend (pin P2).
### Manual
- [ ] Two-process loopback (`networking.md` §Manual loopback recipe): the client quits to the menu, stays at the frontend through a host level change, and starts a local game. The host's player list drops the client at once.
- [ ] Two-process loopback: the host quits to the menu. The client lands at its frontend, logs that the host ended the session, and starts a local game.

## Path
- Verb seams: the `returnToFrontend` handler (`sim` `scripting_systems/system_reactions`) beside `loadLevel`'s, which reads `queue.fire_context()` for its source. The drain arm in `App::dispatch_system_commands` (main.rs). `refuse_client_level_verb` and its latch `client_level_verb_warnings`. `SystemReactionKind::class` and `system_reaction_is_machine_local`: its client `returnToFrontend` clause stays, so client tails land the step and the drain refuses it, and `leaveSession` joins it. A client tail with no machine-local step is not parked at all (`reaction_scheduler` `enroll`, `landing`). `UiButtonAction::QuitToMenu` and `ExitToDesktop` (`app/ui_actions.rs`) bypass the reaction queue. Window close and the quit chord call `event_loop.exit()` directly.
- Reaction install and validation enumerate system reaction kinds; grep `SystemReactionKind::ReturnToFrontend` for every match to extend.
- Role versus transport: `App::is_connected_client` and `Session::build`'s role block read the endpoint variant, but `NetEndpoint::Host` owns the `NetServer` and the remote-pawn maps (`netcode` `endpoint.rs`). End needs the role to change while a closing transport survives. `reject` is the precedent (research §Facts by symbol); end needs its own cause and no hold. `authority_seat_table_or_local_only` is the boot-time precedent for demoting to single-player.
- Seat ledger: a no-flag session takes `SeatTable::new()`; a client has none. An ended host keeps seat 0's binding and carried loadout (`lifecycle_install_world`, `lifecycle_world_handles`).
- Leave and end run before the unload is queued. `clear_net_level_parity` and `reset_level_scoped_client_state` must then find no endpoint. `follow_relevel_catalog` has no role check, and `drain_level_requests` starts a queued load before it reaches a later unload. Check that `active_level_host_named`, `last_relevel` and the endpoint's `ClientSpawnPoseHint` do not outlive the endpoint. A host-side ending changes role in the Control drain (`client_drain_control`), before the frame's ticks.
- Saves: `collected_per_owner_only_state` writes the empty world section; `session.persisted_state` retains the boot document in memory. `save_connected_client_per_owner_state` is the client's character save and needs the endpoint. `exiting` picks its save branch from the endpoint. `overlay_persisted_state` leaves unnamed slots as they are, and `SlotTable` has no reset to defaults. Boot restore skips sentiment seeding on a connected client. The crossing detector's baselines need the restored values before its next pass.
- The clean disconnect is a `NetClient` method that sends netcode's disconnect packets and reports why a connection ended. Nothing above the transport touches a socket (`networking.md` §Transport contract).
- Host-side ending seams: retries stop at `CONNECT_ATTEMPTS`. The next application protocol id is `PRLE`; no append-layout guard covers `ClosingCause` yet. `client_drain_control` logs every non-mod-id closing cause as an incompatible host. A world-less client's `WorldLessPoll::Failed` is acted on only for a host. Leave reads the close reason, so a future host-migration path can branch first on `CloseCause::HostInitiatedLeave`.
- Content: the dev pause menu's quit confirmation already presses `ui.quitToMenu` (`content/dev/scripts/frontend-menu.ts`). `content/dev/scripts/frontend-level-select-fixture.{ts,luau}` is the type-check fixture for the new verbs.
- Rival shape: restart the process as single-player on leave. It keeps "role selected once at startup", but it replays the splash, loses the window and display mode, and in-app joining would discard it.
- Rival shape: rebuild the `Session` without an endpoint. It would clear, by construction, what arrived over the wire, which the brief clears through the store restore and named resets instead. It loses because the session owns the VM, audio and UI.
- First slice: the transport disconnect plus a loopback test asserting the host's close cause. This falsifies the riskiest assumption, that renet_netcode delivers the disconnect before the endpoint drops.

## Open questions
- Should leaving or an ended session show the player a notice? — **delegated**: the executor decides and reports it in the plan of record. The default is a log line only. Whatever notice ships should also cover a host-named load that fails on the client, which `done/coop--client-spawn-view-handoff` returns to the frontend menu with no message.
