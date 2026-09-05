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
  localhost TCP listener, accept one connection, serve read-only requests against
  the live session, exit when the process exits. Off by default (no flag → no
  socket, no thread).
- A background transport thread that owns the socket, frames length-prefixed JSON,
  and marshals request/reply **bytes** over mpsc — never touching engine state.
- A main-thread frame-boundary drain (Input-stage head) that services queued
  requests against the live `EntityRegistry` with exclusive access, builds an
  `OutputDocument`, and serializes it deterministically.
- One read-only verb: `dump`, carrying a `DumpSpec` (the batch runner's dump
  vocabulary), returning an `OutputDocument`.
- A `ServerHello` handshake frame (protocol version, current map) sent on accept.
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
- **`map_lights` and other live-only enrichments.** Available live (unlike
  headless) but not this slice's shared surface.
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
      `127.0.0.1:<PORT>` receives a `ServerHello` frame (protocol version + current
      map) before sending anything.
- [ ] A `dump` request carrying a `DumpSpec` returns a response whose `dump` is a
      well-formed `OutputDocument` reflecting the live session's current entities and
      player summary, honoring the same `DumpSpec` filters as the batch dump
      (`component`, `tag`, `entities`, `cap`).
- [ ] The transport thread holds no engine type: the code compiles despite
      `ScriptCtx: !Send + !Sync`, and the only values crossing the mpsc channel (both
      directions) are `Vec<u8>`. All request parsing and dump serialization run on
      the main thread.
- [ ] Two `dump` requests serviced against an identical frozen registry state
      produce byte-identical response bytes.
- [ ] A live dump of a given registry state equals the batch runner's dump of that
      same state, except for fields the live sampler does not populate: `events` is
      empty and named in `out_of_frame`. (Proves vocabulary reuse, not a fork.)
- [ ] The listener binds only `127.0.0.1`; the flag accepts a port, never a bindable
      address. Without the flag, no socket opens and no transport thread spawns.
- [ ] With `observability` on and `observe-live` off, the headless batch runner
      behaves byte-identically to before the re-gate; `run_headless` remains
      `observability`-gated.
- [ ] A request whose body is not valid `ObserveRequest` JSON yields a
      `ObserveResponse::Error` frame (built main-side) and the connection stays open; a
      request length prefix over the 64 KiB cap closes the connection without
      allocating the payload; a `dump` when no world is loaded returns a valid "no
      world" response (empty entities, null player), not a crash.
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
`#[cfg(feature = "observability")]` (`main.rs:55`), which gates the whole module.
The live channel runs in the windowed binary and needs this vocabulary without
pulling in the headless driver. Re-gate: declare `mod observability` under
`#[cfg(any(feature = "observability", feature = "observe-live"))]`, and inside
`observability/mod.rs` gate the headless-only surface — `mod driver;` and the
`run_headless` re-export — under `#[cfg(feature = "observability")]` so it is
absent when only `observe-live` is on. Move `build_player_summary` (currently in
`observability/driver.rs`; reads `Transform` + `HealthComponent` off
`registry.local_player_movement_pawn()`) into `observability/document.rs` beside
`build_output_document` so both faces share it; the driver calls it from its new
home. Add `observe-live = []` to `[features]` (dep-free, matching `observability`
and `capture`). This is behavior-preserving for the headless path: the driver and
`run_headless` stay `observability`-gated and unchanged (Invariant: Headless path
unchanged). Any item that legitimately compiles only under `observability` (e.g.
`RunSpec`/`parse_runspec` if unused by the windowed build) stays gated to avoid
dead-code warnings in the windowed build; keep the shared read vocabulary
(`DumpSpec`, `apply_dump`, `build_output_document`, `to_deterministic_json`,
`build_player_summary`, kind mapping) reachable under either feature.

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
`mpsc::channel()` sender → block on the reply receiver with `recv_timeout` → write
the received reply bytes as a response frame. On reply timeout (the main thread runs
no frames — suspended), the thread **closes the connection** rather than hanging.
The thread parses no engine JSON — the reply bytes are a main-thread-built response
frame body, written verbatim (Invariant: Transport thread never touches the
registry; Invariant: bytes-only channel). The public seam is a spawn function taking
the port and the `ServerHello` to send, returning the
`mpsc::Receiver<ServiceRequest>` and the `JoinHandle`. Focused tests: length-prefix
encode/decode round-trip; oversized-prefix rejection closes without allocating;
`ServerHello` serde round-trip; a socket round-trip driving a real
`TcpListener`/`TcpStream` pair against a **fake servicer** loop (draining the
receiver and replying with canned bytes) — falsifies the transport contract without
a window (thin-slice falsification).

