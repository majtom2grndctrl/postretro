# Multiplayer Netcode (Epic)

> **Milestone 15 design reference** (`plans/roadmap.md`). Defines the networking model,
> stack, architectural seams, and a six-phase build order. Per-phase implementation
> specs are **reserved** — each phase gets its own `/draft-spec` → review →
> `/orchestrate` cycle as it opens. Rationale and the codebase seam map live in
> `research.md`.

## Goal

Add **co-op multiplayer** to PostRetro: an authoritative client-server engine where
a host runs the world and up to 16 players share a campaign level. The model is
**authoritative server + snapshot replication** (Quake/Source/Overwatch lineage),
not deterministic lockstep — sidestepping the cross-architecture `f32` determinism
trap (`research.md` §1–2). This reverses the standing "Multiplayer / networking"
non-goal; it is a deliberate strategic direction, not incremental polish.

## Prerequisites

**M10 (animated enemies)** — co-op combat needs enemies to fight, and M10's `Agent` +
AI-brain components are what replicate as server-authoritative NPCs. M10 is in final
code review and lands before netcode begins. Phases 0–2 (extraction, transport,
movement prediction) are enemy-free and depend on nothing in M10; the combat-bearing
phases (1.5, 3–5) build on its components. The per-phase combat specs are drafted
against merged M10 code, so enemy-replication detail is grounded, not assumed.

## Scope

### In scope
- **Authoritative client-server** model: server (the host) owns world state; clients
  predict locally and reconcile toward server snapshots.
- **Listen-server architecture**, built to a seam that lets a headless **dedicated
  server** be split out later without re-architecting authority.
- **Up to 16 players**, co-op campaign first.
- **Transport + replication stack**: `renet`/`renetcode` transport, hand-rolled
  replication (`lightyear` as design blueprint), `bitcode` serialization, custom
  snapshot delta.
- **Client-side prediction** for local-player **movement** (including dash) and
  **projectiles** (predicted-entity → server-confirmed handoff).
- **Favor-the-shooter hitscan** validation against a short single-entity history.
- A **headless simulation seam**: the fixed-tick game logic runs with no
  wgpu/winit dependency, so server and client share one tick path.
- The supporting essentials: **time-sync**, **snapshot ack + delta baseline**,
  **join-in-progress**, a **latency-simulation harness**, and a **protocol/version
  handshake**.
- **N-player set-piece design**: how scripted reveals / monster closets / waves
  behave with multiple players — a gating design milestone.

### Out of scope (non-goals)
- **Deterministic lockstep / rollback** netcode (the model this epic rejects).
- **Full server-rewind lag compensation** — per-tick hitbox-history rewind of all
  players + AI. Hitscan uses short single-entity history; that is the ceiling.
- **Prediction of non-movement abilities with heavy world side-effects.** These stay
  server-authoritative with cosmetic local feedback only.
- **Competitive PvP, matchmaking, ranked, anti-cheat hardening.** Co-op PvE frame.
- **Peer-to-peer / mesh topologies.** Single authoritative host only.
- **Networked scripting VMs / replicated script execution.** The server runs the
  engine-owned tick systems and the IR evaluator; per-tick script *callbacks* were
  removed with the live VM (`done/remove-live-vm/`), so there is no script execution to
  replicate. Clients receive replicated state, never script callbacks.
- **Replicated audio-event channel.** Entity-emitted sound events are client-local —
  derived from replicated state plus local input. Server-confirmed outcomes (damage,
  kills) drive their own client-side event on confirmation; no sound rides the wire.
- **Save/load of networked sessions, voice chat, server browser / NAT punchthrough
  matchmaking service.** Direct connect (IP/invite) only for this epic. A networked
  session disables the single-player save/persist path.

## Acceptance criteria

Epic-level, observable, and durable across implementation rewrites. Each maps to a
phase (parenthetical). Phase 0's items split per `experimental_spikes.md`: the
extracted-seam-runs-correctly item is an **honesty gate**; the divergence number is a
**measured finding** (recorded, not threshold-gated).

