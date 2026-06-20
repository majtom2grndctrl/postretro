# Multiplayer Netcode (Epic)

> **Milestone 15 design reference** (`plans/roadmap.md`). Defines the networking model,
> stack, architectural seams, and an **eight-phase, horizontal** build order. Per-phase
> implementation specs are **reserved** — each gets its own `/draft-spec` → review →
> `/orchestrate` cycle as it opens. Rationale + codebase seam map: `research.md`.
> Verified crate landscape + named-pattern references: `crate-pattern-research.md`.

## Goal

Add **co-op multiplayer** to PostRetro: an authoritative client-server engine where
a host runs the world and up to 16 players share a campaign level. The model is
**authoritative server + snapshot replication** (the model CS2 / Valorant / Overwatch 2
ship today, not deterministic lockstep) — sidestepping the cross-architecture `f32`
determinism trap (`research.md` §1–2). This reverses the standing "Multiplayer /
networking" non-goal; it is a deliberate strategic direction, not incremental polish.

## Prerequisites

**M10 (animated enemies)** — co-op combat needs enemies to fight, and M10's `Agent` +
AI-brain components are what replicate as server-authoritative NPCs. M10 is in final
code review and lands before netcode begins. **Phases 0–3 are enemy-free** (extraction,
transport, replication, movement prediction) and depend on nothing in M10 — Phase 2's
on-the-wire test entity is a dumb AI-less path-walker. The **combat-bearing phases
(4–7)** build on M10's components; their per-phase specs are drafted against merged M10
code, so enemy-replication detail is grounded, not assumed.

## Scope

### In scope
- **Authoritative client-server** model: server (the host) owns world state; clients
  predict locally and reconcile toward server snapshots.
- **Listen-server architecture**, built to a seam that lets a headless **dedicated
  server** be split out later without re-architecting authority.
- **Up to 16 players**, co-op campaign first.
- **Transport + replication stack**: `renet` 2.0 + `renet_netcode` transport, hand-rolled
  replication (`lightyear` as design blueprint), `bitcode` serialization, custom
  per-entity snapshot delta. See **Stack & implementation references**.
- **Client-side prediction** for local-player **movement** (including dash) and
  **projectiles** (predicted-entity → server-confirmed handoff).
- **Favor-the-shooter hitscan** validation against a short single-entity history.
- A **headless simulation seam**: the fixed-tick game logic runs with no
  wgpu/winit dependency, so server and client share one tick path.
- The supporting essentials: **time-sync**, **snapshot ack + per-entity delta baseline**,
  **join-in-progress AND player-leave/disconnect**, a **latency-simulation harness**, and
  a **protocol/version handshake**.
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

## Build shape: horizontal, not a vertical slice

Phases are **horizontal layers**, each ending in a runnable, observable checkpoint with
a **crisp single-contract acceptance bar**. A vertical slice (one thin thread through
every subsystem at once) was rejected: it forces fuzzy "the whole thing kind of works"
AC, and for netcode the integration truth it buys is recoverable more cheaply by
**deciding the timing parameters up front** (snapshot rate, interpolation delay,
tick-clock mapping) and **speccing each layer's data structures contracts-first against
the next layer's stated needs**. The one genuinely empirical question — *does basic
predict/reconcile feel right* — lives in the **Phase 0 spike**, where fuzzy
measured-finding AC is correct (`experimental_spikes.md`), not smeared across feature
phases.

## Acceptance criteria

Observable, durable across implementation rewrites, each mapped to a phase. Phase 0
items split per `experimental_spikes.md` into an **honesty gate** (the seam runs
correctly) and **measured findings** (recorded numbers, not thresholds).

- [ ] The fixed-tick game logic advances one tick through the full order (transform
  snapshot → movement → weapon → death sweep) with **no wgpu/winit dependency**; a
  headless determinism test ticks N pawns from a recorded input stream and **stays green
  across runs**. (Phase 0, honesty gate)
- [ ] **Measured finding:** same-input cross-arch divergence over N ticks is recorded,
  feeding the Phase 3 reconciliation tolerance. (Phase 0)
