# E17-C — Trigger/Event and Script Command Surface

> **Status:** ready — reviewed (structural + implementability), awaiting implementation.
>
> **Epic:** 17 — Kinematic Geometry and Moving Platforms.
>
> **Sub-plan:** C. Depends on A (`context/plans/done/E17--kinematic-platform-foundation/`, done). E (doors/blocking) depends on C.

## Goal

Give authors two declarative ways to tie mover motion to level events, without any
per-tick script control: KVP-authored **trigger volumes** (touch/use) that fire
**closed-vocabulary mover commands** (`start`, `stop`, `reverse`, `goToPathNode`),
and the same commands bound to named events from data scripts. Rust owns tick
evaluation; scripts and KVPs only declare. All firing is server-authoritative and
funnels through one activation decision point, and trigger state lives in a
serializable component — so E18 co-op semantics (ownership, late-join restoration)
plug in as a policy check and a data restore, not a refactor.

## Scope

### In scope

- A brush entity classname `trigger_volume` in the FGD and compiler, extracted as an
  invisible AABB volume (fog_volume precedent — AABB only, no render/BSP geometry).
- A new engine-owned `TriggerVolume` component (`ComponentKind::TriggerVolume = 14`)
  holding the trigger's command config plus mutable armed/latched/rearm state. Serde-
  serializable; **not replicated this slice** (a non-wire component kind, like `Light`).
- A runtime AABB side-table keyed by `EntityId` for the touch/use volume test
  (mirrors `FogVolumeBridge`; geometry never lives in the `ComponentValue`).
- Touch and use activation, host-authoritative: a touch fires on a player capsule's
  rising-edge entry into the volume; a use fires on capsule overlap plus a Use-input
  rising edge. Use is **volume-overlap only** — no aim raycast this slice.
- A single activation gate — `evaluate_trigger_activation(state, activator) ->
  Fire | Suppress` — that every touch and use activation routes through. This slice's
  gate checks armed/latched/rearm only; it receives the activator player id and
  discards it (logs in dev). This is the E18 seam (see **E18 Seam**).
- Closed-vocabulary mover commands as data interpreted by Rust: `start`, `stop`,
  `reverse`, `go_to_path_node`, with the durable phase semantics in **Command Semantics**.
- Driver extension for targeted move-and-hold: a replicated `target_segment` phase
  field plus static waypoint-name resolution on the mover component, so
  `goToPathNode(name)` is deterministic and reconciles on clients.
- A shared Rust command applier (`apply_mover_command`) both authoring paths call, so a
  KVP trigger and a script reaction produce identical mover-phase mutation.
- Script authoring path: register `kinematic_mover` as a `world.query` component kind
  (the E17-A seam), an SDK mover handle wrapper exposing `start`/`stop`/`reverse`/
  `goToPathNode`, and four tag-targeted reaction primitives (`moverStart`, `moverStop`,
  `moverReverse`, `moverGoToPathNode`) in the Rust reaction registry.
- Networking: replicate the mover `target_segment` field, and carry a `use_pressed`
  input bit client→host so co-op use triggers fire on the host. One `SNAPSHOT_VERSION`
  and protocol-gate bump covers both.
- A dev map wiring a trigger to a mover, exercised in single-player and over the
  in-memory net harness.

### Out of scope — fenced to E18 (Co-op Set-Pieces)

E18 owns co-op semantics and *consumes* this machinery; C ships the seam, not the policy.

- **Trigger ownership policy.** Which player's activation counts, per-player vs shared
  triggers, activation fan-out. C's gate takes the activator and ignores it; E18 makes
  the gate a pluggable ownership check.
- **Late-join restoration** of trigger/mover state. C keeps trigger state in a serde
  component so E18 restores it as data; C ships no `COMPONENT_KIND_TRIGGER_STATE` wire
  mirror. E18 adds the wire mirror the standard way (as A did for `KinematicMover`).
- **Reveal/spawn fan-out** (monster closets), shared progress, objective tracking,
  respawn / player-leave trigger policy.
- **Aim-raycast interaction** ("look at and press use"). C's use is capsule-overlap only.

### Out of scope — other E17 sub-plans

- Crush/blocking on trigger-driven movers. C keeps A's displace-only push; blocking,
  crush, interruption, and door audio are E17-E (doors).
- Rotation/orientation commands. Angular movers are E17-D.
- Per-tick script control of motion. Commands are declarative; Rust owns every tick.
- Visibility/portal effects of triggered movers (E17-F).
- Rendering: triggers are invisible; movers already render (A/B). No renderer work.

## Command Semantics

Durable contract — a mover command is pure mutation of the deterministic driver phase
(`segment_index`, `direction_sign`, `segment_elapsed_ms`, `wait_remaining_ms`,
`current_linear_velocity`, `started`, `completed`, `target_segment`). Each verb is
total and idempotent-safe. E18 will not change these.

