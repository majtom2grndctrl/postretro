# E17-C — Trigger/Event and Script Command Surface

> **Status:** draft.
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
| mover handle | `collect_mover_handles` → `MoverEntity` handle | n/a | `MoverEntityHandle` (`sdk/lib/entities/movers.{ts,luau}`) | n/a |
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
  raw↔typed conversion, and the `InputCommand` round-trip tests. The host reads the
  replicated edge for remote players; the local player reads `Action::Use` directly.
- Bump `SNAPSHOT_VERSION` 8 → 9 and the transport wire-version/`protocol_id` gate (both
  are existing-type layout changes — see networking.md's two-gate handshake). The
  engine/net `component_kind_discriminant` drift guard is unaffected: no new replicated
  kind is added (`TriggerVolume = 14` is engine-only, like the other non-wire kinds).

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
  re-fires only after `rearm_ms` elapses; an `enabled_on_spawn = 0` trigger does not
  fire until enabled — verified in the deterministic sim.
- [ ] A `use` trigger fires only when a player capsule overlaps the volume **and** a Use
  rising edge occurs the same tick; capsule overlap alone does not fire it.
- [ ] Every touch and use activation reaches command dispatch only through
  `evaluate_trigger_activation`; a test asserts it is the sole call path (no second
  firing route), and that the gate receives the activator id.
- [ ] The four mover commands produce the **Command Semantics** phase effects,
  verified deterministically: `start` resumes from current phase and no-ops when
  completed; `stop` freezes without losing mid-segment progress; `reverse` retraces from
  the exact current position with no teleport; `goToPathNode(name)` moves to the named
  waypoint and holds; an unknown node name warns and no-ops.
- [ ] A command targeting a tag applies to every tagged `KinematicMover` and skips
  tagged non-mover entities with a warning.
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
  motion within the same bounded tolerance A established, with no accumulating drift; a
  client's Use press reaches the host via `use_pressed` and fires a use trigger there.
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
Implement the **Command Semantics** table exactly — pure phase mutation, no wall-clock,
no RNG. Extend `KinematicMoverComponent` with `target_segment: Option<u16>` (phase) and
`waypoint_names: Vec<String>` (static, seeded at construction alongside `waypoints`; not
replicated). Extend the deterministic driver (`advance_mover`) so that when
`target_segment` is `Some`, the mover advances toward it and holds on arrival (endpoint
wait then idle), clearing `target_segment`; `target_segment` supersedes `once`/`ping_pong`
endpoint reversal until reached. `go_to_path_node` resolves the node name against
`waypoint_names` to an index; unknown names warn and no-op. The driver stays a pure
function of {phase, static path, dt} so host and client reproduce it identically.

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
replicated this slice, like `Light`). `component_kind_from_name` keeps `trigger_volume`
unmapped (not script-queryable). The component carries static config
{ `activation`, `target_tag`, `command: MoverCommand`, `fire_mode`, `rearm_ms`,
`enabled_on_spawn` } and mutable state { `armed`, `latched`, `rearm_remaining_ms` };
`armed` seeds from `enabled_on_spawn`.