- [ ] **Measured finding:** a throwaway predict/reconcile feel-prototype runs; a recorded
  read on whether basic reconciliation feels acceptable. (Phase 0 spike — fuzzy AC lives
  here by design)
- [ ] Two instances exchange a **versioned handshake**; a mismatched protocol/version is
  rejected with a logged reason and **no state applied**. A hand-built snapshot struct
  **round-trips over the bitcode wire**; a remote pawn appears and moves. (Phase 1)
- [ ] A server-authoritative entity on a fixed path **interpolates smoothly at 150 ms RTT
  + 5% loss + jitter** (the exit gate — bad network is the definition of done). (Phase 2)
- [ ] A client connecting after level start **converges** (baseline-then-delta); a client
  that **drops is cleaned up** (timeout + clean disconnect, slot freed); the client tick
  clock tracks the server within a stated bound. (Phase 2)
- [ ] The **local pawn responds to input with no perceptible delay** at 100 ms RTT; a
  corrected snapshot reconciles **without visible rubber-banding** under normal
  conditions, including a **mispredicted dash** (no snap-teleport under normal latency).
  (Phase 3)
- [ ] **Design milestone:** a playable co-op set-piece (real M10 enemies) where a
  scripted reveal/wave triggers correctly with 2+ players; trigger-ownership, co-op
  respawn policy, and **player-leave policy** documented; set-piece progress converges
  for a mid-piece joiner. Gates the combat phases. (Phase 4)
- [ ] A client's **hitscan shot registers damage on a moving remote enemy** with
  favor-the-shooter (short single-entity-history) tolerance; **HP changes only on server
  confirmation** while the client shows immediate cosmetic feedback. (Phase 5)
- [ ] A client-fired **projectile appears instantly** and its predicted entity **hands off
  to the server-confirmed entity** with no visible duplicate or pop under normal latency.
  (Phase 6)
- [ ] **16 players** on one listen-server host stay within a stated host-upstream
  **bandwidth budget**; the sim/host boundary exposes a **headless server entry point**
  (dedicated-server readiness). (Phase 7)

## Tasks

Each task is a **phase** with its own observable bar — the unit of the later per-phase
spec cycle. This epic does not break them into sub-tasks.

### Phase 0: Headless simulation seam + determinism harness + spike
Extract the fixed-tick game logic out of `main.rs`'s render-interleaved
`RedrawRequested` handler into a headless `simulate`-style seam that advances one tick
from `(registry, per-tick input, &CollisionWorld, dt)` with **no GPU/window dependency**
— the shared server+client tick path. Today the order (`snapshot_transforms` →
`run_movement_tick` → `run_weapon_fire_tick` → `run_death_sweep` → `push_state`) is
inlined in `main.rs` (5,593 lines); the per-component movement tick lives in
`movement/mod.rs` (6,055 lines), walked by `main.rs::run_movement_tick`.
**Split-before-extend** both files before adding the seam. **Write the determinism test
first** (recorded input → deterministic tick within tolerance) and do not call the phase
done until it is green and stays green — a leaky seam poisons reconciliation forever.
Then the **spike**: measure same-input cross-arch divergence (forced intermediate
rounding and/or a second CI architecture) to set the reconciliation tolerance, and stand
up a **throwaway predict/reconcile feel-prototype** to answer empirically whether basic
reconciliation feels acceptable. Deliverables: the extracted seam, the green determinism
test, the divergence number, the feel read. *Budget 2–3× the obvious estimate* — this is
untangling render/sim coupling across ~11.6k lines, not a file split.

### Phase 1: Transport + wire + handshake
Stand up `renet` 2.0 + `renet_netcode` transport in a new `crates/net/` sibling crate
(polled non-blocking in the frame loop — no tokio), the protocol/version handshake (a
mismatch rejects with a logged reason, applies no state), and the `bitcode` wire codec.
A hand-built snapshot struct round-trips end to end; two instances connect over
loopback/LAN and a remote pawn appears and moves (full-state, ugly-but-connected is the
bar — delta/interpolation are Phase 2). **Resolve the wire-enum encoding here** (see
Wire format): internally-tagged serde enums (`ComponentValue`) cannot deserialize on any
binary format, so the wire-bound component types carry native `bitcode::Encode/Decode`.
The latency-sim harness lands here as durable dev tooling (in-process packet conditioner
+ `tc netem` for soak; **not** turmoil — it only sees tokio sockets). `networking.md`
context/lib doc + its Agent Router entry land at this phase's promotion.