| Verb | Effect on phase | Edge cases |
|---|---|---|
| `start` | `started = true`; `wait_remaining_ms = 0` (cancels a pending endpoint wait so motion resumes immediately). Resumes from current `segment_index`/`segment_elapsed_ms` in the current direction. | No-op if `completed` (a finished `once` mover has no remaining path — use `reverse`). No-op if already moving. |
| `stop` | `started = false`; `current_linear_velocity = ZERO`. Freezes at the current pose and phase; `segment_elapsed_ms` preserved so a later `start` resumes mid-segment. | No-op if already stopped. |
| `reverse` | Flips `direction_sign`; re-anchors within the current segment so the mover retraces from its **exact current position** toward the previous waypoint (`segment_elapsed_ms = segment_duration − segment_elapsed_ms`). Sets `started = true`, `completed = false`, `wait_remaining_ms = 0`. | No teleport — the position is continuous across the reversal. `once`/`ping_pong` endpoint rules then apply in the new direction. |
| `go_to_path_node(node)` | Resolves `node` (a `kinematic_waypoint` name) to a segment index on the target mover; sets `target_segment` to it, points `direction_sign` toward it, sets `started = true`, clears `completed`. The driver advances toward `target_segment` and holds on arrival (endpoint-wait then idle, like a `once` endpoint), clearing `target_segment`. | No-op + `log::warn!` if the name is not in the mover's chain. No-op if already at the node. `target_segment` supersedes `once`/`ping_pong` endpoint reversal until reached. |

A command applies only to entities carrying `KinematicMover`; the applier skips non-mover
targets with a rate-limited warning (mirroring `applyDamage` skipping non-Health targets).

`go_to_path_node` is names-only by design, not an open question: waypoint names are how
modders address path nodes via `command_arg` on a `trigger_volume` KVP, and `world.query` +
the mover handle already cover programmatic script use. A node-index overload would leak
the compiler's internal resolved-segment order into the authoring contract and is brittle
under waypoint reordering, where names are not; it stays deferred as a purely additive
option (the driver already resolves names to an internal `target_segment` index).

## Boundary Inventory

| Name | Rust | PRL / wire / serde | TypeScript / Luau | FGD |
|---|---|---|---|---|
| trigger entity | `TriggerVolumeComponent`, `ComponentKind::TriggerVolume = 14` | PRL `TriggerVolumeRecord`; serde `kind = "trigger_volume"`; **no wire payload this slice** | Not queryable this slice (E18 may expose) | `trigger_volume` |
| trigger AABB | runtime side-table keyed by `EntityId` (fog_volume pattern) | PRL `aabb_min`/`aabb_max: [f32;3]` | none | brush hull |
| activation kind | `TriggerActivation` (`Touch`/`Use`) | PRL/serde `activation: u8` (`touch=0`, `use=1`) | none | `activation` (choices) |
| target tag | `target_tag: String` | `target_tag` | matches mover `_tags` | `target_tag` |
| command verb | `MoverCommand` (`Start`/`Stop`/`Reverse`/`GoToPathNode`) | PRL/serde `command: u8` | `start`/`stop`/`reverse`/`goToPathNode` handle methods | `command` (choices) |
| command arg | `command_arg: String` (waypoint name for go-to) | `command_arg` (empty = none) | `goToPathNode(node)` arg | `command_arg` |
| fire mode | `TriggerFireMode` (`Once`/`Multiple`) | `fire_mode: u8` (`once=0`, `multiple=1`) | none | `fire_mode` (choices) |
| rearm | `rearm_ms: f32` (static); `rearm_remaining_ms: f32` (state) | `rearm_ms` | none | `rearm_ms` |
| enable-on-spawn | `enabled_on_spawn: bool` → seeds `armed` | `enabled_on_spawn` (single byte) | none | `enabled_on_spawn` (choices 0/1) |
| trigger state | `armed`/`latched`/`rearm_remaining_ms` on the component (serde) | serde only; **not on the wire** (E18 seam) | none | n/a |
| mover query kind | `QueryFilter::KinematicMover`; `WorldQueryComponent` variant | n/a | `world.query({ component: "kinematic_mover", tag })` (snake literal, matching `fog_volume`) | n/a |
| mover handle | `collect_kinematic_mover_handles_json` → `MoverEntity` handle | n/a | `MoverEntityHandle` (`sdk/lib/entities/movers.{ts,luau}`) — SDK wrapper around the Rust `MoverEntity` handle; the name split is intentional Rust-vs-SDK layering, not drift | n/a |
| mover target node | `KinematicMoverComponent.target_segment: Option<u16>` (phase, replicated); `waypoint_names: Vec<String>` (static, not replicated) | wire `WireKinematicMoverState.target_segment: Option<u16>` | via `goToPathNode` | n/a |
| use input | `Action::Use` consumed by trigger system | wire `WireMovementInput.use_pressed: bool` | n/a | n/a |

## Wire Format

