# E16 - Host Combat Command Application

> **Status:** draft.
>
> **Epic:** 16 - Combat.
>
> **Milestone:** Resolution Modes.
>
> **Fits first:** realizes the Resolution Modes roadmap bullet "server-authoritative
> fire," pulled forward as a prerequisite for co-op-correct Weapon Systems — the
> ammo/reload spec depends on it. The host already applies each remote client's
> MOVEMENT per pawn through the prediction + reconciliation loop; this extends the
> SAME loop to FIRE (and threads RELOAD), so a remote client's shot is host-resolved
> at their exact aim and reconciled back to them the way movement is.

## Goal

Make co-op fire server-authoritative and prediction-consistent. Each remote client
predicts its own shot locally; the host authoritatively resolves that exact shot
from the client's pawn at the client's full aim; the firing client reconciles the
result the same way it reconciles movement — reusing the command-log / ack / replay
loop, **not** server rewind. The same per-pawn apply seam threads reload to each
owned pawn's weapon.

Today `host_resolve_movement_inputs` keeps only `resolved.command.movement` and
drops `fire_button`; a remote client's fire never reaches their pawn on the host,
the wire carries no reload and no pitch, and fire is never predicted or reconciled.
This spec closes all of that. It is the complete server-authoritative fire slice
built on the engine's prediction + reconciliation + interpolation netcode — not a
thin apply seam, and with no rewind machinery.

Standalone-testable without ammo: with a resourceless weapon, a remote client's held
fire resolves a host-authoritative shot from THEIR pawn's eye along THEIR full aim
(yaw + pitch), and reconciles to that client. Reload rides the frame to the firing
pawn's weapon; the transfer semantics are the ammo spec's.

## Scope

### In scope

- **Full aim on the wire.** Add the firing client's aim (pitch beside `facing_yaw`,
  or the aim vector) to `SimCommand` and the wire `InputCommand`, so the host
  reconstructs the exact `aim_direction` the client aimed — not a yaw-only
  horizontal approximation.
- Generalize the host per-pawn command-apply path so fire and reload resolve to each
  owned pawn's weapon host-authoritatively, mirroring the movement path.
- **Predict + reconcile fire.** The firing client predicts its shot locally; the
  host authoritatively resolves it; the client reconciles the authoritative outcome
  through the SAME command-log / prune-through-ack / replay loop movement uses.
  Movement's `replay` stays weapon-free — the predicted weapon tick is a separate
  local tick keyed to the same command, never run inside `replay`.
- A host-side pawn -> active-weapon map (analogous to `MovementOwners`), populated at
  slot accept from the sibling weapon `spawn_net_slot_pawn` currently spawns and
  discards — captured by returning its `EntityId` instead of dropping it.
- **This spec owns the wire `InputCommand` fire/reload/aim fields and the
  `WIRE_VERSION` bump.** It introduces a `reload` intent on `SimCommand` + the wire
  `InputCommand`, threaded through `wire_convert` and the command queue. The ammo spec
  consumes `SimCommand.reload` locally and does NOT own the wire change or the bump.
- Per-pawn aim for a remote pawn: eye origin from the pawn transform + standing eye
  height; aim direction from the wire pitch + yaw. The listen host's own pawn keeps its
  camera-driven fire/aim path, appended alongside the resolved remote pawns.

### Out of scope (non-goals)

- **Server rewind, favor-the-shooter, lag-compensation history window.** Deliberately
  not built — it contradicts this engine's prediction-not-rewind netcode (the only
  "rewind" in the netcode tree is `adaptive_delay_increase_never_rewinds_render_target`,
  asserting interpolation NEVER rewinds) and the `index.md` §4 non-goal "full
  server-rewind lag compensation." Co-op fire is made to feel right by client
  prediction + reconciliation, not by rewinding the host to the shooter's past view.
- The ammo resource, magazine/reserve semantics, and what reload transfers — the ammo
  spec owns those. This spec delivers the reload intent to the pawn's weapon; it does
  not define the mutation.
- The per-pawn ammo REPLICATION projection — the ammo spec owns the downstream read
  the firing client reconciles ammo against.