- [ ] The fixed-tick game logic advances one tick through the full order (transform
  snapshot → movement → weapon → death sweep) with **no wgpu/winit dependency**; a
  headless harness ticks N pawns from a recorded input stream. (Phase 0)
- [ ] **Measured finding:** same-input simulation divergence over N ticks is recorded
  under forced-divergence conditions, feeding the Phase 2 reconciliation tolerance.
  (Phase 0)
- [ ] Two engine instances (one host, one client) over loopback/LAN exchange
  authoritative snapshots; a **remote pawn renders smoothly via interpolation**.
  (Phase 1)
- [ ] A client connecting **after level start** receives a full baseline, then
  deltas, and converges to the host's **entity/registry state** (**join-in-progress**);
  set-piece-progress convergence for mid-piece joiners is a Phase 1.5 concern. (Phase 1)
- [ ] A client built against a **mismatched protocol/version is rejected** at
  handshake with a logged reason; no partial-state application. (Phase 1, honesty gate)
- [ ] The **latency-sim harness** injects configurable RTT / jitter / loss; remote
  interpolation stays smooth at a stated baseline (e.g. 100 ms RTT, 5% loss). (Phase 1)
- [ ] **Design milestone:** a playable co-op set-piece where a scripted reveal/wave
  triggers correctly with 2+ players present, with documented trigger-ownership
  semantics. Gates the combat phases. (Phase 1.5)
- [ ] The **local pawn responds to input with no perceptible delay** at 100 ms RTT;
  on a corrected snapshot it reconciles without visible rubber-banding under normal
  conditions. (Phase 2)
- [ ] A client's **hitscan shot registers damage on a moving remote target** with
  favor-the-shooter tolerance; **HP changes only on server confirmation** while the
  client shows immediate cosmetic feedback. (Phase 3)
- [ ] A client-fired **projectile appears instantly** and its predicted entity
  **hands off to the server-confirmed entity** with no visible duplicate or pop under
  normal latency. (Phase 4)
- [ ] **16 players** on one listen-server host stay within a stated host-upstream
  **bandwidth budget**; the sim/host boundary exposes a **headless server entry
  point** (dedicated-server readiness). (Phase 5)
- [ ] `context/lib/index.md` §4 and `entity_model.md` §9 non-goals are reconciled with
  the new direction (see Tasks). (At promotion)

## Tasks

Each task is a **phase** with its own observable bar. Phases are the unit of the
later per-phase spec cycle; this epic does not break them into sub-tasks.

### Phase 0: Headless simulation seam + cross-arch reconciliation spike
Extract the fixed-tick game logic out of `main.rs`'s render-interleaved
`RedrawRequested` handler into a headless `simulate`-style seam that advances one
tick from `(registry, per-tick input, &CollisionWorld, dt)` with **no GPU/window
dependency** — the same function the server and the client both call. Today the tick
order (`snapshot_transforms` → `run_movement_tick` → `run_weapon_fire_tick` →
`run_death_sweep` → `push_state`) is inlined in `main.rs` (5,593 lines); the movement
tick lives in `movement/mod.rs` (6,055 lines). **Split-before-extend** both files
along seams already visible (boot/frame-loop orchestration vs. the tick step; the
movement substrate vs. its host glue) before adding the seam. Then the **spike**:
stand up a headless harness that runs the extracted tick over a recorded input
stream and measures same-input position divergence under forced-divergence conditions
(forced intermediate rounding and/or a second CI architecture) — a measured finding
that sets the reconciliation premise. Deliverable: the extracted seam + the
divergence number.