Two `bitcode` deltas on existing types; no new snapshot record kind, no new PRL
section for the wire. One `SNAPSHOT_VERSION` bump (8 → 9) and one protocol-gate bump
cover both. The trigger component is **not** replicated this slice — no
`COMPONENT_KIND_TRIGGER_STATE` constant is added (a non-replicated kind, like `Light`).

- **`WireKinematicMoverState`** gains `target_segment: Option<u16>` (bitcode encodes the
  `Option`; endianness bitcode-internal). Append in declaration order after `velocity`.
  `all_finite`/validation is unchanged (integers only); update raw↔typed conversion,
  baseline/delta, and drift/round-trip tests. The client needs it to predict a
  `goToPathNode` hold — without it a predicted mover overshoots the target node.
- **`WireMovementInput`** gains `use_pressed: bool`. Update its `bitcode` encode/decode,
  raw↔typed conversion, and the `InputCommand` round-trip tests. In co-op the host reads
  `Action::Use` directly for its own local player and the replicated `use_pressed` edge
  only for remote players; a client reads its local `Action::Use` and sends
  `use_pressed` to the host.
- Bump `SNAPSHOT_VERSION` 8 → 9 and the transport wire-version/`protocol_id` gate (both
  are existing-type layout changes — see networking.md's two-gate handshake). Adding
  `TriggerVolume = 14` extends the engine/net `component_kind_discriminant` map and its
  drift test additively, the same way the non-replicated `Light` kind is already covered
  — but no new *replicated* kind enters the wire/replicated set; no net-side wire
  constant is added for it.

### PRL `TriggerVolumeSection` (`SectionId::TriggerVolumes = 44`)

Next free id after `KinematicGeometry = 43`. Extend the hand-written
`SectionId::from_u32` match. Mirror `KinematicGeometrySection` / `MapEntitySection`
encoding; emit only when the map has triggers.

- Little-endian throughout; recorded in the PRL table-of-contents like any section.
  `version` starts at 1. Body self-contained.
- Each list is a `u32` count before its entries (`triggers`, per-trigger `tags`); empty
  = `u32(0)`.
- Each `String` (`name`, `target_tag`, `command_arg`, tag entries) is a `u32` byte-length
  prefix + raw UTF-8, no terminator; empty = `u32(0)`. Absent `command_arg` = empty string.
- `activation`, `command`, `fire_mode` are each a `u8`; `enabled_on_spawn` a single
  `0`/`1` byte (mirror `AlphaLightsSection` / `KinematicGeometrySection`).
- `aabb_min`/`aabb_max` are `[f32; 3]` each; `rearm_ms` an `f32`.

`TriggerVolumeRecord { name, tags, aabb_min, aabb_max, activation, target_tag, command, command_arg, fire_mode, rearm_ms, enabled_on_spawn }`.

## Acceptance Criteria

- [ ] `sdk/TrenchBroom/postretro.fgd` includes `@SolidClass = trigger_volume` with keys
  `activation`, `target_tag`, `command`, `command_arg`, `fire_mode`, `rearm_ms`,
  `enabled_on_spawn`, `_tags` (inspection gate — no automated FGD test exists).
  `activation`, `command`, and `fire_mode` are FGD `choices` enums (TrenchBroom-side
  constraint); `prl-build` still validates them (AC 3) for maps authored outside
  TrenchBroom.
- [ ] `prl-build` compiles a map with one `trigger_volume` brush into a PRL
  `TriggerVolumes` section (id 44). The trigger brush is absent from static geometry,
  static BVH, static collision, portals, lightmap/SDF occluder bakes, and navmesh
  (fog_volume-style natural absence; add a regression test).
- [ ] `prl-build` rejects a `trigger_volume` with an unknown `command`, a
  `go_to_path_node` command missing `command_arg`, or a non-finite/negative `rearm_ms`,
  each with a diagnostic.
- [ ] The runtime spawns one trigger entity per record with a `TriggerVolume` component
  (kind 14) and a runtime AABB; the component's armed/latched/rearm state round-trips
  through serde. Trigger-less and mover-less maps load and behave unchanged.
- [ ] A `touch` trigger fires its command when a player capsule enters its volume
  (rising edge). A `fire_mode = once` trigger fires exactly once; a `multiple` trigger
  re-fires only after `rearm_ms` elapses; an `enabled_on_spawn = 0` trigger stays inert
  this slice (never fires) — verified in the deterministic sim. Enabling a disabled
  trigger at runtime is E18.
- [ ] A `use` trigger fires only when a player capsule overlaps the volume **and** a Use
  rising edge occurs the same tick; capsule overlap alone does not fire it.
- [ ] Every touch and use activation reaches command dispatch only through
  `evaluate_trigger_activation`. A `#[cfg(test)]` activation counter, incremented only
  inside `evaluate_trigger_activation`, equals the total number of fires observed across
  both touch and use activations (the runnable form of "sole call path"), and the gate
  receives the activator id.
- [ ] The four mover commands produce the **Command Semantics** phase effects,
  verified deterministically: `start` resumes from current phase and no-ops when
  completed; `stop` freezes without losing mid-segment progress; `reverse` retraces from
  the exact current position with no teleport; `goToPathNode(name)` moves to the named
  waypoint and holds; an unknown node name is a no-op — `target_segment` and phase stay
  unchanged (the asserted metric); the `log::warn!` is review-only, not a harness assertion.
- [ ] A command targeting a tag applies to every tagged `KinematicMover` (its phase
  mutates) and leaves tagged non-mover entities untouched (the asserted metric); the skip
  `log::warn!` is review-only, not a harness assertion.
- [ ] Script path: `world.query({ component: "kinematic_mover", tag })` returns mover
  handles; `handle.start()` / `.stop()` / `.reverse()` / `.goToPathNode(node)` build
  reaction descriptors that, when the named reaction fires, apply the verb to the tagged
  movers — producing the same mover-phase result as an equivalent KVP trigger.
- [ ] `postretro.d.ts` / `postretro.d.luau` regenerate with `kinematic_mover` in the
  `WorldQueryComponent` union and the four mover-command primitives; the typedef drift
  test passes.
- [ ] Co-op (deterministic net harness at the E15 profile — `LinkConfig { delay: 45,
  jitter: 60, loss_probability: 0.05 }` + fixed seed): only the host evaluates triggers;
  a client never fires one; a host trigger firing (including a `goToPathNode` that sets
  `target_segment`) replicates and the client's predicted mover reconciles to the new
  motion within `MOVER_TOLERANCE_M` (the bound A established), with no accumulating drift
  (`assert_non_accumulating`); a client's Use press reaches the host via `use_pressed` and
  fires a use trigger there. This requires `LoopbackHarness::host_tick` to invoke the Task 3
  trigger system each host tick (it calls `run_kinematic_mover_tick` directly, not
  `simulate_tick`, so the system is not run otherwise), a host-side trigger loaded in the
  harness setup targeting the mover, and `step()`/`SimCommand` injecting a per-player Use
  edge that becomes `use_pressed` on the client→host path; reuse the fixtures'
  `mover_position_error` readout.
- [ ] `SNAPSHOT_VERSION` is bumped 8 → 9 and the protocol gate bumped; wire drift/round-
  trip guards pass; a peer on the old version is refused at the handshake.
- [ ] No trigger state crosses the wire this slice (grep/review gate — no
  `COMPONENT_KIND_TRIGGER_STATE`); the E18 seam is data-only.
- [ ] No new `unsafe`; no non-renderer module imports `wgpu`; no renderer changes.

## Tasks

### Task 1: Mover command vocabulary and driver move-and-hold

Add the closed command vocabulary and the shared applier, in-memory, no triggers or wire
yet. Define `MoverCommand { Start, Stop, Reverse, GoToPathNode(String) }` and
`apply_mover_command(mover: &mut KinematicMoverComponent, cmd: &MoverCommand)` in the
mover crate/module (beside `crates/entities/src/components/kinematic_mover.rs` for the
type; the applier co-locates with the driver in `crates/postretro/src/kinematic_mover.rs`).
Implement these four verbs exactly — pure phase mutation of the deterministic driver phase
(`segment_index`, `direction_sign`, `segment_elapsed_ms`, `wait_remaining_ms`,
`current_linear_velocity`, `started`, `completed`, `target_segment`), no wall-clock, no
RNG; each verb is total and idempotent-safe:

| Verb | Effect on phase | Edge cases |
|---|---|---|
| `start` | `started = true`; `wait_remaining_ms = 0` (cancels a pending endpoint wait so motion resumes immediately). Resumes from current `segment_index`/`segment_elapsed_ms` in the current direction. | No-op if `completed` (a finished `once` mover has no remaining path — use `reverse`). No-op if already moving. |
| `stop` | `started = false`; `current_linear_velocity = ZERO`. Freezes at the current pose and phase; `segment_elapsed_ms` preserved so a later `start` resumes mid-segment. | No-op if already stopped. |
| `reverse` | Flips `direction_sign`; re-anchors within the current segment so the mover retraces from its **exact current position** toward the previous waypoint (`segment_elapsed_ms = segment_duration − segment_elapsed_ms`). Sets `started = true`, `completed = false`, `wait_remaining_ms = 0`. | No teleport — the position is continuous across the reversal. `once`/`ping_pong` endpoint rules then apply in the new direction. |
| `go_to_path_node(node)` | Resolves `node` (a `kinematic_waypoint` name) to a segment index on the target mover; sets `target_segment` to it, points `direction_sign` toward it, sets `started = true`, clears `completed`. The driver advances toward `target_segment` and holds on arrival (endpoint-wait then idle, like a `once` endpoint), clearing `target_segment`. | No-op + `log::warn!` if the name is not in the mover's chain. No-op if already at the node. `target_segment` supersedes `once`/`ping_pong` endpoint reversal until reached. |

The applier skips non-`KinematicMover` targets with a rate-limited warning (mirrors
`applyDamage` skipping non-`Health`). `MoverCommand` derives `Serialize, Deserialize,
Clone, PartialEq` — matching `KinematicMoverMode`'s serde-derive pattern in
`crates/entities/src/components/kinematic_mover.rs` (which also derives `Copy, Eq`; the
`GoToPathNode(String)` payload precludes those two here) — because Task 2 embeds it in the
serde `TriggerVolumeComponent` (AC 4 round-trip) and the PRL record.

Extend `KinematicMoverComponent` with `target_segment: Option<u16>` (phase) and
`waypoint_names: Vec<String>` (static, seeded at construction alongside `waypoints`; not
replicated). Extend the deterministic driver so that when `target_segment` is `Some`, the
mover advances toward it and holds on arrival (endpoint wait then idle), clearing
`target_segment`; `target_segment` supersedes `once`/`ping_pong` endpoint reversal until
reached. The `target_segment` handling is woven into the segment-stepping and
arrival/reversal helpers `advance_mover` / `handle_arrival_at_waypoint`
(`crates/postretro/src/kinematic_mover.rs` ~lines 139–243), where segments advance and
endpoints reverse — that is where the move-and-hold logic lives. The per-tick entry point
`run_kinematic_mover_tick` calls the single-tick integrator `advance_mover_phase_one_tick`,
which in turn calls `advance_mover` and computes velocity; those two names identify the
tick entry vs the private helper only — they do **not** mean the new logic belongs in
`advance_mover_phase_one_tick`. `go_to_path_node` resolves the node name against
`waypoint_names` to an index; unknown names warn and no-op. The driver stays a pure
function of {phase, static path, dt} so host and client reproduce it identically.

Adding `waypoint_names` means `KinematicMoverComponent::new`
(`crates/entities/src/components/kinematic_mover.rs` ~line 39; current args `mover_id`,
`waypoints`, `speed_mps`, `wait_ms`, `mode`, `started`) gains a names argument; update all
call sites (loader and test fixtures). Thread the names from the KinematicGeometry PRL load
site — `spawn_from_geometry` in `crates/postretro/src/runtime_movers.rs` (~line 201) already
holds them: `geometry.waypoints` are `LoadedKinematicWaypoint` with a `.name`, currently
dropped when `resolve_waypoint_chain` collapses the chain to `Vec<Vec3>`; carry the resolved
names alongside the positions into `new`.

Tests, all in-memory: each verb's phase effect (start/stop/reverse idempotence and edge
cases; reverse continuity — position before and after reversal is equal within ε and the
next step heads toward the previous waypoint); `goToPathNode` move-and-hold to a named
node and no-op on unknown name; driver determinism with `target_segment` set (same seed,
same trajectory); the tag applier skipping non-mover targets.

