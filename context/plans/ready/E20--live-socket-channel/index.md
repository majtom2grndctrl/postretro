# Live Socket Channel (E20) — read-only introspection

## Goal

Let an agent or CI attach to a *running windowed* PostRetro session over a
localhost socket and read back structured world state — the same output vocabulary
the batch runner produces, sampled live at a frame boundary. This is the anchor
capability of Epic 20: live introspection the batch runner fundamentally cannot do
(it exits; this attaches to a running game). The transport foundation built here —
a background thread, mpsc marshaling, and a frame-boundary drain — is the seam a
later drive/mutate slice rides. This slice ships read-only.

**Naming.** The roadmap item is "the live socket channel"; the code vocabulary is
`observe-live` (feature, `--observe-live` flag, `observe_live` module), aligned with
the existing `xtask observe` / `observability` batch surface. "Live" in prose means
the running session, not an identifier.

## Scope

### In scope

- A `--observe-live <PORT>` mode of the `postretro` windowed binary: bind a
  localhost TCP listener, accept one connection at a time (re-accepting after each
  closes), serve read-only requests against
  the live session, exit when the process exits. Off by default (no flag → no
  socket, no thread).
- A background transport thread that owns the socket, frames length-prefixed JSON,
  and marshals request/reply **bytes** over mpsc — never touching engine state.
- A main-thread frame-boundary drain (Input-stage head) that services queued
  requests against the live `EntityRegistry` with exclusive access, builds an
  `OutputDocument`, and serializes it deterministically.
- One read-only verb: `dump`, carrying a `DumpSpec` (the batch runner's dump
  vocabulary), returning an `OutputDocument`.
- A `ServerHello` handshake frame (protocol version, engine version, spawn-time map
  snapshot) sent on accept.
- A `observe-live` cargo feature (dep-free module gate) and the re-gate of the
  shared dump vocabulary so it compiles in the windowed build.