### Phase 2: Replication — delta/baseline/ack + time-sync + interpolation + lifecycle
The replication brain. The server serializes post-tick entity state as a set of
`(NetworkId, ComponentValue-delta)` plus spawn/despawn, delta-encoded against a
**per-entity acked baseline** (eventual-consistency state sync, lightyear-style — a
dropped packet re-sends only affected entities, not a global snapshot). The client maps
`NetworkId ↔ local EntityId` and applies through the game-logic-owned apply step. Folds
in **time-sync** (client tick clock ↔ server, the substrate prediction stands on),
**join-in-progress** (baseline-then-delta), and **player-leave/disconnect** (timeout +
clean disconnect, slot freed). Remote entities interpolate via the existing
previous/current Transform path with an **interpolation delay sized from measured
jitter**. Proves out on a **dumb AI-less server-authoritative mover** on the wire — which
de-risks both the set-piece fun-gate (Phase 4) and the moving-target combat (Phase 5)
without waiting on M10. **Exit gate: smooth at 150 ms RTT + 5% loss + jitter.**

### Phase 3: Local-player movement prediction + reconciliation
The client predicts its own pawn immediately from local input (the `MovementInput`
command — `wish_dir`, jump, dash edge, sprint, crouch intent, `facing_yaw`), buffering
inputs by tick (command frames). On each authoritative snapshot it reconciles: rewind to
the acked tick, re-apply buffered inputs, snap only past the Phase 0 tolerance.
**Reconciliation *smoothing* is the hard part, not the rewind-replay** — and dash (high
instantaneous velocity, edge-triggered) makes a mispredicted correction brutal to hide;
budget accordingly. Movement-only (dash included). Replication targets the **mutable tick
subset** of `PlayerMovementComponent`, never the descriptor params both sides hold. An
authoritative respawn (or any large position jump) reconciles as a **teleport** — snap,
do not interpolate, reusing `FrameTiming::hold_state` — the *mechanism*; the co-op
respawn *policy* is Phase 4's. May run in parallel with Phase 4.

### Phase 4: Co-op set-piece design milestone (gating)
**Design-first.** Resolve how the engine's first-class set-pieces — scripted reveals,
monster closets, triggers, waves — behave with N players: trigger ownership (any-player
vs. all-players vs. host), reveal/spawn fan-out, progress tracking, co-op death/respawn
**policy** (where/when players respawn, shared lives, spectate-on-death), and
**player-leave policy** (what happens to the level/set-piece when a player drops
mid-piece). Includes **set-piece-progress replication** so a mid-piece joiner converges.
Deliverable: a short design note **plus** one playable co-op set-piece proving the chosen
semantics with **real M10 enemies** (the Phase 2 dumb mover proved the wire; the fun-gate
needs real combat). **Gates** the combat phases — answers "is co-op fun" before Phases
5–6 commit. May run in parallel with Phase 3.

### Phase 5: Server-authoritative hitscan combat
Weapon fire becomes server-authoritative. The client sends a fire intent (render-rate
look sampled into the per-tick fire command at fire time) and shows immediate cosmetic
muzzle/impact feedback; the server runs `weapon::tick`/`fire_hitscan` and is the sole
authority on damage. **Favor-the-shooter** validation tests the shot against a **short
single-entity history** of the target — explicitly **not** full server-rewind. Tolerance
must absorb the render-vs-tick aim-quantization gap *and* the client-sees-enemies-in-the-
past interpolation gap. HP on all clients changes only on server confirmation.
Non-movement abilities with heavy world side-effects follow the same rule:
server-authoritative, cosmetic local feedback. (Whether to refine to CS2-style sub-tick
timestamping vs. tick-aligned rewind is a Phase 5 spec decision — see Open questions.)