### Task 2: TriggerVolume component, PRL section, FGD, compiler, and load

Add `ComponentKind::TriggerVolume = 14` in `crates/entities/src/registry.rs` (after the
current last variant `KinematicMover = 13`), and a
`TriggerVolumeComponent` in `crates/entities/src/components/`. Update `ComponentKind::COUNT`,
`ComponentValue`, registry storage, serde, the `component_kind_name` reverse map
(`crates/entities/src/ffi.rs`), and the `component_kind_discriminant` map + drift test
(`crates/postretro/src/netcode/mod.rs`) — **no** net-side constant (kind 14 is not
replicated this slice, like `Light`). `component_kind_from_name` simply has no
`trigger_volume`/`kinematic_mover` arm (both stay unmapped there — neither is
script-attachable; `kinematic_mover` remains query-only, per Task 4); the actual hard
rejection of `kinematic_mover` is an explicit arm in the FromJs/FromLua `setComponent`
match (`crates/entities/src/ffi.rs` ~lines 246 and 401), which must stay. The component
carries static config
{ `activation`, `target_tag`, `command: MoverCommand`, `fire_mode`, `rearm_ms`,
`enabled_on_spawn` } and mutable state { `armed`, `latched`, `rearm_remaining_ms` };
`armed` seeds from `enabled_on_spawn`.

