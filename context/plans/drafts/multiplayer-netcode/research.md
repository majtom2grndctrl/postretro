# Multiplayer Netcode — Research & Rationale

> Supporting analysis for the epic spec (`index.md`). Decisions live there; this
> file holds the investigation behind them and the codebase seam map. Not durable
> — consumed during planning, discarded at ship.

---

## 1. Model: authoritative client-server, snapshots over determinism

Two viable families for a real-time FPS:

| Family | Mechanism | Fatal cost here |
|--------|-----------|-----------------|
| Deterministic lockstep / rollback (GGPO lineage) | All peers run bit-identical sims; exchange inputs only | Requires bit-identical f32 across machines |
| Authoritative server + snapshot replication (Quake / Source / Overwatch lineage) | Server is sole source of truth; clients predict + reconcile toward server state | Bandwidth + interpolation latency (manageable) |

**Why not lockstep.** The simulation runs in `f32` end to end — `glam` math, `parry3d`
collision (`movement/mod.rs`, `collision/mod.rs`). Cross-architecture f32 is not
bit-identical: x86 SSE vs ARM NEON, FMA contraction differences, and divergent
transcendental implementations all break determinism. Lockstep desyncs the moment
two players run different hardware. Closing that gap means fixed-point math or
heroic determinism work across `glam` + `parry3d` — out of proportion to a co-op
shooter.

**Why snapshots.** The authoritative model *tolerates* float drift by construction:
the server's state is ground truth, and clients continuously reconcile toward it.
Small per-tick divergence between a client's prediction and the server's result is
corrected, not fatal. The genre is forgiving — co-op PvE against a shared
authoritative world does not need frame-perfect competitive fairness.

## 2. The cross-arch f32 question — what the Phase 0 spike measures

Snapshots tolerate drift, but *how much* drift sets the reconciliation tolerance:
the threshold past which a client's predicted pawn must be visibly snapped to the
server's position. Too tight → constant rubber-banding; too loose → players walk
through walls the server says they hit.

The Phase 0 spike measures same-input divergence directly: run the extracted
headless tick over a recorded input stream twice under conditions that force
divergence (forced intermediate rounding, or a second architecture in CI), and
record the position delta over N ticks. This is a **measured finding**
(`experimental_spikes.md`), not a pass/fail gate — the number feeds the
reconciliation-tolerance design in Phase 2, and confirms (or refutes) the
snapshots-over-determinism premise before the epic commits to it.

## 3. Stack rationale

| Layer | Choice | Why |
|-------|--------|-----|
| Transport | `renet` + `renetcode` | De-facto Rust game transport: reliable-ordered / unreliable / unreliable-sequenced channels over UDP, with `renetcode` providing the netcode.io-style encrypted connection handshake. Not Bevy-coupled. |
| Replication | Hand-rolled, `lightyear` as blueprint | `lightyear` is the most complete Rust reference for prediction / interpolation / snapshot replication — but it is built on `bevy_ecs`. PostRetro's registry is a bespoke generational-index store, not Bevy. We borrow lightyear's *structure* (predicted vs. confirmed entities, input buffering, snapshot interpolation), not its code. |
| Serialization | `bitcode` | Compact binary, serde-compatible. Components already derive `Serialize`/`Deserialize` and `ComponentValue` is `#[serde(tag = "kind")]`, so the replication layer reuses existing derives. Tighter on the wire than `bincode`/`postcard`. |
| Delta | Custom | Snapshot delta against an acked baseline is replication-specific; no off-the-shelf crate fits the bespoke registry. Small, owned, tunable. |

**Why not an all-in-one (lightyear, bevy_replicon, naia).** Every mature Rust
replication crate assumes `bevy_ecs` archetype storage and the Bevy schedule.
Adopting one means adopting Bevy or shimming the registry into a fake ECS — more
coupling and surface than a focused hand-rolled layer over `renet`. The registry,
fixed-tick loop, and component columns already supply the substrate these crates
reimplement.

## 4. Why the listen-server-first / dedicated-later shape

Co-op campaign is the product frame, so the host is a player — a listen server.
But the authority model is identical to a dedicated server's; the only difference
is whether the host also renders. Phase 0's extraction (a headless tick with no
wgpu/winit dependency) is precisely the seam that lets a dedicated server be split
out later by running the host half without the client half. Designing to that seam
now costs little; retrofitting it later is the expensive path.

## 5. Risk ledger (the four things most likely to bite)