Add `SectionId::TriggerVolumes = 44` to `postretro-level-format` (+ the hand-written
`from_u32` arm) and `TriggerVolumeRecord` with the encoding pinned in **Wire Format**;
serialization tests. Add the FGD `@SolidClass = trigger_volume` (choices for
`activation`/`command`/`fire_mode`/`enabled_on_spawn`). Compiler: collect
`trigger_volume` brush entities in `parse.rs` before the brush-entity skip path, compute
their AABB (fog_volume precedent — AABB only, **not** the mover's textured projection),
source `tags` from `_tags`, validate `command`/`command_arg`/`rearm_ms`, and pack the
section in `pack.rs`, emitting it only when the map has triggers. Assert the trigger
brush is absent from `world_brush_ids` / `brush_volumes` and the static `GeometrySection`.

Load section 44 into `LevelWorld` (`crates/level-loader/src/prl.rs` /
`prl_loader.rs`; absent/empty = no triggers). At level load, spawn one entity per record
with a `Transform`, the `TriggerVolumeComponent` (seeded), the record's `tags`, and a
runtime AABB in a trigger side-table keyed by `EntityId` (mirror `FogVolumeBridge`). Both
host and client spawn triggers locally; triggers are **not** registered in
`ReplicableSet`.

### Task 3: Host-authoritative trigger system and single firing gate

Add a fixed-tick trigger system that runs after player movement settles (a new
update-order stage after Player movement tick, before AI brain tick — see
entity_model.md §5). Each tick, for each trigger, the system computes activation against
authoritative player state: `touch` = a player capsule's rising-edge entry into the
trigger AABB (track prior-overlap per trigger to detect the edge); `use` = capsule
overlap plus a Use rising edge for that player. Single-player reads `Action::Use`
locally; co-op reads the host-side per-player `use_pressed` (Task 5). On activation, call
the sole gate `evaluate_trigger_activation(&TriggerVolumeComponent, activator: PlayerId)
-> Fire | Suppress`, which this slice decides on armed/latched/rearm only and logs the
discarded activator in dev builds. On `Fire`: resolve `target_tag` to entities
(`query_by_component_and_tag`), call Task 1's `apply_mover_command` on each
`KinematicMover` target, then update trigger state — `latched = true` for `once`,
`rearm_remaining_ms = rearm_ms` for `multiple`. Count down `rearm_remaining_ms` each tick.
Enforce that both activation paths funnel through the one gate (no direct dispatch).

Trigger firing and command application are server-authoritative: on a client the trigger
system and applier are inert; mover phase changes arrive via replication. Wire the system
into `simulate_tick` (call-site only in `main.rs`/`sim`; logic in a focused module).

Tests (deterministic sim): touch rising-edge fire; `once` fires once; `multiple` rearm
gating; `enabled_on_spawn = 0` suppression; use requires overlap + press edge; the
single-gate assertion; end-to-end trigger → mover command → observed motion.

### Task 4: Script authoring path — world.query mover kind, handle, reaction primitives

Register `kinematic_mover` as a `world.query` component kind: add the
`WorldQueryComponent` enum variant (`crates/lighting/src/script_primitives.rs`, snake
literal `kinematic_mover` matching `fog_volume`), a `QueryFilter::KinematicMover { tag }`
arm and `collect_mover_handles` in `crates/postretro/src/scripting/entity_world_primitives.rs`,
and a `MoverEntity` handle type (id, position, tags). Add
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
`SNAPSHOT_VERSION` 8 → 9 and the protocol gate. Host snapshot production already collects
`KinematicMoverState` for registered movers — `target_segment` rides along. Client apply
seeds the predictive mover driver from the replicated phase including `target_segment`, so
a `goToPathNode` hold reconciles rather than overshoots. Host input handling reads
`use_pressed` per client into the per-player use edge the trigger system (Task 3)
consumes for remote players. No trigger-state wire payload is added (E18 seam).

Extend the in-memory prediction/reconciliation harness with a trigger scenario at the E15
profile: the host fires a trigger (`start` and a `goToPathNode`), and assert the client's
predicted mover tracks the host within bounded tolerance with no accumulating correction,
and that a client-issued Use press fires a use trigger on the host.

### Task 6: Demo map, diagnostics, and documentation

Extend a dev map with a `trigger_volume` wired to A's elevator/platform (a touch pad that
starts it, and a use panel that sends it to a named node). Diagnostics: optional debug-line
overlay for trigger AABBs and their target tags; log one summary at level load (trigger
count, activation/command breakdown). Update context docs where the durable contract
changed:

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
- Whether `goToPathNode` should accept a node index as well as a name — names only for now.
