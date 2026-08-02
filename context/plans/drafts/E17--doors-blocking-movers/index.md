# Doors and Blocking Movers (E17-E)

## Goal

Make kinematic movers react to what's in their path. A mover push blocked by
static geometry is a no-op today (`mover_carry.rs:126`); movers are "unstoppable
kinematics" that displace but never yield (`movement.md` §6). E17-E turns that
seam into a real block policy — reverse, stop, or crush — authored per mover and
settable at runtime, applied to players and enemies, with an auto-close timer
and author-wired sounds. Doors become authoring on `kinematic_mover`, not a new
entity class.

## Scope

### In scope

- A per-mover **block policy** — `displace` (default, today's behavior),
  `reverse`, `stop`, `crush` — authored as a seeded KVP and settable at runtime
  via a `moverSetBlockPolicy` scripting command.
- Reaction on **swept contact** (the mover's leading face crossing an entity),
  not only terminal pinch — a `reverse`/`stop` door reacts to an entity standing
  in the doorway.
- A host-only **mover blocking/crush pass** that detects contact and dispatches
  the policy. Reverse flips travel direction; stop holds and resumes when clear
  (new replicated `blocked` flag); crush damages a pinned entity on a repeating
  cadence through the E16 chokepoint.
- **Players and enemies.** Enemy handling requires net-new mover-vs-enemy
  collision (movers collide only with the player today).
- **Auto-close / auto-return timer** — host-only. Engine default, overridable
  mod-wide via a new `ModManifest` field, overridable per mover via KVP. Homes
  interruption (a `stop` command cancels the pending return) and re-trigger
  (resets the timer).
- **Mover sound events** — a new `TickEvents` mover bucket emitting named
  transition events (open/close/blocked/crush) drained through the *executing*
  dispatch path, wired by authors to `playSound`.
- Wire: one new replicated phase field (`blocked`); `SNAPSHOT_VERSION` and
  `WIRE_VERSION` bump.

### Out of scope

- Portal/visibility interaction — E17-F owns whether a door affects what
  renders.
- Enemies **riding** movers (carry/displace under the `displace` default).
  Enemies are handled only under `reverse`/`stop`/`crush`; a `displace` mover
  ignores enemies exactly as today. Enemy navmesh does not account for moving
  movers.
- Client-predicted blocking. The block decision depends on entity positions, so
  it is host-authoritative and reconciled — never predicted (see Direction).
- Spatialized crush/door audio (positional, reverb) — `audio.md` defers
  spatialization; sounds play dry.
- A typed `onMoverEvent` dispatch source (see Alternatives rejected).
- Client-side crush prediction; per-voice volume/looping on the sound path.

## Direction

**Problem.** A mover push blocked by static geometry returns the entity still
penetrated and logs `pinch/crush resolution is deferred` (`mover_carry.rs:126`).
The cause is E17-A deferring the block decision, not a symptom of collision
tuning. Observed in `movement.md` §6: *"Movers are unstoppable kinematics… dev
maps must avoid pinch points."* A door cannot react to what it hits, so a closing
mover soft-locks a pinned player and neither crushers nor reversing doors are
authorable.

**Placement.** Engine floor for the mechanism, author data for the taste
(`scripting.md` §1). The engine owns the *evaluator* — collision detection, the
deterministic driver, the host-authoritative block *decision*, the wire. The
author owns the *policy point* — which of `displace/reverse/stop/crush` a mover
uses — as seeded data plus a runtime setter, exactly the closed-vocabulary shape
E17-C/D established for mover commands. Crush damage amount and cadence are
per-mover KVPs: this is level trap-authoring (a specific crusher's lethality),
not gameplay balance, so it is a mover KVP like `speed`/`mode` and does not
violate `entity_model.md` §4's "no map-overridable gameplay tuning" rule, which
targets weapon/movement balance. The mover blocking pass is host-only because
the block decision reads entity positions.

**Prior commitments.**
- `movement.md` §6 declares movers unstoppable and crush deferred — E17-E
  **overturns** this by design; the doc updates at promotion.
- E17-A's client prediction is a **pure function of {replicated phase, static
  path}** (`networking.md` §"Phase boundaries"). Blocking makes mover motion
  depend on entity positions, which the client does not simulate for remote
  entities — so the block decision **diverges** to host-authoritative and
  reconciled. This is the central architectural consequence, not a choice.
- `networking.md` "Hash only what cannot be replicated": `block_policy` and the
  auto-return timer are **neither** replicated **nor** a client-prediction input
  (the client never evaluates the policy or the timer, only observes their
  effects). So both stay host-only and off the level content digest — the same
  shape as `carry_yaw`, a mover component field "held locally rather than
  replicated" (`kinematic_mover.rs:47`). Only `blocked` — the stop-hold state
  the client must see to avoid snap-back — goes on the wire.
- `scripting.md` §12: "a dispatch source is added only when a case blocks on
  it." Mover sound events publish no ephemeral per-fire inputs an author needs,
  so E adds no typed source; named events reuse the dispatch-address model.

**Alternatives rejected.**
- **Client-predicted blocking.** Would need clients to simulate every entity's
  position against every mover — data they do not have for remote pawns/enemies,
  and a determinism liability on the hottest path. Host-authoritative +
  reconcile is the only model consistent with A. Rejected.
- **A typed `onMoverEvent({tag}, phase, [r])` dispatch source** (the mover twin
  of `onTriggerEvent`). Richer and tag-bound, but justified only by ephemeral
  dispatch inputs — which mover phase transitions do not have. It costs an SDK
  params type, an install-time binding gate, and a typedef surface for no gain
  over `defineReaction("<authored name>", [playSound(...)])`. Deferred until a
  concrete case needs per-fire mover-event inputs. Rejected for v1.
- **Doors as a new entity class.** Settled earlier: a door is authoring KVPs on
  `kinematic_mover`. Rejected.

**Foreclosures / one-way doors.** The `blocked` wire field and the
`SNAPSHOT_VERSION`/`WIRE_VERSION` bump are a one-way door — removing them later
costs another version bump. Keeping `block_policy` host-only (off the digest) is
**reversible**: it can later join the digest without a breaking change if a
prediction model ever needs it. Choosing host-authoritative reconciliation
forecloses a pure-function-predicted blocking model without a wire change; the
cost of reversing is a wire bump plus reconcile rework. Nothing else material.

## Acceptance criteria

- [ ] **AC1 (stop, player).** A mover with `block_policy = stop` that contacts
  the player holds in place while contact persists and resumes when the player
  steps clear. The client's predicted mover shows no snap-back at the hold edge
  or the resume edge.
- [ ] **AC2 (reverse, player).** `block_policy = reverse` reverses the mover's
  travel direction on player **swept contact** — a player standing in the
  doorway reopens it — not only when pinned against static geometry.
- [ ] **AC3 (crush, player).** `block_policy = crush` damages a player pinned
  between the mover and static geometry on a repeating cadence while pinned, and
  stops when the player is no longer pinned or has died. A player with room to be
  pushed clear takes **no** crush damage.
- [ ] **AC4 (default preserved).** No authored policy behaves exactly as today:
  the player is displaced out of overlap where possible, and the push no-ops
  (no damage, no mover reaction) when blocked.
- [ ] **AC5 (death paths reused).** A crush kills a player through the existing
  HP-latch → `playerDied` path and an enemy through the existing despawn +
  impact-animation path. No new death path is introduced.
- [ ] **AC6 (enemies).** A map-placed enemy is subject to reverse/stop/crush the
  same as a player: a `reverse`/`stop` door reacts to an enemy in the doorway,
  and a `crush` mover kills an enemy it pins.
- [ ] **AC7 (co-op reconcile).** A connected client reconciles a host block
  reaction — hold, reverse, and crush HP loss — without divergence, and the
  block decision never executes on the client.
- [ ] **AC8 (runtime setter parity).** `moverSetBlockPolicy(policy)` changes a
  mover's policy at runtime, and the reaction, sequence, and KVP-authored routes
  produce identical resulting behavior (parity test, twin of the `set_spin_rate`
  parity test).
- [ ] **AC9 (auto behavior).** A mover with `auto_close_ms` reverses toward its
  closed position after the timer elapses; re-triggering during the hold resets
  the timer; a `stop` command during the hold cancels the auto-return.
- [ ] **AC10 (default cascade).** The engine auto-close default applies when
  neither the mover KVP nor the mod descriptor sets one; a `ModManifest`
  auto-close field overrides the engine default mod-wide; a per-mover KVP
  overrides both.
- [ ] **AC11 (sound executes).** Authoring a mover transition-event name and a
  matching `defineReaction(name, [playSound(...)])` plays the sound on that
  transition — the mover event reaches the executing dispatch path, not the
  non-executing drain.
- [ ] **AC12 (wire).** The drift-guard tests on both the net and engine sides
  pass with the new `blocked` phase field, and a pre-`blocked` peer is refused by
  the handshake.

## Tasks

### Task 1: Blocking substrate + player stop policy (thin slice)

Build the end-to-end vertical slice that falsifies the host-authoritative block
reconciliation model, using `stop` for the player only. Add a `BlockPolicy` enum
(`Displace`, `Reverse`, `Stop`, `Crush`; `#[serde(rename_all = "snake_case")]`)
and a **host-only** `block_policy` field on `KinematicMoverComponent`, seeded
from a new `block_policy` KVP on `kinematic_mover` (carried on PRL
`KinematicGeometry`, parsed alongside `mode`/`speed`; default `Displace`). Field
is held locally, **not** added to the wire mirror — mirror `carry_yaw`
(`kinematic_mover.rs:47`). Add a **replicated** `blocked: bool` phase field to
`KinematicMoverComponent` and to the `KinematicMoverState` wire payload, bumping
`SNAPSHOT_VERSION` 12→13 (`net/src/wire.rs:78`) and `WIRE_VERSION` 15→16
(`net/src/handshake.rs:11`); extend the engine↔wire conversion in
`crate::netcode` and both drift-guard tests. Add a new host-only module (e.g.
`kinematic_mover/blocking.rs`) with a `run_mover_blocking_pass` that runs after
agent steering (entity_model §5 order 6) and before the death sweep (order 8);
it sweeps every active mover's collider against the player capsule using the
existing penetration/swept queries in `collision::moving`
(`deepest_mover_push_penetration`, the swept variant), detects contact, and for
`Stop` sets `blocked = true` while the player is in swept contact and clears it
when clear. The deterministic driver (`kinematic_mover` tick, order 1) must
**honor `blocked`**: a blocked mover does not advance `segment_elapsed_ms`
(holds position) and resumes from the held phase when cleared. Gate the pass
host-only (the block decision never runs on a client; `displace_from_movers`
stays unchanged on all peers). Define the shared `MoverEventKind` enum and the
mover `TickEvents` bucket here (populated by later tasks; drained by Task 6), and
emit `Blocked`/`Opened` where this task produces those edges. Reconcile: a client
re-runs the driver from the replicated `blocked` phase with no snap-back.
Verifies AC1, AC4, AC7 (stop), AC12.

### Task 2: Player reverse + crush policies

Extend `run_mover_blocking_pass` (Task 1's module) with the remaining player
policies. `Reverse`: on swept contact flip the mover's travel direction using the
same phase mutation `MoverCommand::Reverse` performs (re-anchor direction; do not
route through the trigger command surface — the pass mutates phase directly,
host-side). `Crush`: while the player is **pinned** (the displace push is blocked
— the `unblocked_mover_displacement` blocked condition, generalized), apply
damage through `apply_damage_with_context` with `DamagePayload { amount }` and
`DamageContext { source_id: "mover.crush", attacker: None, weapon: None, zone:
None, producer: DamageProducer::InTick }`, on a repeating cadence driven by a
per-mover countdown that mirrors `BrainComponent.attack_cooldown_remaining_ms`.
Crush damage amount (`crush_damage`) and cadence (`crush_interval_ms`) are seeded
mover KVPs with engine defaults. A player with room to be pushed clear is not
pinned and takes no damage. The lethal path is unchanged — HP latches at zero and
the existing sweep fires `playerDied`. Emit the `Crushed` `MoverEventKind` where
crush damage lands. Verifies AC2, AC3, AC5 (player).

### Task 3: Mover-vs-enemy collision + enemy policies

Net-new collision: extend `run_mover_blocking_pass` to sweep each active mover's
collider against enemy collision volumes (enemy `Agent` capsule / entity AABB,
`entity_model.md` §7). Movers collide only with the player today, so this is the
real cost of enemy scope. Apply the full policy matrix to enemies: `reverse` and
`stop` react to an enemy in swept contact exactly as for the player (mover-phase
mutation); `crush` damages an overlapped enemy through the same
`apply_damage_with_context` call (`source_id: "mover.crush"`), with the enemy's
lethal transition flowing through the existing death sweep → authored impact
policy → despawn/animation (remote enemies replicate as host despawn/animation;
they carry no client `Health`). An enemy is "pinned" for crush purposes whenever
the mover overlaps it — enemies are not mover-pushed, so there is no
displace-clear escape, unlike the player (warrant: enemies have no mover push
path). Under the `displace` default, enemies are ignored (no push, no reaction).
Consumes Task 2's policy dispatch. Verifies AC5 (enemy), AC6.

### Task 4: `moverSetBlockPolicy` scripting setter

Add a runtime setter mirroring `set_spin_rate` across its seven sites. Add
`MoverCommand::SetBlockPolicy(BlockPolicy)` (`kinematic_mover.rs:19`,
`#[serde(rename_all = "snake_case")]` → `set_block_policy`) and its applier arm
in `apply_mover_command` (`commands.rs`), which sets the host-only `block_policy`
field. Register the `moverSetBlockPolicy` primitive on both the reaction
(`register_mover_reaction_primitives`) and sequenced
(`register_sequenced_mover_primitives`) paths with an args struct twin of
`MoverSetSpinRateArgs`; add it to the consequential-primitive allowlist
(`reaction_dispatch.rs`); extend the SDK type templates (`sdk_lib.luau` /
`sdk_lib.d.ts`) and regenerate the `expected.d.*` fixtures; add the SDK handle
method `setBlockPolicy(policy)` in `sdk/lib/entities/movers.{ts,luau}`; extend
the closed-vocabulary test in `scripting/reactions/registry.rs`. Add the parity
test twin of `set_spin_rate_reaction_and_sequence_routes_match_shared_kvp_command_path`
proving reaction, sequence, and KVP routes converge. Size watch: `commands.rs` is
near the ~800-line soft threshold — split only if the addition pushes it well
past; otherwise extend in place. Verifies AC8.

### Task 5: Auto-close timer + mod-descriptor default

Add a host-only auto-close/auto-return timer to the mover. The mover carries a
seeded `auto_close_ms` value; when a mover reaches its open extent and
`auto_close_ms > 0`, the host counts the timer down and on expiry issues the
close (reverse toward the closed waypoint), mutating replicated phase
(`direction_sign`) — the timer itself is host-only state, never on the wire
(clients reconcile the reversal, same shape as the block decision). A re-trigger
during the hold resets the countdown; a `stop` command (interruption) during the
hold cancels the pending return. The default value cascades: engine constant <
`ModManifest` field < per-mover KVP (most specific wins). Add the `ModManifest`
field mirroring `render` — a new field on `ModManifestResult`
(`scripting-core/.../runtime/types.rs`), parsed in the js/lua manifest parsers
(`data_descriptors/js/manifest.rs`, `.../lua/manifest.rs`), drained at boot in
`run_deferred_mod_init` (`startup/splash_lifecycle.rs`) into the mover default,
and surfaced in the SDK `ModManifest` type. Emit the `Closed`/`Opened`
`MoverEventKind` at the close/open edges. Timer state is transient — cleared on
level unload with the rest of mover phase. Verifies AC9, AC10.

### Task 6: Mover sound events through the executing drain

Wire the mover `TickEvents` bucket (defined in Task 1) to author-wired
`playSound`. The mover carries seeded per-transition event-name KVPs
(`open_event`, `close_event`, `blocked_event`, `crush_event`) — each an optional
string naming a reaction **dispatch address**. When a mover produces a
transition edge (Tasks 1/2/5 emit the `MoverEventKind`), the host resolves the
authored name for that kind and pushes it into the mover bucket. Drain the bucket
post-tick through the **executing** `fire_named_event_with_sequences`
(`reaction_dispatch.rs:141`, the death-event precedent at `main.rs:2462`) — **not**
the non-executing `fire_named_event` the weapon/movement drains use
(`reaction_dispatch.rs:110`) — so a `defineReaction("<name>", [playSound(...)])`
actually dispatches `SystemReactionCommand::PlaySound` into
`App::dispatch_system_commands` → `Audio::play`. Sound is host-local
presentation and never replicated; each peer's own drain plays its own sounds.
Size watch: `main.rs` is far past the soft threshold, but the drain addition is a
localized insertion into the existing post-tick drain sequence — a `main.rs`
split is its own effort, out of scope here; note the smell, do not split.
Verifies AC11.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies host-authoritative
block reconciliation and the wire bump before any fan-out.
**Phase 2 (concurrent):** Task 2 (player reverse+crush — edits the pass module),
Task 4 (scripting setter — edits `commands.rs`/SDK), Task 5 (auto behavior + mod
descriptor — edits the driver/manifest/boot). Disjoint files.
**Phase 3 (sequential):** Task 3 — consumes Task 2's policy dispatch in the pass
module.
**Phase 4 (sequential):** Task 6 — emits at edges built by Tasks 1/2/5 and drains
through the executing path; touches the pass and driver after they settle.

## Rough sketch

- `BlockPolicy` enum and `block_policy` / `crush_damage` / `crush_interval_ms` /
  `auto_close_ms` / `*_event` fields live on `KinematicMoverComponent`
  (`crates/entities/src/components/kinematic_mover.rs`); the policy, timer, and
  event names are host-only (not in the wire mirror), following `carry_yaw`.
- The blocking pass is a new host-only module under `crates/postretro/src/
  kinematic_mover/`; it reuses `collision::moving` penetration/swept queries and
  calls `apply_damage_with_context` directly. It runs after agent steering and
  before the death sweep, feeding mover phase for the next mover tick — the same
  producer→next-tick shape trigger commands use.
- `blocked` is the only new wire field, appended to the `KinematicMoverState`
  phase mirror; bitcode owns layout.
- `MoverCommand::SetBlockPolicy` and `moverSetBlockPolicy` follow `SetSpinRate` /
  `moverSetSpinRate` exactly.
- Mover sound events reuse the named-reaction dispatch address model; the only
  new plumbing is the bucket + routing it through the sequence-aware drain.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | KVP |
|---|---|---|---|---|---|
| Block policy | `BlockPolicy` / `block_policy` field | host-only (not on wire); command serde `set_block_policy` | `"displace"\|"reverse"\|"stop"\|"crush"` | same | `block_policy` on `kinematic_mover` |
| Blocked hold | `blocked: bool` phase | new `KinematicMoverState` field | n/a | n/a | n/a |
| Set-policy command | `MoverCommand::SetBlockPolicy` | `set_block_policy` | `setBlockPolicy(policy)` / `moverSetBlockPolicy` | same | n/a |
| Auto-close | `auto_close_ms` field + `ModManifest` default | host-only | `movers.autoCloseMs` manifest field | same | `auto_close_ms` |
| Crush tuning | `crush_damage`, `crush_interval_ms` | host-only | n/a | n/a | `crush_damage`, `crush_interval_ms` |
| Sound events | `MoverEventKind` + `*_event` name fields | host-only | `defineReaction("<name>", …)` | same | `open_event`/`close_event`/`blocked_event`/`crush_event` |

## Wire format

One new field: `blocked: bool` appended to the `KinematicMoverState` phase
mirror, beside `started`/`completed`. bitcode owns endianness and bit-packing —
no manual layout. It is a deterministic input to client mover reconciliation, so
it rides the existing mover phase payload (added after the current phase fields,
consistent with `ComponentKind` numeric order for the payload). `SNAPSHOT_VERSION`
12→13, `WIRE_VERSION` 15→16; both drift-guard tests (net side and engine side)
extend. No other phase field changes. `block_policy`, the auto-return timer, and
crush tuning are host-only and never serialized — mirror `carry_yaw`.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| The block decision is host-authoritative; no client path mutates mover phase on block | Task 1 (host-only pass) | Threatened if `displace_from_movers` or any client tick reacts to a block | AC7 |
| `blocked` is the only new replicated block state; `block_policy`, auto-return timer, crush tuning stay host-only | Task 1, Task 5 | Threatened if a client reads `block_policy` or the timer, or either lands on the wire | AC7, AC12 |
| Crush damage flows only through `apply_damage_with_context` (no direct HP write); death paths unchanged | Task 2, Task 3 | Threatened by a bespoke HP mutation or death path | AC3, AC5, AC6 |
| The `displace` default is byte-for-byte today's behavior and the per-tick local overlap resolution on all peers | Task 1 | Threatened if the pass alters `displace_from_movers` | AC4 |
| Mover sound events route through the executing `fire_named_event_with_sequences`, never the non-executing `fire_named_event` | Task 6 | Threatened by copying the weapon/movement drain | AC11 |

## Orderings

Spec text; the test tasks cite these rows.

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Two entities pinned by one crusher on the same tick | Both detected in one pass | Both take crush damage that tick; both swept the same tick if lethal |
| Reverse door, two entities in contact same tick | Both contacts detected before the direction flip | Direction flips once (idempotent per tick), not twice |
| Stop-hold contact clears within the hold before the next mover tick | Set then clear across one blocking pass | Mover never advanced a held segment; resumes cleanly, no snap-back on the client |
| Fast mover sweeps fully through an entity in one tick (no end-of-tick penetration) | Swept-contact query, not terminal-penetration query | Policy still fires (swept contact catches it) |
| Auto-close timer expires the same tick a re-trigger arrives | Re-trigger observed before the expiry check | Timer resets; no close that tick |
| Auto-close hold crosses a level unload | Timer is host-only transient | Cleared on unload; no dangling reverse |
| `stop` command during an auto-return hold | Interrupt observed during hold | Pending auto-return cancelled; mover stops |
| Crush policy, player has room to be pushed clear | Displace push resolves (not pinned) | Player pushed clear; no crush damage |
| Enemy under `displace` default | No push path for enemies | No push, no reaction — ignored as today |
| `crush_interval_ms` spans multiple ticks while pinned | Countdown per tick, mirrors `attack_cooldown` | Damage applied each interval, not each tick |
| `moverSetBlockPolicy` mid-contact | Field set this tick; pass reads it next tick | Next pass uses the new policy; host-only, no client divergence |

## Script syntax examples

Door authored in the map (KVPs on a `kinematic_mover`), with a reversing policy,
a 3s auto-close, and a blocked sound:

```
// kinematic_mover brush entity KVPs
block_policy   "reverse"
auto_close_ms  "3000"
blocked_event  "vaultDoorBlocked"
open_event     "vaultDoorOpen"
```

Author wires the sound and (optionally) flips a specific door to a crusher at
runtime:

```typescript
import { defineReaction, playSound, world } from "postretro";

// Named dispatch address the mover emits on its blocked edge.
defineReaction("vaultDoorBlocked", [playSound({ sound: "door_blocked" })]);
defineReaction("vaultDoorOpen",    [playSound({ sound: "door_open" })]);

// Runtime policy change on a tagged mover.
const crusher = world.query({ component: "kinematic_mover", tag: "trap_a" });
crusher.setBlockPolicy("crush");
```

Mod-wide auto-close default in the manifest, mirroring `render`:

```typescript
export default defineMod({
  name: "my-mod", id: "example.mymod", version: "1.0",
  movers: { autoCloseMs: 2500 }, // engine default unless a mover KVP overrides
});
```