| Risk | Why it bites | De-risk in plan |
|------|--------------|-----------------|
| Phase 0 extraction effort | The tick loop is interleaved with render inside `main.rs` (5,593 lines); `movement/mod.rs` is 6,055 lines. No headless seam exists today. | Phase 0 is sized as a real refactor, sequenced first, with a split-before-extend pass on the affected files. |
| Cross-arch reconciliation tuning | The whole snapshot model rests on drift being small and correctable. | Phase 0 spike measures it before Phase 2 depends on it. |
| Host-upstream bandwidth (listen server) | The host uploads N per-client snapshot streams on a home connection. | Phase 5 budgets it explicitly; delta compression + (if needed) interest management. |
| N-player set-piece *fun* | Monster-closet / scripted-reveal set-pieces are the product's first-class gameplay; their semantics with N players are undesigned. | Pulled forward as a gating design milestone (Phase 1.5) before combat phases commit. |

## 6. Codebase seam map (grounded)

Confirmed against source — the contracts the epic builds on.

### Fixed-tick loop
- `frame_timing.rs`: 60 Hz fixed tick (`TICK_DURATION = 16_667µs`); `begin_frame()` →
  `FrameTickResult { ticks, alpha, frame_dt }`. Accumulator clamps to 250 ms.
- The tick *runs* inside `main.rs`'s `RedrawRequested` handler, interleaved with
  render. Per tick: `registry.snapshot_transforms()` (order 0) → `run_movement_tick`
  → `run_weapon_fire_tick` → `run_death_sweep` → `frame_timing.push_state(...)`.
- **No headless seam.** Render-rate camera look is applied outside the tick loop;
  game-state mutation is tick-only.

### Entity registry
- `scripting/registry.rs` (1,328 lines). `EntityRegistry` (line 449) owned by
  `ScriptCtx` as `RefCell<EntityRegistry>`.
- `EntityId(u32)` = packed `index: u16` + `generation: u16`; 65,536 live-entity cap;
  generation retires a slot on overflow (despawned IDs never become valid again).
- `ComponentKind` (line 90, `#[repr(u16)]`): `Transform`, `PlayerMovement`, `Weapon`,
  `Health`, … `ComponentValue` (line 156) is `#[serde(tag = "kind", rename_all =
  "snake_case")]` — already serde round-trippable.
- `spawn` (610), `despawn` (644), `iter_with_kind` (571), `set_component`,
  `get_component`, `snapshot_transforms` (793, copies current→previous Transform for
  render interpolation).

### Movement / weapon / health tick
- `movement::tick` (`movement/mod.rs:1797`): `tick(registry, component, input:
  MovementInput, world: &CollisionWorld, dt) -> MovementEvents`.
- `MovementInput` (`movement/mod.rs:86`): `{ wish_dir: Vec2, jump_pressed,
  dash_pressed (edge), running, crouch_intent, facing_yaw: f32 }` — a compact,
  already-quantizable per-tick command. **This is the networked input command shape**
  for movement prediction.
- `PlayerMovementComponent` (`scripting/components/player_movement.rs:148`): mutable
  tick state (`position` via Transform, `velocity`, `is_grounded`, `air_ticks`,
  `movement_state`, ability budgets) *plus* descriptor-immutable params (capsule,
  ground/air/fall) and render-only fields (`view_feel`) and compiled IR
  (`dash_programs`). Replication targets the **mutable tick subset**, not the params
  both sides already hold from the descriptor, nor the render-only/IR fields.
- `weapon::tick` (`weapon/mod.rs:81`) → `fire_hitscan(...)`: ray from camera, resolves
  world-vs-entity hit + zone. Server-authoritative target for Phase 3. Weapon aim
  needs camera pitch (render-rate today) carried into the tick command.
- `CollisionWorld` (`collision/mod.rs:32`): parry3d trimesh from PRL static geometry.
  Identical on server and client once the same level loads — the shared deterministic
  substrate for movement.

### Serialization today
- Engine: `serde` + `serde_json` (scripting FFI, state persistence). `postcard` lives
  in the level-format/compiler path. `bitcode` and `renet`/`tokio` are **net-new**.
- Components carry serde derives already; the replication layer adds bitcode encoding,
  not new derives on the component types.

### Entity-id mapping (design consequence)
Server and client `EntityId`s do not coincide: client-predicted spawns get local IDs;
the server assigns its own. Replication needs a network-id ↔ local-`EntityId` mapping
on the client (a side table), resolved per Phase 1. Predicted-entity handoff (Phase 4)
rebinds a locally-predicted projectile's network id to the server-confirmed one.