### Phase 6: Predicted projectiles (predicted-entity → confirmed handoff)
Client-side predicted projectiles for weapon feel: a locally-fired rocket/grenade spawns
a **predicted entity** instantly; when the server's confirmed projectile arrives, the
client rebinds the predicted entity's network id to the confirmed one (the
predicted/confirmed/interpolated classification) with no visible duplicate or pop. The
bug-prone reconciliation/de-dup machinery, accepted deliberately for the weapon feel.
Playtest bar: "do projectiles feel crisp." Server stays authoritative on outcomes.

### Phase 7: Scale + dedicated-server readiness
Validate **16 players** on one listen-server host within a stated **host-upstream
bandwidth budget** (state-sync + priority-accumulator budgeting); add interest management
(reuse the portal/PVS visibility) only if the budget demands it. Prove the
**dedicated-server seam**: the host/sim boundary compiles and runs as a **headless server
entry point** (no client half), confirming the Phase 0 extraction delivered the
split-later promise.

## Sequencing

**Critical path:** `0 → 1 → 2 → (3 ‖ 4) → 5 → 6 → 7`. Eight phases, ~six deep, with a
concurrent middle band.

**Phase 0** blocks everything — nothing networks until the tick runs headless and the
divergence + feel premises are measured.
**Phase 1** consumes the Phase 0 seam; every later phase rides this wire.
**Phase 2** consumes Phase 1; the replication brain every gameplay phase reads.
**Phase 3** (movement prediction) consumes Phase 2 (snapshots to reconcile against) + the
Phase 0 tolerance. **Phase 4** (set-piece design) consumes Phase 2 (multi-client + the
dumb mover) + M10. **3 and 4 run concurrently** (merge-coordinate on the net-crate
boundary, the M13 D/F precedent); 3 owns the respawn *mechanism*, 4 the respawn *policy*.
**Phase 5** (hitscan) consumes Phase 3 (predicted pawn), Phase 2 (enemy replication), the
Phase 4 gate, and M10.
**Phase 6** (projectiles) consumes Phase 5 + Phase 3's prediction/reconciliation
machinery.
**Phase 7** (scale + dedicated) consumes the full stack.

### Promotion status (landed at Milestone 15 registration)
- **Registered as Milestone 15** in `context/plans/roadmap.md` (detail-on-open phases,
  the M10/M13/M14 pattern); this doc is the milestone's design reference.
- **`index.md` §4 Non-Goals** reconciled: "Multiplayer / networking" narrowed to the set
  this epic still excludes; co-op direction points here.
- **`entity_model.md` §9 Non-Goals** reconciled to the authoritative-replication
  direction; **§6 Ownership** annotated (snapshot apply runs through a game-logic-owned
  step, preserving exclusive ownership).
- **Crate landscape + named-pattern/reference currency** captured in
  `crate-pattern-research.md` (verified mid-2026).

Still deferred by design: the durable `networking.md` context/lib doc + its Agent Router
entry land at **Phase 1** promotion, not here.

## Stack & implementation references

Full verified detail (maturity, sources, gotchas) in `crate-pattern-research.md`.

**Depend, don't hand-roll:**

| Concern | Decision |
|---|---|
| UDP transport / connection / encryption | **renet 2.0 + renet_netcode** (Bevy-free since 2.0, sync frame-poll, no tokio) |
| Steam relay / P2P (Steam release) | **steamworks-rs** behind a feature flag |
| Reliability (seq / ack / fragmentation) | **renet channels** — no separate crate |
| Wire serialization | **bitcode** (compact bit-packing; pin the version, never persist its bytes) |

**Hand-roll** (no good non-Bevy crate fits the bespoke registry): replication,
prediction, interpolation, per-entity delta, float quantization, time-sync, the
latency-sim harness, interest management (reuse portal/PVS). `lightyear` is the **primary
blueprint** (Bevy-locked, not a dependency); `naia`'s core is a secondary non-Bevy
blueprint. **Avoid:** turmoil (tokio-only sim), bincode (unmaintained, RUSTSEC-2025-0141),
laminar / numquant / speedy / bitvec (abandoned).