- Byte-identical response for an identical frozen registry state (reuses the
  batch runner's deterministic serializer).

### Out of scope

- **Drive / mutating verbs.** Anything that mutates the sim from outside. Deferred:
  gated on the host-authoritative sim, must ride the existing `SimCommand` ingress
  (§Extension point), its own slice. The transport seam here is built to accept it.
- **Per-tick event history in the live dump.** `events` is a batch-driver
  accumulation across the N ticks it runs; the live sampler reads one boundary and
  runs no scripted tick loop. The live dump reports `events` empty and records it in
  `out_of_frame`. (Entities, player summary, and baked `cell_visibility` are live.)
- **`map_lights`, a monotonic sim-tick clock, and other live-only enrichments.**
  Available live (unlike headless) but not this slice's shared surface. The sim-tick
  clock is why live `ticks_run` reads `0` here — a session tick belongs in its own
  field, not an overload of the batch run-length (roadmap Epic 20 records the need).
- **Concurrent clients.** One connection at a time, serialized request/response.
- **MCP frontend, streaming telemetry, record/replay.** Separate roadmap items.
- **Protocol-crate extraction (`postretro-observability-protocol`).** Deferred to a
  second typed consumer. This slice reuses the `pub(crate)` vocabulary in-crate.
- **A parallel headless path.** Reuse the existing session substrate and vocabulary;
  do not fork a second build (roadmap: one substrate, two entry points).
- **Splitting `main.rs`.** The feature adds a stage call, not logic, to `main.rs`;
  channel logic lives in a new module.

## Direction

**Problem.** The batch runner and frame capture both *exit* to report state — there
is no way to inspect a session while it runs. Epic 20's north star ("attaches to a
running session over a socket to read live state") has no vehicle: the engine has no
live ingress, and session state is main-thread-only, so nothing outside the frame
loop can read it safely.

**Prior commitments.**
- *One shared vocabulary* (roadmap Epic 20). The read request is a `DumpSpec` and
  the response is an `OutputDocument` — the batch runner's exact types, reused, not
  re-invented. Serialized by the same `to_deterministic_json`.
- *One substrate, two entry points* (roadmap; `session/mod.rs` comment: "A net
  endpoint is deliberately omitted, not designed out: Epic 15 Phase 4's dedicated
  server attaches one to this same path later"). This channel attaches to the same
  session substrate; it does not grow a parallel headless path.
- *Game-logic owns all registry mutation* (networking.md §Game-logic-owned apply).
  A read borrows immutably; nothing here mutates. The transport thread is
  compiler-barred from the registry (`ScriptCtx: !Send + !Sync`).
- *Frame ordering* (dev_guide §4.3) and *event-loop ownership* (§4.2). The drain
  slots into the Input stage and never blocks the loop (`try_recv`); the thread
  blocks off-loop.
- *Distinct from `crates/net/`.* That transport is UDP, polled-synchronous, no
  threads. This is a separate localhost TCP transport with its own thread — no
  shared code, no conflation.

**Alternatives rejected.**
- *Service requests directly on the transport thread.* Impossible by construction —
  `ScriptCtx` is `!Send`; the registry cannot cross a thread. Marshaling is not a
  choice, it is the only shape.
- *A bespoke external-write path for live mutation.* Rejected by the non-goal
  ("does not accept arbitrary outside mutation") and the roadmap ("not a bespoke
  path"). Deferred and, when built, host-arbitrated through the `SimCommand` ingress.
- *Reuse the `postretro-net` UDP transport.* Wrong contract (unreliable, no threads,
  registry-blind wire mirrors) and wrong domain (multiplayer). A request/response
  introspection channel wants reliable ordered TCP.
- *Extract the protocol crate now.* Premature (roadmap gates it on a second typed
  consumer). In-crate reuse first; extraction is the refactor this consumer earns.
- *Defer the whole channel until its mutating/MCP consumer lands, and build the
  transport in that slice.* Rejected: the roadmap makes the channel itself "the
  load-bearing new capability" and pairs it with frame capture as "the two anchors
  [that] run in parallel" — live *read* of a running session is the anchor's own
  testable outcome ("attaches to a running session over a socket to read live
  state"), not a stub awaiting a mutate verb. Standing the transport up on a
  read verb also falsifies the marshaling/frame-drain seam while the blast radius
  is smallest, so the later drive verb inherits a proven foundation.

## Acceptance criteria

- [ ] With `--observe-live <PORT>` on a windowed run, a TCP client connecting to
      `127.0.0.1:<PORT>` receives a `ServerHello` frame (protocol version, engine
      version, and the map as of the channel's spawn — empty when no level was
      installed then) before sending anything.
- [ ] A `dump` request carrying a `DumpSpec` returns a response whose `dump` is a
      well-formed `OutputDocument` reflecting the live session's current entities and
      player summary, honoring the same `DumpSpec` filters as the batch dump
      (`component`, `tag`, `entities`, `cap`, and `cell_visibility`). The `events`
      filter is the one exception — the live sampler runs no tick loop, so it reports
      `events` empty regardless of the flag.
- [ ] The transport thread holds no engine type: the code compiles despite
      `ScriptCtx: !Send + !Sync`, and the only values crossing the mpsc channel (both
      directions) are `Vec<u8>`. All request parsing and dump serialization run on
      the main thread.
- [ ] Two `dump` requests serviced against an identical frozen registry state
      produce byte-identical response bytes.
- [ ] A live dump of a given registry state matches the batch runner's dump of that
      same state in `entities`, `player`, `truncated`, and `cell_visibility`, and
      differs only where the live sampler runs no tick loop: `events` is empty, and
      its absence is recorded by `"events"` appended to
      `out_of_frame.present_not_dumped` — the one field in which the live and batch
      `out_of_frame` differ. (Proves vocabulary reuse, not a fork.)
- [ ] The listener binds only `127.0.0.1`; the flag accepts a port, never a bindable
      address. Without the flag, no socket opens and no transport thread spawns.
- [ ] With `observability` on and `observe-live` off, the headless batch runner
      behaves byte-identically to before the re-gate; `run_headless` remains
      `observability`-gated.
- [ ] A request whose body is not valid `ObserveRequest` JSON yields a
      `ObserveResponse::Error` frame (built main-side) and the connection stays open; a
      request length prefix over the 64 KiB cap closes the connection without
      allocating the payload; a `dump` when no world is loaded returns a valid "no
      world" response (empty entities, `player` omitted since `None`), not a crash.
- [ ] Edge timing: the drain runs on a zero-tick frame and services queued requests;
      the reply reflects settled post-previous-tick state. If the main thread runs no
      frames (suspended), the transport thread's reply times out and it closes the
      connection rather than hanging.

## Tasks

### Task 1: Re-gate the shared dump vocabulary for the windowed build

The dump vocabulary (`DumpSpec` in `observability/runspec.rs`; `apply_dump`,
`build_output_document`, `OutputDocument` and its record structs in
`observability/document.rs`; `to_deterministic_json` and the snake_case kind
mapping in `observability/mod.rs`) is today reachable only under
`#[cfg(feature = "observability")]` on the `mod observability` declaration in
`main.rs`, which gates the whole module.
The live channel runs in the windowed binary and needs this vocabulary without
pulling in the headless driver. Re-gate: declare `mod observability` under
`#[cfg(any(feature = "observability", feature = "observe-live"))]`, and inside
`observability/mod.rs` gate the headless-only surface — `mod driver;` and the
`run_headless` re-export — under `#[cfg(feature = "observability")]` so it is
absent when only `observe-live` is on. Move `build_player_summary` (currently in
`observability/driver.rs`; reads `Transform` + `HealthComponent` off
`registry.local_player_movement_pawn()`) into `observability/document.rs` beside
`build_output_document`, moving its private `local_pawn` helper (the
`registry.local_player_movement_pawn()` wrapper it calls) with it, so both faces
share it; the driver calls it from its new home. Add `observe-live = []` to `[features]` (dep-free, matching `observability`
and `capture`). This is behavior-preserving for the headless path: the driver and
`run_headless` stay `observability`-gated and unchanged (Invariant: Headless path
unchanged). Any item that legitimately compiles only under `observability` (e.g.
`RunSpec`/`parse_runspec` if unused by the windowed build) stays gated to avoid
dead-code warnings in the windowed build; keep the shared read vocabulary
(`DumpSpec`, `OutputDocument`, `OutOfFrame`, `build_output_document`,
`build_player_summary`, `to_deterministic_json`, `apply_dump`, kind mapping)
reachable under either feature. Because `mod document`/`mod runspec` are private
and Task 3's `observe_live` is a sibling module, surface the pieces Task 3 names —
`DumpSpec`, `OutputDocument`, `OutOfFrame` (with `headless()`),
`build_output_document`, `build_player_summary` — as `pub(crate)` re-exports at the
`observability` module root, joining the existing `pub(crate) use` lines
(`to_deterministic_json` is already a module-root `pub(crate) fn`). Task 3 builds
the no-world document by struct literal, so `OutputDocument` and
`OutOfFrame::headless()` must be nameable from outside `observability`.

### Task 2: Transport thread + framing codec

A new module `crates/postretro/src/observe_live/`, gated
`#[cfg(feature = "observe-live")]`, containing the **engine-free** transport half —
no dependency on the session or the dump vocabulary, so it is fully testable in
isolation and concurrent with Task 1. It moves opaque bytes; it never parses an
engine request or builds a dump. Define only the engine-free pieces here:
`ServerHello { protocol: u32, map: String, engine: String }` (plain data);
`ServiceRequest { payload: Vec<u8>, reply: mpsc::Sender<Vec<u8>> }`; and
`OBSERVE_LIVE_PROTOCOL: u32 = 1`. (The typed `ObserveRequest`/`ObserveResponse`, which
reference the dump vocabulary, are Task 3's — the thread handles only bytes.)
Framing per the Wire format section: a 4-byte little-endian unsigned length prefix
then that many UTF-8 JSON bytes, for every frame; a request length prefix over the
64 KiB cap is a protocol violation — the thread rejects it before allocating the
body and **closes the connection**. The transport thread (`std::thread`,
`std::net::TcpListener`) binds `127.0.0.1:<port>`, writes the caller-supplied
`ServerHello` frame on accept, then loops: read one request frame → send
`ServiceRequest { payload, reply }` over the provided `mpsc::Sender`, where
`payload` is the raw request-frame body and `reply` is a fresh per-request
`mpsc::channel()` sender → block on the reply receiver with
`recv_timeout(OBSERVE_LIVE_REPLY_TIMEOUT)` → write the received reply bytes as a
response frame. `OBSERVE_LIVE_REPLY_TIMEOUT` is a pinned `Duration` constant declared
beside `OBSERVE_LIVE_PROTOCOL` (`Duration::from_secs(5)`); because the drain runs on
every windowed frame in every boot state, the gap between two drains is one frame's
wall time — normally a display interval, so a merely slow or hitching frame under the
window still drains in time. The timeout fires when no drain completes for the whole
window: an OS suspension, or a single synchronous main-thread frame longer than the
window. A large level install and a cold first-use pipeline warmup both run after the
drain point inside `drive_boot_state_for_redraw`, so their whole cost falls between two
drains — a long one exceeds the window while the main thread is fully alive. On reply
timeout the thread **closes the connection** rather than hanging, then loops back to
`accept()` for the next connection. Closing a connection — reply timeout, oversized
prefix, or client disconnect — never ends the thread or the listener: one connection
is served at a time, but a closed connection frees the listener for the next, so a
single transient suspend or hitch does not retire the channel for the rest of the
process.
The thread parses no engine JSON — the reply bytes are a main-thread-built response
frame body, written verbatim (Invariant: Transport thread never touches the
registry; Invariant: bytes-only channel). The public seam is a spawn function taking
the port and the `ServerHello` to send, returning the
`mpsc::Receiver<ServiceRequest>` and the `JoinHandle`. Binding happens inside the spawned
thread, so the spawn function itself does not fail on a busy port: a bind error is logged
and ends the thread, and the engine runs on without the channel — the netcode
setup-error posture (degrade, never block boot). The `JoinHandle` is never
joined at shutdown: the thread spends its life parked in a blocking `accept()` or
socket read that no exit signal unblocks, so joining it would hang the quit path. It
is a daemon — process exit abandons it; the handle is retained only to keep the
channel endpoint owned for the App's lifetime (and for test teardown), not to join. Focused tests: length-prefix
encode/decode round-trip; oversized-prefix rejection closes without allocating;
`ServerHello` serde round-trip; a socket round-trip driving a real
`TcpListener`/`TcpStream` pair against a **fake servicer** loop (draining the
receiver and replying with canned bytes) — falsifies the transport contract without
a window (thin-slice falsification). These plus the transport-side rows of the
pinned-behaviors table (§Pinned behaviors: P3 re-accept after a close, P4 a second
client queued behind the first, P10 the reply timeout firing when no drain answers
within the window, P12 `ServerHello` re-sent verbatim on re-accept) establish Task 2's
contract; P5 (shutdown never joins the daemon) is manual,
via the smoke recipe.

### Task 3: Frame-boundary service + main-thread wiring

Wire Task 2's transport to the running windowed session, consuming Task 1's
windowed-available vocabulary. Define the vocabulary-referencing protocol types here
(they cannot live in the engine-free Task 2): `ObserveRequest`, a
`#[serde(tag = "verb", rename_all = "snake_case")]` enum with the one variant
`Dump { #[serde(default)] spec: DumpSpec }`; and `ObserveResponse`, a
`#[serde(tag = "status", rename_all = "snake_case")]` enum with
`Ok { dump: OutputDocument }` and `Error { message: String }`. Internally-tagged
enums are correct here despite networking.md's "no internally-tagged enum crosses
the wire" rule: that rule is bitcode-specific (bitcode cannot round-trip an internal
tag); this is a JSON channel, where an internal tag round-trips cleanly. Add the observe-live
state to `App` (a field holding
the `mpsc::Receiver<ServiceRequest>` and the thread's `JoinHandle`, `Option`,
`#[cfg(feature = "observe-live")]`), constructed at startup only when
`--observe-live <PORT>` is passed — parse the flag as a `u16` port and spawn the
Task 2 thread bound to `127.0.0.1:PORT`, handing it the `ServerHello` to write on
accept: `protocol` is `OBSERVE_LIVE_PROTOCOL`, `map` is the currently-installed level
identity — `startup::lifecycle::level_identity(source, App.content_root)` for the
`App.active_level_source`, the empty string when `active_level_source` is `None` (as
at spawn, before any level loads) — and `engine` is `env!("CARGO_PKG_VERSION")`. The registry-blind transport thread writes this frame
verbatim on every accept, so `ServerHello.map` is a spawn-time snapshot that does not
track later host level changes; a client reads the current map from a `dump` response
instead. Add a frame-stage function
`observe_live::run_observe_ingress_stage(...)` following the
`netcode::frame_order::run_snapshot_apply_stage` precedent (a small dedicated
function, one call site), invoked in the `RedrawRequested` arm of `main.rs`
positioned like `drain_script_reload_requests` — after `frame_timing.begin_frame`
but before `drive_boot_state_for_redraw` and its `Frontend`/`Loading`/`Splash` early
returns — so the drain runs on every windowed frame regardless of boot state. The
gamepad poll and `ui_dispatch.take_ready/advance_frame` sit past the `Frontend` early
return, in the Running-only region; a drain placed among them never runs while no
world is loaded (the menu, a level load, early boot), stranding every request that
arrives in those states until its reply times out and the connection closes — the
exact opposite of AC 8's no-world reply. The drain therefore tolerates `App.session`
being `None` (Booting/Splash) as well as a session holding no level; the service
(below) returns the no-world document in those states. Conversely, the full-document path
runs only when `App.level` is `Some`, where `App.session` is necessarily `Some` too — a
level installs after the session, so the registry that path borrows through `App.session`
is always present. The stage `try_recv`s all
queued `ServiceRequest`s (non-blocking — never blocks the event loop, dev_guide
§4.2) and for each: parse the payload as `ObserveRequest`. For `Dump` when a level is
loaded (`App.level` is `Some`), build the `OutputDocument` via `build_output_document`
— which internally runs `apply_dump`, so the two are one call, not two — passing the
live `EntityRegistry` (immutable borrow through the session's `ScriptCtx`), the
retained `LevelWorld` from `App.level` (`build_output_document` reads `cell_visibility`
from it), the request's `DumpSpec` (the `Dump` variant's `spec` field, so the live dump
honors the same `component`/`tag`/`entities`/`cap`/`cell_visibility` filters as the batch
— AC 2), an empty `events` vec, and `build_player_summary(registry, self.camera.yaw)`
(`self.camera.yaw` is the live facing yaw, the analog of the headless runspec-authored
yaw). `build_output_document` stamps `out_of_frame` as `OutOfFrame::headless()`; the
live service then appends `"events"` to the returned document's
`out_of_frame.present_not_dumped`, recording that the live sampler ran no tick loop
(the same list headless already uses to declare what it does not dump). When no level
is loaded (`App.level` is `None` — the menu, a level load, or early boot with no
session), build the no-world document directly rather than through
`build_output_document` (which requires a `LevelWorld` this path lacks): `map` the empty
string, `ticks_run` 0, empty `entities`, `truncated` 0, `player` `None`, `cell_visibility` `None`, `events` empty,
and `out_of_frame` the same headless declaration with `"events"` appended. Serialize
the `ObserveResponse` via `to_deterministic_json` to a `String`, and
`reply.send(bytes)`; `reply.send` returns `Err` when the transport thread already
dropped that request's reply receiver — its `recv_timeout` fired and closed the
connection while the `ServiceRequest` still sat in the queue (a request outlives its
reply channel on the timeout path) — and the drain discards that `Err` and services
the next request, never unwrapping it. A parse failure or dump error becomes a
`ObserveResponse::Error` frame, not a panic. The document's `map` is re-read live at each drain (the same `level_identity` of the
current `App.active_level_source`), so it tracks host level changes; `ticks_run` is
`0` — the live sampler runs no tick loop, mirroring the no-world path (no session-wide
fixed-tick counter exists to read; a running session's lifetime tick count, if wanted,
is a deferrable live-only enrichment). Servicing is the sole registry reader at one atomic frame point
(Invariant: single-boundary read, no torn read; Invariant: shared vocabulary, not a
fork). Include focused tests callable without a window: service a `ObserveRequest` against
a constructed registry fixture and assert the `OutputDocument` matches
`build_output_document` of the same inputs (the shared builder — not `run_headless`,
which is `observability`-gated) modulo `events`; two services of a frozen fixture
byte-identical; a no-world request returns the "no world" document; and the windowless
service-side rows of the pinned-behaviors table (§Pinned behaviors: P1 no-world reply,
P2 a timed-out request discarded without a panic, P6 an empty-queue drain, P7 the
serialized one-request-per-connection contract, P9 a zero-tick-frame reply). P8
(one-frame latency) and P11 (`ServerHello.map` staleness across a level change) are
manual, covered by the smoke recipe. Add the manual smoke recipe (§Manual
verification) to the spec's usage, not to code.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — disjoint. Task 1 re-gates the
observability module and moves `build_player_summary`; Task 2 builds the engine-free
`observe_live` transport module and needs neither the vocabulary nor the session.
Task 2 self-falsifies the transport contract against a fake servicer.

**Phase 2 (sequential):** Task 3 — the integrating slice. Consumes Task 1's
windowed vocabulary (`DumpSpec`, `build_output_document`, `to_deterministic_json`,
`build_player_summary`) and Task 2's transport (`ServiceRequest`, `ServerHello`,
the spawn fn); defines the typed `ObserveRequest`/`ObserveResponse`; wires the service,
the frame stage, the `App` field, and the CLI flag; exercises the real transport →
drain → registry → serialize → reply round-trip end to end.

## Wire format

Every frame on the socket — `ServerHello`, request, response — is a **4-byte
little-endian unsigned (`u32`) length prefix** followed by exactly that many bytes
of **UTF-8 JSON**. Mirrors no existing PRL/bitcode layout: this is a fresh
localhost debug transport, deliberately JSON (not bitcode) so it stays untyped until
the protocol crate is earned, and human-readable for agents.

- Length prefix: `u32`, little-endian, unsigned. Counts body bytes, prefix excluded.
- Request body cap: 64 KiB. A prefix exceeding the cap is rejected before allocating
  the body (guards against a hostile or buggy client). Response bodies are
  server-produced and uncapped.
- Empty/optional fields follow the reused `DumpSpec` serde defaults (`#[serde(default)]`).
- `ServerHello` is the first frame the server writes, unsolicited, on accept.
- One response frame per request frame, in order (single serialized connection).
  A transport-level violation (oversized prefix, reply timeout) closes the
  connection with no further frame; application errors return a normal frame.

## Boundary inventory

Rust ↔ wire JSON (this channel crosses no Lua/FGD boundary). snake_case throughout,
matching the batch vocabulary.

| Name | Rust | Wire / serde JSON |
|---|---|---|
| Handshake | `ServerHello { protocol, map, engine }` | `{"protocol":1,"map":"…","engine":"…"}` |
| Read request | `ObserveRequest::Dump { spec }` | `{"verb":"dump","spec":{…DumpSpec…}}` |
| Dump filter | `DumpSpec` (reused verbatim) | `{"component":…,"tag":…,"entities":…,"cap":1000,"events":…,"cell_visibility":…}` |
| OK response | `ObserveResponse::Ok { dump }` | `{"status":"ok","dump":{…OutputDocument…}}` |
| Error response | `ObserveResponse::Error { message }` | `{"status":"error","message":"…"}` |
| Component kind (in dump) | `ComponentKind` | snake_case from `ComponentValue`'s own serde `kind` tag (which `component_kind_snake` mirrors) |

Note: `ObserveRequest`/`ObserveResponse` are serde-**internally**-tagged enums, sound
here because the codec is JSON, not bitcode (Task 3 carries the rationale against
networking.md's bitcode-specific rule).

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Transport thread never touches the registry | Task 2 (thread is codec-only, moves `Vec<u8>`) | Any engine type placed on the channel breaks it; `ScriptCtx: !Send` is the compile wall | AC 3 |
| Bytes-only channel; all serde on the main thread | Task 2 (`ServiceRequest.payload`/reply are `Vec<u8>`), Task 3 (parse + serialize main-side) | A typed request/response crossing the mpsc | AC 3, AC 4 |
| Single-boundary read, no torn read | Task 3 (service is the sole reader, at the Input-head drain, one frame point) | A second reader, or servicing off the drain | AC 4, AC 9 |
| Shared vocabulary, not a fork | Task 1 (re-gate + shared `build_player_summary`), Task 3 (reuse `build_output_document`/`to_deterministic_json`) | Re-implementing dump/serialize on the live side | AC 5 |
| Localhost-only bind | Task 3 (flag parses as a `u16` port, never a bindable address) + Task 2 (the thread's bind hard-codes `127.0.0.1`) | Accepting a bindable address string | AC 6 |
| Headless path unchanged | Task 1 (driver + `run_headless` stay `observability`-gated; behavior-preserving move) | Widening the driver's gate, or altering `build_player_summary` semantics on the move | AC 7 |

## Pinned behaviors

Invariants above state outcomes; these rows pin the mechanics that make them
observable — reachability across boot states, the `send`-after-timeout race,
re-accept, zero-tick chains, and batching arithmetic. Most are windowless unit tests;
the three marked `manual` need the running window (the smoke recipe). Task 2 and
Task 3 test bullets reference these ids rather than restate the scenarios.

| id | scenario | ordering | expected outcome | Kind |
|---|---|---|---|---|
| P1 | Request arrives while no world is loaded (menu / `Frontend`, or during `Loading`). | connect → `ServerHello` → `dump` while no world → drain runs before `drive_boot_state_for_redraw` → services with a session that has no world, and with no session at all. | Valid "no world" document (empty entities, `player` and `cell_visibility` omitted since `None`), built directly (not via `build_output_document`); connection stays open; no timeout. | unit |
| P2 | Request times out, then the drain services it. | transport thread enqueues `ServiceRequest` → `recv_timeout` fires, reply receiver dropped, connection closed → a later drain `try_recv`s the stale request → `reply.send` returns `Err`. | Drain discards the `Err`, no panic, services the next request; engine keeps running. | unit |
| P3 | Reconnect after a connection closes (timeout or disconnect). | client A served → A closes → transport thread loops to `accept()` → client B connects. | B receives `ServerHello` and is served; the port is not dead after A's close. | unit |
| P4 | Second client connects while the first is open. | A open → B `connect()` → OS backlog holds B → A closes → transport thread `accept`s B. | B's `connect` does not error; B is not served (no `ServerHello`) until A closes; A is never starved by B. | unit |
| P5 | Process exits while an agent is connected but idle. | agent parked (no request) → user quits (`event_loop.exit`) → `App` dropped. | Quit completes promptly; the `JoinHandle` is not joined, so the parked read never hangs shutdown. | manual |
| P6 | Drain runs with an empty queue (every idle frame). | empty mpsc → `try_recv` returns `Empty` immediately. | No-op: no registry borrow, no allocation, event loop never blocks. | unit |
| P7 | Two requests queued for one drain from a single client. | transport thread reads req1 → enqueues → blocks on reply1 → cannot read req2 until reply1 is written. | At most one live request per connection reaches any drain; >1 only across a stale timed-out request plus a reconnect. | unit |
| P8 | A same-frame mutation must not appear in this frame's reply. | frame N: drain reads registry (= end of N−1) → snapshot apply mutates → tick loop mutates. | Reply equals end-of-frame-(N−1) state; a frame-N mutation appears only in the next reply (one-frame latency). | manual |
| P9 | Request drained on a run of consecutive zero-tick frames. | frames N…N+k all zero-tick → drained at N+k. | Reply reflects the most recent frame that ran ≥1 tick; state is stable, never torn. | unit |
| P10 | No reply arrives within the timeout window. | transport thread enqueues `ServiceRequest` → no drain sends a reply within `OBSERVE_LIVE_REPLY_TIMEOUT` → `recv_timeout` returns `Err(Timeout)`. | Thread closes the connection (no response frame) and loops back to `accept()`; the listener survives. The window elapses only when no drain completes for it — an OS suspension, or one synchronous main-thread frame (a large level install, cold pipeline warmup) longer than the window — not a stream of short frames, each of which drains a multi-second `Loading` in time. | unit |
| P11 | Map changes mid-connection; `ServerHello.map` vs. `dump.map`. | connect at menu → `ServerHello.map` = connect-time value → load level → `dump`. | `ServerHello.map` does not update; `dump.map` is live (`dump.ticks_run` is `0`, not a live counter); clients read the current map from the dump. | manual |
| P12 | A client connects after a level loads; `ServerHello` on the re-accepted connection. | channel spawns before any level (`ServerHello.map` = empty) → level loads → client A served → A closes → thread re-accepts → client B connects. | B's `ServerHello` is byte-identical to A's — the empty spawn-time `map`, not the loaded level; the thread re-sends the verbatim snapshot on every accept. B reads the current map from a `dump`. | unit |

## Extension point (drive verbs — not built here)

The drain seam is shaped to accept a future mutating verb without a bespoke path.
A drive verb carries the runspec command vocabulary (`CommandEntry` /
`MovementCommand` / `AimCommand`, `observability/runspec.rs`), and the service, at
the Input-head drain, builds a `SimCommand` and hands it to the same-frame tick loop
via `sim::simulate_tick_with_presentation_aim` (`sim/mod.rs`) — the exact seam
the windowed (`build_sim_command`) and headless (`simulate_tick`) paths already
share. It is host-arbitrated inside the fixed tick, consequences replicating through
`crate::netcode`; it is never a direct registry write from the transport thread.
Building it is a separate slice, gated on the host-authoritative sim (roadmap).

## Manual verification

The read-against-a-real-running-session behavior (AC 1, AC 2) is not fully provable
by a windowless test. Smoke recipe:

```bash
# Terminal 1 — run the windowed engine with the live channel on a port.
cargo run -p xtask -- run --features observe-live -- --observe-live 8998 \
  content/dev/maps/campaign-test.prl

# Terminal 2 — read the ServerHello, then request a dump.
#   Frame = 4-byte LE length prefix + JSON body. A tiny client (python/nc-with-framing)
#   connects to 127.0.0.1:8998, reads the ServerHello frame, sends
#   {"verb":"dump","spec":{}} framed, and prints the response document.
```

Expect: a `ServerHello` carrying the spawn-time `map` snapshot (empty when the channel
spawned before `campaign-test` loaded), then an `OutputDocument` whose `map` names
`campaign-test` and whose entities and player position track the live game as it runs.