Add `SectionId::TriggerVolumes = 44` to `postretro-level-format` and a
`TriggerVolumeRecord`. Encoding (little-endian throughout, recorded in the PRL
table-of-contents like any section; `version` starts at 1; body self-contained; mirror
`KinematicGeometrySection` / `MapEntitySection`; emit only when the map has triggers):

- `SectionId::TriggerVolumes = 44` — next free id after `KinematicGeometry = 43`; extend
  the hand-written `SectionId::from_u32` match with the new arm.
- Each list is a `u32` count before its entries (`triggers`, and per-trigger `tags`);
  empty = `u32(0)`.
- Each `String` (`name`, `target_tag`, `command_arg`, tag entries) is a `u32` byte-length
  prefix + raw UTF-8, no terminator; empty = `u32(0)`; absent `command_arg` = empty string.
- `activation`, `command`, `fire_mode` are each a `u8`; `enabled_on_spawn` a single
  `0`/`1` byte (mirror `AlphaLightsSection` / `KinematicGeometrySection`).
- `aabb_min`/`aabb_max` are `[f32; 3]` each; `rearm_ms` an `f32`.

Field order: `TriggerVolumeRecord { name, tags, aabb_min, aabb_max, activation, target_tag,
command, command_arg, fire_mode, rearm_ms, enabled_on_spawn }`. Serialization tests. Add
the FGD `@SolidClass = trigger_volume` (choices for
`activation`/`command`/`fire_mode`/`enabled_on_spawn`). Compiler: collect
`trigger_volume` brush entities in `parse.rs` before the brush-entity skip path, compute
their AABB (fog_volume precedent — AABB only, **not** the mover's textured projection),
source `tags` from `_tags`, validate `command`/`command_arg`/`rearm_ms`, and pack the
section in `pack.rs`, emitting it only when the map has triggers. Assert the trigger
brush is absent from `world_brush_ids` / `brush_volumes` and the static `GeometrySection`.

Load section 44 into `LevelWorld` (`SectionId` dispatch during load is owned by
`crates/level-loader/src/prl_loader.rs`; absent/empty = no triggers). At level load, spawn
one entity per record with a `Transform`, the `TriggerVolumeComponent` (seeded), the
record's `tags`, and a runtime AABB in a trigger side-table named `TriggerVolumeBridge`,
keyed by `EntityId` (mirror `FogVolumeBridge`; Task 3 reads trigger AABBs from
`TriggerVolumeBridge`). Both host and client spawn triggers locally; triggers are **not**
registered in `ReplicableSet`.

### Task 3: Host-authoritative trigger system and single firing gate

Add a fixed-tick trigger system that runs after player movement settles (a new
update-order stage after Player movement tick, before AI brain tick — see
entity_model.md §5). Each tick, for each trigger, the system computes activation against
authoritative player state: `touch` = a player capsule's rising-edge entry into the
trigger AABB (read from Task 2's `TriggerVolumeBridge` side-table, keyed by `EntityId`;
track prior-overlap per (trigger, player) pair, so each player's rising edge is detected
independently — required for correct co-op behavior with multiple players); `use` =
capsule overlap plus a Use rising edge for that player.

Consume each player's Use edge through an explicit per-player seam — a map keyed by
`PlayerId` (see gate signature below) holding the current Use rising-edge state per
player — never a direct single-player `Action::Use` read inlined into the trigger logic.
In this phase (Phase 3) only the local player's `Action::Use` edge is wired into that map;
Task 5 (Phase 4) later fills remote-player entries from the replicated per-player
`use_pressed`. This keeps Task 3 from hard-coding a single-player read that Task 5 would
have to refactor: single-player and the host's own local player supply their edge locally,
every remote player's edge arrives via `use_pressed`, but the trigger system reads only the
`PlayerId`-keyed map either way.

On activation, call the sole gate
`evaluate_trigger_activation(&TriggerVolumeComponent, activator: PlayerId) -> Fire |
Suppress`, which this slice decides on armed/latched/rearm only and logs the discarded
activator in dev builds. Introduce `PlayerId` here as a thin alias/newtype over the
existing per-client input key: the host already keys remote client input by `client_id:
u64` (`HostCommandQueues.clients` in `crates/postretro/src/netcode/command_queue.rs`, with
`PawnOwnerMap` mapping pawn `EntityId → client_id`); single-player's only identity is
`registry.local_player_pawn()` (an `EntityId`), which maps onto a `PlayerId` too. Do **not**
invent a heavyweight new identity system — real ownership is E18; this only names the seam.
The **same** `PlayerId` is the key for (a) the per-(trigger, player) overlap tracking above,
(b) the per-player Use seam map, and (c) Task 5's per-player `use_pressed` map — one type,
shared across Task 3 and Task 5. On `Fire`: resolve `target_tag` to entities
(`query_by_component_and_tag`), call Task 1's `apply_mover_command` on each
`KinematicMover` target, then update trigger state — `latched = true` for `once`,
`rearm_remaining_ms = rearm_ms` for `multiple`. Count down `rearm_remaining_ms` each tick.
Enforce that both activation paths funnel through the one gate (no direct dispatch). For a
runnable single-call-path assertion, add a `#[cfg(test)]` activation counter incremented
only inside `evaluate_trigger_activation`; a test asserts it equals the total number of
fires observed across both touch and use activations — making "sole call path" and "gate
receives the activator id" runnable assertions rather than code-review claims.

Trigger firing and command application are server-authoritative: on a client the trigger
system and applier are inert; mover phase changes arrive via replication. Wire the system
into `simulate_tick` (call-site only in `main.rs`/`sim`; logic in a focused module).

Tests (deterministic sim): touch rising-edge fire; `once` fires once; `multiple` rearm
gating; `enabled_on_spawn = 0` suppression; use requires overlap + press edge; the
single-gate assertion (via the `#[cfg(test)]` activation counter); end-to-end trigger →
mover command → observed motion.

### Task 4: Script authoring path — world.query mover kind, handle, reaction primitives

Register `kinematic_mover` as a `world.query` component kind: add the
`WorldQueryComponent` enum variant (`crates/lighting/src/script_primitives.rs`, snake
literal `kinematic_mover` matching `fog_volume`), a `QueryFilter::KinematicMover { tag }`
arm and `collect_kinematic_mover_handles_json` in `crates/postretro/src/scripting/entity_world_primitives.rs`,
and a `MoverEntity` handle type (id, position, tags). Registering the `world.query` kind
also means extending `parse_query_filter` and the `WORLD_QUERY_DOC` literal list
(`crates/postretro/src/scripting/entity_world_primitives.rs` ~lines 72 and 88), not just
adding the enum variant. Query-ability and attach-ability are separate mechanisms: this
registration makes `kinematic_mover` readable via `world.query`, but the existing hard
rejection of `kinematic_mover` — an explicit arm in the FromJs/FromLua `setComponent`
match (`crates/entities/src/ffi.rs` ~lines 246 and 401), **not** in
`component_kind_from_name` (which simply has no `kinematic_mover` arm) — must stay in
place, so scripts may read mover handles but must not be able to spawn/attach a raw
`KinematicMover` component. Add
`sdk/lib/entities/movers.{ts,luau}` — a `MoverEntityHandle` wrapper whose `start()`,
`stop()`, `reverse()`, and `goToPathNode(node)` build reaction step descriptors (the
`SequenceStep { id, primitive, args }` shape lights/fog use), delegated to from
`sdk/lib/world.{ts,luau}` when `component: "kinematic_mover"`.

Register four tag-targeted reaction primitives — `moverStart`, `moverStop`,
`moverReverse`, `moverGoToPathNode` — in the `ReactionPrimitiveRegistry` via a
`register_mover_reaction_primitives` registrar co-located with the mover module and wired
into the aggregator (`crates/postretro/src/scripting/reactions/registry.rs`). Each handler
receives `(&mut EntityRegistry, &[EntityId], &Value)`, filters to `KinematicMover`
targets, and calls Task 1's `apply_mover_command` — the identical applier the KVP trigger
uses, so both paths converge. `moverGoToPathNode` reads its node name from the args.
Regenerate `postretro.d.ts`/`.d.luau`; the drift test must pass.

### Task 5: Networking — mover target replication and use input

Extend `postretro-net` and `postretro` replication per **Wire Format**: add
`target_segment: Option<u16>` to `WireKinematicMoverState` and `use_pressed: bool` to
`WireMovementInput`, updating `bitcode` encode/decode, raw↔typed conversion,
baseline/delta, `InputCommand` round-trip, and drift/round-trip tests. Bump
`SNAPSHOT_VERSION` 8 → 9 and the transport protocol gate — `WIRE_VERSION` (currently `8`)
in `crates/net/src/transport.rs` (~line 52), which `transport_protocol_id()` folds into the
handshake id. Host snapshot production already collects `KinematicMoverState` for registered
movers — `target_segment` rides along. Client apply seeds the predictive mover driver from
the replicated phase including `target_segment`, so a `goToPathNode` hold reconciles rather
than overshoots.

Add the client-side source for the use bit: `SimCommand` (`crates/postretro/src/sim/mod.rs`
~line 29, currently `movement`/`fire_button`/`reload`) and `MovementInput`
(`crates/postretro/src/movement/mod.rs` ~line 35, currently no use field) each gain a
`use_pressed: bool`, populated from `Action::Use` in the input layer;
`sim_command_to_input` and `input_command_to_sim`
(`crates/postretro/src/netcode/wire_convert.rs`, lines 19 and 47) map it onto/from
`WireMovementInput.use_pressed` (`crates/net/src/wire.rs:813`) alongside the other movement
fields. Host input handling then reads `use_pressed` per client into the per-player Use seam
map keyed by `PlayerId` — the same type Task 3 introduces, one type shared by Task 3's
per-(trigger, player) overlap tracking and this per-player `use_pressed` map — that the
trigger system (Task 3) consumes for remote players. No trigger-state wire payload is added
(E18 seam).

Extend the in-memory prediction/reconciliation harness with a trigger scenario at the E15
profile. `LoopbackHarness::host_tick`
(`crates/postretro/src/netcode/predict_reconcile_harness_test_fixtures.rs` ~line 525) is
hand-rolled and calls `run_kinematic_mover_tick` directly, **not** `simulate_tick`, so it
will not run Task 3's trigger system automatically. Therefore: (a) `host_tick` must invoke
the Task 3 trigger system each host tick; (b) the harness setup loads a host-side
`trigger_volume` targeting the mover; (c) `step()` / `SimCommand` can inject a per-player
Use edge that becomes `use_pressed` on the client→host path; (d) reuse the existing readouts
`mover_position_error` (in that fixtures file, ~line 960) and `MOVER_TOLERANCE_M` /
`assert_non_accumulating` (in the sibling `crates/postretro/src/netcode/predict_reconcile_harness.rs`,
~lines 650 and 1002) for the tolerance / no-drift assertions. The host fires a trigger
(`start` and a `goToPathNode`); assert the client's predicted mover tracks the host within
`MOVER_TOLERANCE_M` with no accumulating correction (`assert_non_accumulating`), and that a
client-issued Use press fires a use trigger on the host.

### Task 6: Demo map, diagnostics, and documentation

Extend a dev map with a `trigger_volume` wired to A's elevator/platform (a touch pad that
starts it, and a use panel that sends it to a named node). Diagnostics (non-gated
deliverable — no AC): optional debug-line overlay for trigger AABBs and their target
tags; log one summary at level load (trigger count, activation/command breakdown).
Update context docs where the durable contract changed (also non-gated — docs, not
covered by an AC):