**Named patterns → phase → contemporary anchor** (every hand-rolled part maps to a
documented technique; the classics remain canonical, paired with a current source):

| Pattern | Phase | Reference |
|---|---|---|
| Client prediction & server reconciliation | 3 | Gambetta; Valve Source; **lightyear book**; **Riot "Peeking into VALORANT's Netcode"** (2020) |
| Snapshot interpolation + interpolation delay | 2 | Fiedler; Gambetta Pt. III; **SnapNet "Netcode Architectures" Pt. 3** (2023) |
| Per-entity delta + eventual-consistency state sync | 2 | **lightyear** (component-level) — supersedes Quake-3 monolithic baseline |
| State sync + priority accumulator (bandwidth) | 7 | Fiedler "State Synchronization"; **lightyear** interest management |
| Input command buffering / command frames | 3 | **Overwatch GDC 2017** (still the reference talk); lightyear input handling |
| Predicted / confirmed / interpolated classification | 6 | **lightyear** (the modern formulation) |
| Favor-the-shooter, bounded short-history rewind | 5 | Valve Lag Compensation; **Riot Valorant** (victim-cost framing); CS2 sub-tick (footnote) |
| Float quantization / bounded serialization | 1 | Fiedler "Serialization Strategies" |
| Fixed timestep + ring buffers | 0 | Fiedler "Fix Your Timestep" (already in `frame_timing.rs`) |
| Bounded extrapolation on packet starvation | 2 | snapshot-interp refs above — **not** IEEE DIS dead reckoning (wrong fit for a corridor FPS; dropped) |

## Rough sketch

**Crate topology.** A new library crate `crates/net/` (`postretro-net`) owns transport +
replication + wire types, depended on by the `postretro` binary. It depends on `renet`
2.0 + `renet_netcode`, `bitcode`, and `steamworks-rs` (feature-flagged). It defines the
snapshot/delta/command envelopes. The wire-bound component types carry native
`bitcode::Encode/Decode` (serde derives stay for JSON/SDK/persistence — see Wire format).
The sibling-crate boundary (not a `postretro` module) is what makes the headless server
entry point cheap. Engine-side glue — frame-loop calls into the net crate, the
game-logic-owned snapshot apply — lives in `postretro`.

**Frame-loop integration.** Client per frame: send buffered input command(s) → apply
received snapshots to the registry (reconcile predicted pawn) → run the headless tick for
predicted entities → render. Server (host) per tick: drain client input commands → run
the headless tick → serialize per-entity delta snapshots per client → send. The headless
`simulate` seam from Phase 0 is the shared core; the listen-server host runs both halves,
a dedicated server runs only the server half. Four invariants:

- **Entity-ownership stays exclusive.** Snapshot application and predicted-entity
  spawn/despawn run through a **game-logic-owned apply step** — the net crate emits typed
  snapshots; engine-side game logic applies them and owns every spawn/despawn, so
  `entity_model.md` §6 holds.
- **Server runs a sanctioned frame-order subset.** Input→Game logic only (no
  Audio/Render/Present) — not a violation; the dropped stages are read-only consumers.
- **No `RefCell` borrow across the network encode.** Serialization captures one **owned
  post-tick snapshot buffer** (single borrow, released before the per-client encode loop).
- **Network runtime never blocks the event loop.** `renet` is **polled non-blocking** in
  the frame loop (sync transport, no tokio), honoring `development_guide.md` §4.2.

**Replication granularity.** Replicate the mutable tick subset of components, not
descriptor-immutable params (both sides load those from the descriptor). The snapshot is
a set of `(NetworkId, ComponentValue-delta)` plus spawn/despawn, delta-encoded against a
**per-entity acked baseline** (eventual consistency — each entity converges
independently). `NetworkId ↔ local EntityId` mapping is a client side table
(`research.md` §6).

**Determinism substrate.** Movement prediction leans on both sides holding an identical
`CollisionWorld` (parry3d trimesh from the same PRL) and stepping the same
`movement::tick`. Divergence is bounded, measured (Phase 0), and corrected
(reconciliation), never assumed zero.

