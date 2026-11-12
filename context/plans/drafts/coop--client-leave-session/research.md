# coop--client-leave-session — research

Read at 514b5944. Findings that informed the brief; its Decisions are the conclusions. The whole lifecycle, today and target, is mapped in `context/research/coop-session-lifecycle.md`; this brief is order 1 there.

## Why the shape changed
- The first draft gave UI presses a new `FireOrigin::LocalPlayer`. `done/reaction-body-composition` had since landed `MachineSource::OwnState`, which UI presses, movement events and own-slot crossings share (`ui_dispatch_context`, `drain_frame_named_events`, `crossing_machine_source`).
- The refresh split UI input out as `MachineSource::LocalInput` and made only it leave. Direction review returned *Reshape*. The host's mirrored `returnToFrontend` (a levelLoad tail, a shared crossing) was the only thing that took a client to its menu when the host went to its own. On a host unload, the client receives only `Holding(HostLevelAbsent)`, and `client_drain_control` demotes on it without unloading.
- The owner separated navigation from membership: explicit `leaveSession`, `endSession` and `evictPlayer`, one future navigation function, and an optional mod-declared lobby. Origin no longer decides anything here.

## Facts by symbol
- `App::return_to_frontend` presents the menu and queues `frontend_return_requests` with no role branch. `ui.quitToMenu` calls it directly (`UiButtonAction::QuitToMenu`).
- `system_reaction_is_machine_local` makes client `returnToFrontend` machine-local, so a client tail lands it. `done/reaction-body-composition` AC: a UI press "returns that client to the frontend 300 ms later, and the host stays in the level."
- `follow_relevel_catalog` enqueues `LevelRequest::HostLevel` for a client at its menu unless the life repeats.
- `NetClient` has no disconnect method. `NetClient::update` logs an established connection's end once and exposes no reason. On a refusal, `client_drain_control` logs the cause, the endpoint stays `NetEndpoint::Client`, and the process runs disconnected.
- `NetServer::reject` sends `Divergence(Closing(cause))`, closes the slot as `Timeout`, and disconnects once acknowledged (`pending_disconnects`), applied inside `NetServer::update`. `ClosingCause` has `Protocol` and `ModId`. An appended Control variant advances the application protocol (now `PRLD`); `WIRE_VERSION` (39) moves only if the append-layout guard shows a shipped encoding changed.
- `close_cause_from` maps `DisconnectedByClient` to `CloseCause::Disconnect`, otherwise `Timeout`. `CloseCause::HostInitiatedLeave` is reserved and never constructed.
- The transport runs netcode `ServerAuthentication::Unsecure`.

## Saves
- `state.json` (`state_path`) holds global `persist` slots, per-owner rows keyed by player id, and faction sentiment. It is read once per process after the first mod-init commit.
- Single-player and host exit saves write globals plus the local player's per-owner rows for `Seat(0)` (`collect_per_owner_state`). A joining client's `JoinSeed` reads the same rows (`join_seed_from_persisted_state`). Character progress therefore already crosses single-player and co-op.
- A connected client writes only its own per-owner rows, every 60 s and at clean exit (`save_connected_client_per_owner_state`).
- `should_save_persisted_state(can_save, is_connected_client)` gates global saves on the endpoint alone. Once a client drops its endpoint, its slot table still holds the host's replicated globals, and its exit would save them as its own.

## Owner decisions this session
- Leaving and ending are explicit membership verbs. Navigation is one generalized function, owned by the optional-lobby brief.
- A host removes a player with `evictPlayer`, a host-executed close. A held or content-diverged client receives no state a script could act on.
- Character and world progress save separately. Gear scope is the author's choice. Both belong to the saves brief; this brief only keeps each machine's saves its own.
- A session password is an engine admission gate, in its own brief.
- Review round 1: a client's saves keep the player's own world (fixing the empty world section each client save writes today). A host close carries the client's final character values, the mirror of `JoinSeed`. A client whose connects all fail lands at single-player, because `--connect` is a developer path. The owner also approved the technical corrections: a client lands `leaveSession` after a `wait`; end keeps a closing transport for up to 2 s; an ended host keeps its own seat; a client whose session ends stops the host's level at once.

## Pin table
| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Session-ended cause or lost host on a frame with ticks; the client's pawn stands in a trigger that runs `loadLevel` | Role changes in the Control drain; ticks, crossings and both command drains follow before the unload drains | No level game logic runs as single-player; the level queue holds only the leave's frontend requests |
| P2 | Watched slot holds host value H; the client's own save holds S ≠ H | Leave reloads the store, then the same frame's crossing stage runs | No crossing fires from the reload; the next real change fires normally |
| P3 | Relevel(L) then leave in one frame; Relevel then session-ended cause in one drain; client Loading L when the host ends | `HostLevel(L)` queued or in flight before the leave's unload | L never begins or is cancelled; client at frontend, not host-named, no relevel record |
| P4 | Host: `endSession`, `wait`, `returnToFrontend`. Client: a levelLoad tail parked, then leave | End keeps the host's scheduler role; leave changes the client's | Host tail lands as single-player and navigates; client tail is dropped |
| P5 | Client exits to desktop while connected; host set global G | Exit leaves, then the exit save runs as single-player | Save holds the client's own G |
| P6 | Session-ended cause, then the transport disconnect | Cause drained before the transport reports the end | One ended-session line; no lost-connection error |
| P7 | Host at frontend, client admitted world-less | `endSession` drains in the frontend frame | Same outcome as an end from a level |
| P8 | Cause then own leave in one frame; two leaves in one drain | First leave changes role; second runs as single-player | One save, one close, one frontend return |
| P9 | A client booted with world value W saves during the session, then leaves and exits | Client saves keep W; leave restores; exit saves as single-player | Disk, store and exit save hold W: not the default, not the host's value |
| P10 | Host writes each player's persisted per-owner value, then ends, in one reaction | Write, end and the frame's snapshot send in one host frame; client receives cause and snapshot in one frame, Control drained first | Each client saves the written value, carried by the close, before it drops its endpoint |
| P11 | `--connect` to a host that never answers | Last connect attempt fails | Client takes the leave path to a single-player frontend |
| P12 | Host ends, then exits; one client never acknowledges | Exit waits on pending closes | Host drops its transport within 2 s; acknowledging clients still receive the cause |