### Phase 1: Transport + replication foundation
Stand up `renet`/`renetcode` transport and the hand-rolled replication layer
(`lightyear` as blueprint) with `bitcode` serialization and a custom snapshot delta.
The server serializes post-tick entity state (`ComponentValue` deltas against an
acked baseline); the client deserializes and applies to its local registry, mapping
network ids to local `EntityId`s. Remote entities render through the existing
previous/current interpolation path. **Folded essentials, all in this phase:**
time-sync (client tick clock ↔ server), snapshot ack + delta baselines,
join-in-progress (baseline-then-delta for a mid-level joiner), a latency-sim harness
(artificial RTT/jitter/loss for single-machine testing — durable dev tooling reused by
Phases 2–4 for reconciliation and projectile-feel tuning, not a Phase 1 throwaway), and
a protocol+version handshake that rejects mismatched builds. **Outcome:** host + client (incl. a
mid-level joiner) over loopback/LAN, remote pawns interpolating smoothly.

### Phase 1.5: N-player set-piece design milestone (gating)
**Design-first, pulled forward.** Resolve how the engine's first-class set-pieces —
scripted reveals, monster closets, triggers, waves — behave with N players: trigger
ownership (any-player vs. all-players vs. host), reveal/spawn fan-out, progress
tracking, and co-op death/respawn **policy** (where/when players respawn, shared lives,
spectate-on-death — the *mechanism* of reconciling a respawned pawn is Phase 2's).
Includes **set-piece-progress replication** so a mid-piece joiner converges (Phase 1's
join-in-progress delivers entity state; set-piece progress rides on top). Deliverable is
a short design note **plus** one playable co-op set-piece proving the chosen semantics.
This **gates** the combat phases: it answers "is co-op fun" before Phases 3–4 commit to
a combat shape.

### Phase 2: Local-player movement prediction + reconciliation
The client predicts its own pawn immediately from local input (the `MovementInput`
command — `wish_dir`, jump, dash edge, sprint, crouch intent, `facing_yaw`),
buffering inputs by tick. The server is authoritative. On each authoritative snapshot
the client reconciles: rewind to the acked tick, re-apply buffered inputs, and snap
only when divergence exceeds the Phase 0 tolerance. Movement-only (dash included,
since dash is a movement state). Replication targets the **mutable tick subset** of
`PlayerMovementComponent`, never the descriptor params both sides hold. An authoritative
respawn (or any large position jump) reconciles as a **teleport** — snap, do not
interpolate, reusing the `FrameTiming::hold_state` teleport path — distinct from the
co-op respawn *policy* Phase 1.5 owns.

### Phase 3: Server-authoritative hitscan combat
Weapon fire becomes server-authoritative. The client sends a fire intent (with aim
pitch carried into the tick command) and shows immediate cosmetic muzzle/impact
feedback; the server runs `weapon::tick`/`fire_hitscan` and is the sole authority on
damage. **Favor-the-shooter** validation tests the shot against a **short
single-entity history** of the target (cheap, generous tolerance) — explicitly **not**
full server-rewind. Render-rate look (yaw/pitch) is sampled into the per-tick fire
command at fire time; favor-the-shooter tolerance must absorb the render-vs-tick
aim-quantization gap, not only target-position history. HP on all clients changes only
on server confirmation. Non-movement abilities with heavy world side-effects follow the
same rule: server-authoritative, cosmetic local feedback.

### Phase 4: Predicted projectiles (predicted-entity → confirmed handoff)
Client-side predicted projectiles for weapon feel: a locally-fired rocket/grenade
spawns a **predicted entity** instantly; when the server's confirmed projectile
arrives, the client rebinds the predicted entity's network id to the confirmed one
with no visible duplicate or pop. This is the bug-prone reconciliation/de-dup
machinery, accepted deliberately for the weapon feel it buys. Playtest bar: "do
projectiles feel crisp." Server stays authoritative on projectile outcomes (damage,
despawn).

### Phase 5: Scale + dedicated-server readiness
Validate **16 players** on one listen-server host within a stated **host-upstream
bandwidth budget**; tune delta compression and add interest management only if the
budget demands it. Prove the **dedicated-server seam**: the host/sim boundary
compiles and runs as a **headless server entry point** (no client half), confirming
the Phase 0 extraction delivered the split-later promise.

## Sequencing