- Predicting reload. Reload stays host-authoritative and arrives via snapshot; only
  fire is predicted client-side.

## Acceptance criteria

- [ ] With a resourceless weapon (`resolution: Hitscan`, no ammo), a remote client
  sending a pressed fire button resolves a host-authoritative shot from THAT client's
  pawn: it originates at the pawn's eye and travels along the client's FULL aim (yaw +
  pitch), hitting a target on that ray. A pitched-up shot clears a waist-high wall a
  yaw-only horizontal shot would hit. The listen host's own pawn firing is unaffected.
- [ ] The firing client predicts its own shot locally and reconciles to the host's
  authoritative resolution through the movement command-log / ack / replay loop; a
  mispredicted shot converges to the authoritative outcome with no host rewind.
  Movement's `replay` stays weapon-free (signature/grep gate).
- [ ] Two owned pawns firing on the same tick each resolve against their OWN weapon
  (per-pawn cooldown, per-pawn credit source) - no cross-talk.
- [ ] A pawn whose owner sends no fire does not fire; the neutral/held gap policy
  synthesizes a released fire button, so a disconnected client's weapon does not
  auto-fire on held stale intent.
- [ ] The host maps every owned remote pawn to its active weapon at slot accept. A
  pawn whose descriptor declares no weapon maps to none and never fires host-side
  (logged once, not an error).
- [ ] The client's aim (pitch / aim vector) round-trips `SimCommand` <-> wire
  `InputCommand` through `wire_convert`; `sanitize_input_command` rejects non-finite
  aim and constrains pitch to its valid look range; the host reconstructs the same
  `aim_direction` the client aimed within tolerance.
- [ ] `reload` round-trips `SimCommand` <-> wire `InputCommand`, survives the command
  queue (sanitize/hold/neutral/catch-up), and the host per-pawn apply reaches the
  firing pawn's mapped weapon carrying the resolving client's reload bit. A held
  reload lapses to neutral under the gap policy like every other button.
- [ ] `WIRE_VERSION` is bumped (6 -> 7); both drift-guard sides still agree. A peer
  built before this change fails Gate 1, as the handshake contract requires.
- [ ] No new `unsafe` (review/grep gate).

## Tasks

### Task 1: Wire command fields - reload, aim, and the version bump

Add `reload: bool` and the client's aim to `SimCommand` (`sim/mod.rs`) and the wire
`InputCommand` (`crates/net/src/wire.rs`). For aim, add the pitch the wire lacks — a
`pitch` scalar beside `WireMovementInput.facing_yaw`, or a normalized aim vector; the
constraint is that the host must reconstruct the exact `aim_direction`
`weapon_fire_command` builds for the local pawn from `camera.aim_ray()`. Thread both
through `sim_command_to_input` / `input_command_to_sim` (`netcode/wire_convert.rs`)
and default them in `neutral_sim_command` (`command_queue.rs`). Extend
`sanitize_input_command`: reject a non-finite pitch and constrain pitch to its valid
look range (mirror the camera pitch clamp); `reload` is a `bool`, so it needs no new
validation. Bump `WIRE_VERSION` (`crates/net/src/transport.rs`, 6 -> 7) and update
both layout drift guards. Local input sampling (`main.rs`, where `fire_button` is
built from the action snapshot) fills `reload` from the reload action and the aim
from the camera; wire the reload action if absent.

### Task 2: Pawn -> active-weapon map, populated at accept

Return the sibling weapon `EntityId` from `spawn_net_slot_pawn`
(`scripting/builtins/net_descriptor.rs`) instead of discarding it. Add a host-owned
`pawn -> weapon` map beside `MovementOwners` (`command_queue.rs`), set in the same
accept path that calls `owners.set(pawn, client_id)` (`netcode/mod.rs`) and cleared
on slot close alongside `owners.remove_pawn`. A pawn with no weapon records no entry.

### Task 3: Generalize the host per-pawn command resolve

Widen `host_resolve_movement_inputs` (movement-only, `command_queue.rs:355`) to yield
the full resolved `SimCommand` per owned pawn (a `(EntityId, SimCommand)` list) so
`fire_button`, aim, and `reload` survive alongside `movement`. Gap policy, catch-up,
and cursor logic are unchanged; only the projection out of `ResolvedCommand.command`
widens from `.movement` to the whole command.

