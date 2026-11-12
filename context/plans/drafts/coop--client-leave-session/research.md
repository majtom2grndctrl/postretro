# coop--client-leave-session — research

Read at 3fba30ae. Findings that informed the brief; the brief's Decisions are the conclusions.

## Drift since the first draft
- `done/reaction-body-composition` landed `MachineSource::{HostTimeline, HostMirrored, OwnState}`. UI presses and commits dispatch as `EveryMachine(OwnState)` (`ui_dispatch_context`). Movement events (`drain_frame_named_events`) and crossings reading any own, owner-private or local slot (`crossing_machine_source`) share that source. The first draft's proposed `FireOrigin::LocalPlayer` would have duplicated it. The roadmap's "Local-slot crossing origin" classification is satisfied by `crossing_machine_source`.
- The same plan made client `returnToFrontend` machine-local (`system_reaction_is_machine_local`), so a client lands it after a `wait` and unloads locally. Its AC: a UI press "returns that client to the frontend 300 ms later, and the host stays in the level." The first draft's claim that it drops on a client was stale.
- `done/coop--client-spawn-view-handoff` defers a client's own quit to this brief. Its `follow_relevel_catalog` re-enqueues `LevelRequest::HostLevel` for a client sitting at its menu.

## Routing facts
- `resolve_recipients` reads no `MachineSource`. Under any `EveryMachine` origin, no audience or `everyone` resolves to `Recipients::Local`, and a subject token skips with a once-per-source warning. Crossing and levelLoad installs reject subject tokens. UI sources publish none.
- `FireOrigin::follow_up` keeps `EveryMachine(source)` unchanged.

## Ending a connection
- `NetServer::reject` sends `Divergence(Closing(cause))`, then disconnects once acked. The client logs the cause in `client_drain_control`. The endpoint stays `NetEndpoint::Client` and the process runs disconnected.
- `CloseCause::HostInitiatedLeave` is reserved and never constructed.

## Owner decisions this session
- Leave is player input only. Booting a client is a host-executed close, deferred to the lobby authoring surface. "Any `OwnState` leaves" gives only a cooperative kick that a held or diverged client never receives.
- `@player` for player-input fires is dropped: the default audience already resolves locally.

## Pin table
| id | scenario | ordering | expected outcome |
|---|---|---|---|