### Task 3: Frame-boundary service + main-thread wiring

Wire Task 2's transport to the running windowed session, consuming Task 1's
windowed-available vocabulary. Define the vocabulary-referencing protocol types here
(they cannot live in the engine-free Task 2): `ObserveRequest`, a
`#[serde(tag = "verb", rename_all = "snake_case")]` enum with the one variant
`Dump { #[serde(default)] spec: DumpSpec }`; and `ObserveResponse`, a
`#[serde(tag = "status", rename_all = "snake_case")]` enum with
`Ok { dump: OutputDocument }` and `Error { message: String }`. Add the observe-live
state to `App` (a field holding
the `mpsc::Receiver<ServiceRequest>` and the thread's `JoinHandle`, `Option`,
`#[cfg(feature = "observe-live")]`), constructed at startup only when
`--observe-live <PORT>` is passed — parse the flag as a `u16` port and spawn the
Task 2 thread bound to `127.0.0.1:PORT`. Add a frame-stage function
`observe_live::run_observe_ingress_stage(...)` following the
`netcode::frame_order::run_snapshot_apply_stage` precedent (a small dedicated
function, one call site), invoked at the head of the Input stage in the
`RedrawRequested` arm of `main.rs` — after `frame_timing.begin_frame`, before the
gamepad poll and `ui_dispatch.take_ready/advance_frame`. The stage `try_recv`s all
queued `ServiceRequest`s (non-blocking — never blocks the event loop, dev_guide
§4.2) and for each: parse the payload as `ObserveRequest`; for `Dump`, read the live
registry immutably and build the `OutputDocument` via `apply_dump` +
`build_output_document` + `build_player_summary`, marking `events` empty in
`out_of_frame`; on no world loaded (`App.session` has no world), produce a valid
"no world" document; serialize the `ObserveResponse` via `to_deterministic_json` to a
`String`, and `reply.send(bytes)`. A parse failure or dump error becomes a
`ObserveResponse::Error` frame, not a panic. The `map` in `ServerHello` and the
document's `map`/`ticks_run` come from the live session's current level and tick
counter. Servicing is the sole registry reader at one atomic frame point
(Invariant: single-boundary read, no torn read; Invariant: shared vocabulary, not a
fork). Include focused tests callable without a window: service a `ObserveRequest` against
a constructed registry fixture and assert the `OutputDocument` matches the batch
runner's dump of the same state modulo `events`; two services of a frozen fixture
byte-identical; a no-world request returns the "no world" document. Add the manual
smoke recipe (§Manual verification) to the spec's usage, not to code.

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
| Component kind (in dump) | `ComponentKind` | snake_case via existing `component_kind_snake` |

Note: `ObserveRequest`/`ObserveResponse` are serde-**internally**-tagged enums. This is
sound here because the codec is JSON. The networking.md rule against internally-
tagged enums is bitcode-specific (bitcode cannot round-trip them); it does not apply
to this JSON channel.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Transport thread never touches the registry | Task 2 (thread is codec-only, moves `Vec<u8>`) | Any engine type placed on the channel breaks it; `ScriptCtx: !Send` is the compile wall | AC 3 |
| Bytes-only channel; all serde on the main thread | Task 2 (`ServiceRequest.payload`/reply are `Vec<u8>`), Task 3 (parse + serialize main-side) | A typed request/response crossing the mpsc | AC 3, AC 4 |
| Single-boundary read, no torn read | Task 3 (service is the sole reader, at the Input-head drain, one frame point) | A second reader, or servicing off the drain | AC 4, AC 9 |
| Shared vocabulary, not a fork | Task 1 (re-gate + shared `build_player_summary`), Task 3 (reuse `build_output_document`/`to_deterministic_json`) | Re-implementing dump/serialize on the live side | AC 5 |
| Localhost-only bind | Task 3 (flag is a `u16` port; bind hard-codes `127.0.0.1`) | Accepting a bindable address string | AC 6 |
| Headless path unchanged | Task 1 (driver + `run_headless` stay `observability`-gated; behavior-preserving move) | Widening the driver's gate, or altering `build_player_summary` semantics on the move | AC 7 |

## Extension point (drive verbs — not built here)

The drain seam is shaped to accept a future mutating verb without a bespoke path.
A drive verb carries the runspec command vocabulary (`CommandEntry` /
`MovementCommand` / `AimCommand`, `observability/runspec.rs`), and the service, at
the Input-head drain, builds a `SimCommand` and hands it to the same-frame tick loop
via `sim::simulate_tick_with_presentation_aim` (`sim/mod.rs:393`) — the exact seam
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

Expect: a `ServerHello` naming `campaign-test`, then an `OutputDocument` whose
entities and player position track the live game as it runs.