### Task 4: Host per-pawn combat apply in the sim

Generalize `simulate_tick`'s weapon stage (`sim/mod.rs`) from one `active_wieldable`
+ one `command` to a per-pawn apply, mirroring how movement iterates
`remote_pawn_inputs` then appends the local pawn. Per owned pawn: resolve its weapon
via the Task 2 map, derive aim (eye origin from the pawn transform +
`PlayerMovementComponent` standing eye height, aim direction from the wire pitch +
yaw), build a `WeaponFireCommand`, and run `weapon::tick_resolved` for that weapon.
`tick_resolved` already takes an arbitrary `active_wieldable`, so it is reused per
pawn unchanged. The listen host's own pawn keeps the camera-driven aim path, appended
last.

### Task 5: Per-pawn reload delivery seam

Route each pawn's resolved `reload` to that pawn's mapped weapon in the Task 4 apply.
This spec delivers the intent to the per-pawn weapon and proves it reaches the correct
one; the weapon-side reload function and its magazine/reserve transfer are the ammo
spec's. Keep it a single call site so the ammo spec fills the body without
re-plumbing. (The ammo spec references this seam as "its Task 5" — preserve the task
number.)

### Task 6: Client-side fire prediction and reconciliation

The client command log already retains `fire_button` per tick
(`PredictedTick.command`, `netcode/prediction.rs`), so the fire history the ack/replay
loop needs already exists. Predict the local shot when the client fires — a local
weapon tick keyed to the sent command — WITHOUT running weapons inside movement's
`replay` (which must stay weapon-free; its registry-blind signature is the guard).
Reconcile the authoritative fire outcome the host resolved, carried in the snapshot,
through the same prune-through-ack + merge-authoritative-subset path
`reconcile_local_pawn` uses for movement, so a mispredicted shot converges to the
host's resolution without rewind. The replicated ammo/magazine subset the client
reconciles against is the ammo spec's per-pawn projection; for the resourceless slice
the reconciled fact is the shot / weapon cooldown.

### Task 7: Tests

Host applies a remote pawn's fire (shot resolves from that pawn's eye along its full
aim, hits a placed target; a pitched shot clears a wall a yaw-only shot hits); the
firing client predicts and reconciles its shot to the authoritative outcome; two pawns
fire independently; a no-fire pawn stays silent; aim and reload round-trip the wire and
reach the mapped weapon; `WIRE_VERSION` drift guards pass. Reuse the `command_queue.rs`
/ `wire_convert.rs` / `predict_reconcile_harness.rs` test patterns.

## Sequencing

**Phase 1 (parallel-safe):** Task 1 (wire/fields) and Task 2 (pawn->weapon map) touch
disjoint seams and can run together.
**Phase 2 (sequential):** Task 3 - consumes the widened command.
**Phase 3 (sequential):** Task 4, then Task 5 - the apply path consumes the map, the
widened resolve, and the reload field.
**Phase 4 (sequential):** Task 6 - client predicts/reconciles fire against the
host-authoritative outcome.
**Phase 5 (sequential):** Task 7 - verifies the surface.

## Rough sketch

Grounded seams (current source):

- `netcode/command_queue.rs`: `host_resolve_movement_inputs` (line 355) drops
  `resolved.command.fire_button`; `MovementOwners` (line 48) is the map to mirror;
  `neutral_sim_command` (line 376) gains `reload: false` + a neutral-aim default.
- `sim/mod.rs`: `SimCommand { movement, fire_button }` (line 28) gains `reload` + the
  aim; `simulate_tick` (line 47) applies fire through one `active_wieldable` +
  `command.fire_button`; `weapon_fire_command` (line 186) builds the
  `WeaponFireCommand`, which already carries a full 3D `aim_origin` + `aim_direction`
  (line 196) filled from `camera.aim_ray()` for the local pawn. `weapon/mod.rs::tick_resolved`
  is the per-weapon apply, already parameterized on an arbitrary `active_wieldable`.