- `entity_model.md` — `TriggerVolume` component and the new trigger stage in the §5
  update-order table (after Player movement, before AI brain).
- `scripting.md` — the mover command reaction primitives (§10) and the `kinematic_mover`
  `world.query` kind; note commands are the closed-vocabulary declarative path, not
  per-tick control.
- `build_pipeline.md` — the `trigger_volume` FGD/compiler/PRL path and section 44.
- `networking.md` — the `target_segment` mover field, the `use_pressed` input bit, and
  the host-authoritative trigger-firing model (clients never evaluate triggers).

## Sequencing

**Phase 1 (sequential):** Task 1 — the command vocabulary, driver move-and-hold, and
shared applier, proven in-memory. Everything downstream calls the applier.

**Phase 2 (sequential):** Task 2 — the `TriggerVolume` component, PRL section, FGD,
compiler, and load. Establishes the trigger data and runtime carrier.

**Phase 3 (concurrent):** Task 3 and Task 4 — the host trigger system versus the script
authoring path. Disjoint modules; both consume Task 1's applier, and Task 3 also consumes
Task 2's volumes.

**Phase 4 (sequential):** Task 5 — networking. Consumes Task 1's `target_segment` shape
and Task 3's `use_pressed` consumer; one wire bump.

**Phase 5 (sequential):** Task 6 — demo map, diagnostics, docs, and QA.