**Phase 0 (sequential):** headless seam + spike — blocks everything. Nothing networks
until the tick runs headless and the divergence premise is measured.
**Phase 1 (sequential):** transport + replication foundation — consumes the Phase 0
seam; every later phase rides this wire.
**Phase 1.5 (sequential gate):** N-player set-piece design — consumes Phase 1's
multi-client substrate; gates the combat phases. Phase 2 may begin in parallel (it
does not depend on set-piece semantics), but Phases 3–4 wait on this gate.
**Phase 2 (sequential):** movement prediction — consumes Phase 1 replication and the
Phase 0 tolerance number.
**Phase 3 (sequential):** hitscan combat — consumes Phase 2 (predicted local pawn +
reconciliation) and the Phase 1.5 gate.
**Phase 4 (sequential):** predicted projectiles — consumes Phase 3's authoritative
combat path and Phase 2's prediction/reconciliation machinery.
**Phase 5 (sequential):** scale + dedicated readiness — consumes the full stack;
validates the whole at 16 players.

The chain is mostly sequential by dependency. The one parallelism: Phase 2 (movement)
may run alongside the Phase 1.5 design gate. Movement prediction owns only the
respawn-as-teleport *mechanism* (Phase 1.5 owns co-op respawn *policy*), so it does not
depend on set-piece semantics.

### Promotion status (landed at Milestone 15 registration)
Durable capture that landed when this epic was promoted to a milestone:
- **Registered as Milestone 15** in `context/plans/roadmap.md` (detail-on-open phases,
  the M10/M13/M14 pattern); this doc is the milestone's design reference.
- **`index.md` §4 Non-Goals** reconciled: "Multiplayer / networking" narrowed to the set
  this epic still excludes (lockstep/rollback, full server-rewind, competitive
  PvP/matchmaking/anti-cheat, peer-to-peer), pointing here for the direction taken.
- **`entity_model.md` §9 Non-Goals** reconciled: "Networked entity replication" replaced
  with the authoritative-replication direction; remaining non-goal narrowed to
  client-authoritative state and deterministic-lockstep replication.
- **`entity_model.md` §6 Ownership** annotated: snapshot application runs through a
  game-logic-owned apply step, preserving exclusive ownership.

Still deferred by design: the durable `networking.md` context/lib doc + its Agent Router
entry land at **Phase 1** promotion (the first phase with a real contract), not here.

## Rough sketch

**Crate topology.** A new library crate `crates/net/` (`postretro-net`) owns transport
+ replication + wire types, depended on by the `postretro` binary. It depends on
`renet`/`renetcode`, `bitcode`, and reuses serde derives on engine component types
(no new derives on the components themselves; the net crate defines the snapshot/delta
envelopes). Keeping it a sibling crate (not a `postretro` module) enforces the
boundary and is what makes the headless server entry point cheap. Engine-side glue —
where the frame loop calls into the net crate, where snapshots apply to the registry —
lives in `postretro`.

**Frame-loop integration.** Client per frame: send buffered input command(s) → apply
received snapshots to the registry (reconcile predicted pawn) → run the headless tick
for predicted entities → render. Server (host) per tick: drain client input commands →
run the headless tick → serialize delta snapshots per client → send. The headless
`simulate` seam from Phase 0 is the shared core; the listen-server host runs both
halves, a dedicated server runs only the server half. Four invariants this must hold:

- **Entity-ownership stays exclusive.** Snapshot application and predicted-entity
  spawn/despawn run through a **game-logic-owned apply step** — the net crate emits
  typed snapshots; engine-side game logic applies them and owns every spawn/despawn. The
  net crate never mutates the registry directly, so `entity_model.md` §6 still holds.
- **Server runs a sanctioned frame-order subset.** The dedicated/headless server runs
  Input→Game logic only (no Audio/Render/Present) — not a violation of the frame-order
  invariant, since the dropped stages are read-only consumers of game state.
- **No `RefCell` borrow across the network encode.** Serialization captures one **owned
  post-tick snapshot buffer** (single borrow, released before the per-client encode
  loop), from which all per-client deltas are computed.