- `netcode/wire_convert.rs::{sim_command_to_input, input_command_to_sim}` (lines 19,
  46) round-trip `fire_button` + `facing_yaw`; add `reload` + pitch, and the pitch
  finite/range guard in `sanitize_input_command` (line 79).
- `crates/net/src/wire.rs`: `WireMovementInput.facing_yaw` (line 815) is the ONLY aim
  on the wire (no pitch); `InputCommand` (line 836) gains `reload` + the aim field;
  `WIRE_VERSION` (`crates/net/src/transport.rs:52`, currently 6) bumps to 7.
- `netcode/prediction.rs::replay` (line 426) is movement-only, weapon-free by its
  registry-blind signature; `PredictedTick.command` (line 147) already stores the full
  `InputCommand` (fire included), so the fire command log exists — only the fire
  predict/reconcile is missing. `reconcile.rs::reconcile_local_pawn` merges the
  authoritative movement subset, prunes through the host ack, replays the unacked tail
  — the loop to mirror. `interpolation.rs:803`
  (`adaptive_delay_increase_never_rewinds_render_target`) is the sole "rewind"
  reference; the netcode is predict / reconcile / interpolate, not rewind.
- `scripting/builtins/net_descriptor.rs::spawn_net_slot_pawn` (line 37) spawns the
  sibling `defaultWeapon` and discards its `EntityId`; return it. `netcode/mod.rs`
  (`owners.set(pawn, client_id)`) records the map.

**Key grounded findings.** (1) The netcode is prediction + reconciliation +
interpolation with NO rewind path; the fire loop mirrors movement's predict/reconcile,
it does not add lag compensation. (2) Pawn and weapon are SEPARATE entities with no
stored link — a remote pawn's sibling weapon id is spawned then discarded, so the
pawn->weapon map is the load-bearing new state. (3) Fire already rides the wire
(`fire_button` round-trips); only the APPLY was missing. (4) The wire carries yaw only
— no pitch — so today a remote shot reconstructs horizontally; the client aim lets the
host resolve the exact `aim_direction` `WeaponFireCommand` already models. Reload and
aim are the new wire fields.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
| --- | --- | --- | --- | --- | --- |
| Reload intent | `SimCommand::reload` | `InputCommand.reload` (bitcode `bool`); `WIRE_VERSION` 6->7 | n/a | n/a | n/a |
| Client aim | `SimCommand` aim (pitch beside `facing_yaw`, or aim vector) | `InputCommand` pitch/aim field; part of the 6->7 bump | n/a | n/a | n/a |
| Pawn->weapon map | new host map beside `MovementOwners` | n/a (engine-side; registry-blind net crate never sees `EntityId`) | n/a | n/a | n/a |
| Remote fire application | per-pawn `WeaponFireCommand` via `weapon::tick_resolved` | consumes `InputCommand.fire_button` + aim | n/a | n/a | n/a |
| Fire predict/reconcile | local predicted weapon tick; reconcile via `reconcile_local_pawn` loop | authoritative outcome via snapshot | n/a | n/a | n/a |

## Open questions

- **Reload-field ownership (confirmed consistent).** This spec owns `reload` on
  `SimCommand` + wire `InputCommand` and the single `WIRE_VERSION` bump; ammo's Task 5
  already defers the field and bump here and consumes `SimCommand.reload` locally, so
  exactly one bump lands. One sampling detail to keep aligned: ammo Task 5 samples
  reload as a rising edge (`ButtonState::Pressed`) so a held R does not re-attempt each
  tick; this spec's gap policy synthesizes neutral on loss — compatible, but keep the
  rising-edge sample the single source.
- **Aim representation.** Pitch scalar beside `facing_yaw` (cheapest, mirrors the
  existing field) versus a normalized aim vector. Implementer's choice under the
  constraint that the host reconstruct the client's exact `aim_direction`; the pitch
  scalar is the recommended default.
- **Reload before ammo exists.** Task 5 delivers the reload intent to the mapped
  weapon but defines no transfer (no ammo yet). The AC proves delivery, not effect —
  the intended split, with the ammo spec filling the body.