## E18 Seam

> Non-normative rationale. Nothing here ships beyond what Scope lists; it records why the
> foundation's shape lets E18 build co-op without a refactor. E18 consumes E17 machinery;
> C draws the line A's Open Questions deferred ("trigger ownership and late-join semantics
> for co-op set pieces").

Three seams, each already in Scope:

1. **One firing decision point.** `evaluate_trigger_activation(state, activator)` is the
   sole gate every touch and use activation passes through, and it already takes the
   activator id. E18 turns it into a pluggable ownership check (per-player vs shared,
   fan-out) — a policy swap at one call site, not a new firing path. The AC asserting
   single-gate routing protects this.

2. **Serializable trigger state.** Armed/latched/rearm live in a registry component
   (serde), reserved `ComponentKind::TriggerVolume = 14`, shaped so E18 restores late-join
   trigger state as data. E18 adds a `COMPONENT_KIND_TRIGGER_STATE` wire mirror the exact
   way A added `KinematicMover` (kind constant + `RawComponentPayload` slot +
   `ComponentPayload` variant + drift guard) — additive, not a rework. C ships no such
   wire, keeping trigger state off the wire until late-join needs it.

3. **Durable command semantics.** The four verbs' phase effects (**Command Semantics**)
   are a contract E18 won't change; co-op reveal/spawn scripting binds these same verbs.

What C keeps open, called out so it is not optimized away: the activator id reaches the
gate; trigger state is a first-class serde component reachable by a future wire mirror;
command application is server-authoritative already (clients never fire), so co-op
ownership is a host-side policy decision with no client trust to unwind.

## Script Syntax Examples

```ts
// TypeScript. Luau is a behavioral twin (require("postretro")).
// A level data script selects movers by component + tag and binds closed-vocabulary
// commands to a named event. Rust owns tick evaluation; this only declares.
import { world, defineReaction } from "postretro";

const bridge = world.query({ component: "kinematic_mover", tag: "bridge-lift" });

defineReaction("raiseBridge", () => [
  ...bridge.start(),
  ...bridge.goToPathNode("top"),
]);
```

A pure-KVP author needs no script: place a `trigger_volume` brush over the approach,
set `activation = touch`, `target_tag = bridge-lift`, `command = go_to_path_node`,
`command_arg = top`, `fire_mode = once`. The FGD is the whole interface.

## Open Questions

None block this draft. Deferred by design:

- Trigger ownership, activation fan-out, and late-join restoration — E18 (the seam above).
- Whether a `use` trigger ever needs an aim raycast rather than volume overlap — revisit
  if a set-piece demands look-at interaction; C ships overlap-only.
