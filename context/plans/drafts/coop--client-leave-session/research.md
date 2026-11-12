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
- `NetServer::reject` sends `Divergence(Closing(cause))` and disconnects once acknowledged (`pending_disconnects`). `ClosingCause` has `Protocol` and `ModId`. `WIRE_VERSION` is 39.
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

## Pin table
| id | scenario | ordering | expected outcome |
|---|---|---|---|