- **Network runtime never blocks the event loop.** `renet` is **polled non-blocking**
  within the frame loop — its transport is synchronously pollable, no `tokio` — honoring
  `development_guide.md` §4.2 (winit owns the loop; never block it).

**Replication granularity.** Replicate the mutable tick subset of components, not
descriptor-immutable params (both sides load those from the same descriptor at level
load). `ComponentValue`'s existing `#[serde(tag = "kind")]` shape is the per-component
unit; the snapshot is a set of `(network_id, ComponentValue-delta)` plus
spawn/despawn events. Network-id ↔ local-`EntityId` mapping is a client side table
(`research.md` §6).

**Determinism substrate.** Movement prediction leans on both sides holding an
identical `CollisionWorld` (parry3d trimesh from the same PRL) and stepping the same
`movement::tick`. Divergence is bounded, measured (Phase 0), and corrected
(reconciliation), never assumed zero.

## Boundary inventory

Netcode is engine-internal: it crosses **Rust ↔ wire**, not into JS/Lua or FGD. The
framework-level names below are pinned here; **per-message field layouts are reserved
for the Phase 1 implementation spec** (state the constraint, not the byte layout).

| Name | Rust | Wire (bitcode/serde) | JS/TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Network entity id | `NetworkId(u32)` (net crate) | bitcode u32 | n/a | n/a | n/a |
| Local entity id | `EntityId` (`scripting/registry.rs`) | not sent raw; mapped to `NetworkId` | n/a | n/a | n/a |
| Component snapshot unit | `ComponentValue` | `#[serde(tag = "kind", rename_all = "snake_case")]` | n/a | n/a | n/a |
| Input command | mirrors `MovementInput` + fire/aim | bitcode struct | n/a | n/a | n/a |

Scripts never observe the wire; FGD never configures replication. If a future phase
exposes a server/listen-config KVP, it joins this inventory then.

## Wire format

Netcode adds a binary wire surface. **Epic-level invariants pinned now; per-message
layouts deferred to Phase 1.**

- **Encoding owner:** `bitcode` owns endianness and field packing — the wire is not a
  hand-rolled byte format. Snapshot/command structs derive serde; bitcode encodes.
- **Channel model (`renet`):** reliable-ordered for control (handshake, spawn/despawn,
  ack negotiation); unreliable-sequenced for snapshots (latest-wins, drop stale);
  the input-command stream is its own channel. Exact channel assignment per message
  type lands in Phase 1.
- **Protocol/version handshake:** a version/protocol stamp gates every connection;
  mismatch rejects with a logged reason and applies no state. Mirrors the existing
  `BakedIr`/persist version-stamp discipline (`scripting.md` §11).
- **Snapshot protocol:** delta against a per-client acked baseline; baseline advances
  on ack; a joiner with no baseline gets a full snapshot first. Empty-delta and
  spawn/despawn encoding are pinned in the Phase 1 spec.

## Open questions

Resolved as **decisions during the relevant phase's spec**, not blockers for this epic:

- **Reconciliation tolerance value** — set by the Phase 0 spike's measured divergence.
- **Network-id allocation** — server-assigned monotonic vs. recycled; and how
  predicted-spawn local ids reconcile to confirmed network ids (Phase 1 / Phase 4).
- **Interest management** — whether 16-player bandwidth needs per-client culling
  (likely portal/PVS-derived, reusing the existing visibility) or whether delta
  compression alone fits the budget (Phase 5, measured).
- **Tick-rate decoupling** — whether the network send rate equals the 60 Hz sim tick
  or a lower snapshot rate with client interpolation buffering (Phase 1).
- **Library versions / current APIs** — Phase 1 pins `renet`/`renetcode`/`bitcode`
  versions and confirms current channel APIs via Context7 at implementation time.
- **AI/enemy replication** — M10 enemies (`Agent` + AI brain) are server-authoritative
  and replicate as ordinary entities; any prediction of them is out of scope. M10 is a
  stated prerequisite (above); the combat-phase specs ground enemy-component
  serialization against merged M10 code.