## Boundary inventory

Netcode is engine-internal: it crosses **Rust ↔ wire**, not into JS/Lua or FGD.
Per-message field layouts are reserved for the Phase 1/2 specs (state the constraint, not
the byte layout).

| Name | Rust | Wire | JS/TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Network entity id | `NetworkId(u32)` (net crate) | bitcode u32 | n/a | n/a | n/a |
| Local entity id | `EntityId` (`scripting/registry.rs`) | not sent raw; mapped to `NetworkId` | n/a | n/a | n/a |
| Component snapshot unit | wire-mirror types in the net crate (`WireTransform`, …), converted from `ComponentValue` | **native `bitcode::Encode/Decode`** (NOT serde-tagged — see Wire format) | n/a | n/a | n/a |
| Input command | mirrors `MovementInput` + fire/aim | bitcode struct | n/a | n/a | n/a |

Scripts never observe the wire; FGD never configures replication.

## Wire format

Netcode adds a binary wire surface. **Epic-level invariants pinned now; per-message
layouts deferred to the Phase 1/2 specs.**

- **Encoding owner:** `bitcode` owns endianness and bit-packing. **Critical:** an
  internally-tagged serde enum (`#[serde(tag = "kind")]`, which `ComponentValue` is)
  **cannot deserialize on any non-self-describing binary format** — bitcode, postcard, and
  bincode all fail with `DeserializeAnyNotSupported`. The wire-bound component data
  therefore carries **native `#[derive(bitcode::Encode, bitcode::Decode)]`**; the engine
  `ComponentValue` keeps its serde derives for JSON/SDK/persistence. The Phase 1 spec
  resolves the wire side as **dedicated wire-mirror types in the net crate** (keeping the net
  crate `postretro`-free), converted at the `crate::netcode` boundary — not bitcode derives on
  `ComponentValue` itself. This amends the earlier "reuse serde derives, no new derives"
  assumption — the addition is bounded (the known component-type set) and buys best wire
  compactness.
- **bitcode version stability:** the format is unstable across major versions (by design).
  Fine for co-op (host + client ship together); **pin the version exactly, never persist
  bitcode bytes, gate every connection on the version handshake.**
- **Channel model (`renet`):** reliable-ordered for control (handshake, spawn/despawn);
  unreliable for snapshots (latest-wins); the input-command stream its own channel. Exact
  assignment lands in the Phase 1/2 specs.
- **Protocol/version handshake:** a version/protocol stamp gates every connection;
  mismatch rejects with a logged reason and applies no state. Mirrors the `BakedIr`/persist
  version-stamp discipline (`scripting.md` §11).
- **Snapshot protocol:** per-entity delta against a per-entity acked baseline (eventual
  consistency); a joiner with no baseline gets a full snapshot first.

## Open questions

Resolved as **decisions during the relevant phase's spec**, not blockers for this epic:

- **Reconciliation tolerance + smoothing curve** — tolerance set by the Phase 0 spike;
  the smoothing curve (esp. dash corrections) is empirical, tuned in Phase 3.
- **Snapshot send rate vs. 60 Hz sim tick** — decided **up front** at the Phase 1/2 specs
  (the no-slice discipline: timing parameters first), with client interpolation buffering.
- **Network-id allocation** — server-assigned monotonic vs. recycled; predicted-spawn →
  confirmed reconciliation (Phase 1 / Phase 6).
- **Hitreg precision** — tick-aligned rewind (simpler, Valorant-style) vs. CS2 sub-tick
  timestamping (exact shot moment). Phase 5 spec, after a prototype.
- **Interest management** — whether 16-player bandwidth needs per-client portal/PVS
  culling or delta compression alone fits (Phase 7, measured).
- **Wire-enum representation** — resolved: native `bitcode::Encode/Decode` on wire types
  (see Wire format). The Phase 1 spec pins the exact derive set.
- **AI/enemy replication** — M10 enemies replicate as ordinary entities; any prediction of
  them is out of scope. M10 is a stated prerequisite; combat-phase specs ground
  enemy-component serialization against merged M10 code.
