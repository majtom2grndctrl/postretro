# E16 - Host Combat Command Application

> **Status:** draft.
>
> **Epic:** 16 - Combat.
>
> **Milestone:** Resolution Modes.
>
> **Fits first:** the front edge of "server-authoritative fire + hit
> confirmation," pulled forward. The host already applies remote clients'
> MOVEMENT per pawn; this generalizes the per-pawn apply seam to FIRE and RELOAD.
> Prerequisite for co-op-correct ammo — the ammo spec depends on this for
> host-side magazine decrement and reserve transfer against the firing client's
> OWN weapon.

## Goal

Make the host authoritatively apply each remote co-op client's fire and reload
to that client's pawn, the same way it already applies their movement. Today
`host_resolve_movement_inputs` extracts `resolved.command.movement` per owned
pawn and drops `fire_button`; a remote client's fire never reaches their pawn on
the host, and reload does not exist on the wire. This spec closes that gap.

Standalone-testable without ammo: with a resourceless weapon, a remote client's
held fire resolves a host-authoritative shot from THEIR pawn's eye, at their
`facing_yaw`. Reload rides the frame to the firing pawn's weapon; the transfer
semantics are the ammo spec's.

## Scope

### In scope

- Generalize the host per-pawn command-apply path so fire and reload resolve to
  each owned pawn's weapon host-authoritatively, mirroring the movement path.
- A host-side pawn -> active-weapon map (analogous to `MovementOwners`),
  populated at slot accept from the sibling weapon `spawn_net_slot_pawn`
  currently spawns and discards.
- Capture the sibling weapon `EntityId` in `spawn_net_slot_pawn` (return it) so
  the accept path can record the mapping.
- A `reload` intent field on `SimCommand` and the wire `InputCommand`, threaded
  through `wire_convert` and the command queue, with the `WIRE_VERSION` bump.
  **This spec owns the reload field's introduction** (see Open questions).
- Per-pawn aim derivation for a remote pawn: eye origin from the pawn's
  transform + standing eye height; horizontal direction from `facing_yaw`.
- The listen host's own pawn keeps its existing camera-driven fire/aim path,
  appended alongside the resolved remote pawns (mirroring movement).

### Out of scope

- Hit confirmation, the favor-the-shooter history window, lag compensation,
  server rewind — the rest of "server-authoritative fire + hit confirmation."
- Pitch-accurate remote aim. Remote fire is yaw-only (horizontal from
  `facing_yaw`); the wire carries no pitch. A pitch/aim-vector wire field is
  deferred to the full hit-confirmation spec, which needs precise per-tick aim.
- The ammo resource, magazine/reserve semantics, and what reload transfers — the
  ammo spec owns those. This spec delivers the reload intent to the pawn's
  weapon; it does not define the mutation.
- The per-pawn ammo REPLICATION projection (ammo spec owns the downstream read).
- Client-side prediction/reconciliation of fire or reload beyond what movement
  already does. Fire and reload stay host-authoritative and arrive via
  snapshots; the client does not predict them.

## Acceptance criteria

- [ ] With a resourceless weapon (`resolution: Hitscan`, no ammo), a remote
  client sending a pressed fire button resolves a host-authoritative shot from
  THAT client's pawn: the fire originates at the pawn's eye, aimed at the pawn's
  `facing_yaw`, and hits a target placed on that ray. The listen host's own pawn
  firing is unaffected.
- [ ] Two owned pawns firing on the same tick each resolve against their OWN
  weapon (per-pawn cooldown, per-pawn credit source) - no cross-talk.
- [ ] A pawn whose owner sends no fire does not fire; the neutral/held gap policy
  synthesizes a released fire button, so a disconnected client's weapon does not
  auto-fire on held stale intent.
- [ ] The host maps every owned remote pawn to its active weapon at slot accept.
  A pawn whose descriptor declares no weapon maps to none and never fires
  host-side (logged once, not an error).
- [ ] `reload` round-trips `SimCommand` <-> wire `InputCommand` through
  `wire_convert`, survives the command queue (sanitize/hold/neutral/catch-up),
  and the host per-pawn apply reaches the firing pawn's mapped weapon carrying
  the resolving client's reload bit. A held reload lapses to neutral (false)
  under the gap policy like every other button.
- [ ] `WIRE_VERSION` is bumped; both drift-guard sides still agree. A peer built
  before this change fails Gate 1, as the handshake contract requires.
- [ ] No new `unsafe` (review/grep gate).

## Tasks

### Task 1: Reload input field and wire plumbing

Add `reload: bool` to `SimCommand` (`sim/mod.rs`) and the wire `InputCommand`
(`crates/net/src/wire.rs`). Thread it through `sim_command_to_input` /
`input_command_to_sim` (`netcode/wire_convert.rs`) and default it false in
`neutral_sim_command` (`command_queue.rs`). Bump `WIRE_VERSION`
(`crates/net/src/transport.rs`) and update both layout drift guards. `reload` is
a `bool`, so `sanitize_input_command` needs no new validation. Local input
sampling (`main.rs`, where `fire_button` is built from the action snapshot) fills
`reload` from the reload action; wire the action if absent.

### Task 2: Pawn -> active-weapon map, populated at accept

Return the sibling weapon `EntityId` from `spawn_net_slot_pawn`
(`scripting/builtins/net_descriptor.rs`) instead of discarding it. Add a
host-owned `pawn -> weapon` map beside `MovementOwners` (`command_queue.rs`), set
in the same accept path that calls `owners.set(pawn, client_id)`
(`netcode/mod.rs`) and cleared on slot close alongside `owners.remove_pawn`. A
pawn with no weapon records no entry.

### Task 3: Generalize the host per-pawn command resolve

Widen `host_resolve_movement_inputs` (movement-only) to yield the full resolved
`SimCommand` per owned pawn (a `(EntityId, SimCommand)` list) so `fire_button`
and `reload` survive alongside `movement`. Gap policy, catch-up, and cursor logic
are unchanged; only the projection out of `ResolvedCommand.command` widens from
`.movement` to the whole command.

### Task 4: Host per-pawn combat apply in the sim

Generalize `simulate_tick`'s weapon stage (`sim/mod.rs`) from one
`active_wieldable` + one `command` to a per-pawn apply, mirroring how movement
iterates `remote_pawn_inputs` then appends the local pawn. Per owned pawn:
resolve its weapon via the Task 2 map, derive aim (eye origin from the pawn
transform + `PlayerMovementComponent` standing eye height, horizontal direction
from `facing_yaw`), build a `WeaponFireCommand`, and run `weapon::tick_resolved`
for that weapon. `tick_resolved` already takes an arbitrary `active_wieldable`,
so it is reused per pawn unchanged. The listen host's own pawn keeps the
camera-driven aim path, appended last.

### Task 5: Per-pawn reload delivery seam

Route each pawn's resolved `reload` to that pawn's mapped weapon in the Task 4
apply. This spec delivers the intent to the per-pawn weapon and proves it reaches
the correct one; the weapon-side reload function and its magazine/reserve
transfer are the ammo spec's. Keep it a single call site so the ammo spec fills
the body without re-plumbing.

### Task 6: Tests

Host applies a remote pawn's fire (shot resolves from that pawn's eye at its yaw,
hits a placed target); two pawns fire independently; a no-fire pawn stays silent;
reload round-trips the wire and reaches the mapped weapon; `WIRE_VERSION` drift
guards pass. Reuse the `command_queue.rs` / `wire_convert.rs` test patterns.

## Sequencing

**Phase 1 (parallel-safe):** Task 1 (wire/field) and Task 2 (pawn->weapon map)
touch disjoint seams and can run together.
**Phase 2 (sequential):** Task 3 - consumes the widened command.
**Phase 3 (sequential):** Task 4, then Task 5 - the apply path consumes the map,
the widened resolve, and the reload field.
**Phase 4 (sequential):** Task 6 - verifies the surface.

## Rough sketch

Grounded seams (current source):

- `netcode/command_queue.rs::host_resolve_movement_inputs` (line 355) drops
  `resolved.command.fire_button`; `MovementOwners` (line 48) is the map to mirror
  for weapons; `neutral_sim_command` (line 376) gains a `reload: false` default.
- `sim/mod.rs`: `SimCommand { movement, fire_button }` (line 28) gains `reload`;
  `simulate_tick` (line 47) applies fire only through the single `active_wieldable`
  + `command.fire_button` (weapon stage, lines 109-118); `weapon_fire_command`
  (line 186) builds the aim-bearing `WeaponFireCommand`; `run_weapon_fire_tick`
  (line 224) wraps `tick_resolved`.
- `weapon/mod.rs::tick_resolved` (line 133) - the per-weapon apply, already
  parameterized on an arbitrary `active_wieldable`, reusable per pawn.
- `netcode/wire_convert.rs::{sim_command_to_input, input_command_to_sim}` (lines
  19, 46) - `fire_button` already round-trips here; add `reload`.
- `crates/net/src/wire.rs::InputCommand` (line 836) - add `reload: bool`;
  `WIRE_VERSION` at `crates/net/src/transport.rs:52` (currently 6) bumps to 7.
- `scripting/builtins/net_descriptor.rs::spawn_net_slot_pawn` (line 37) spawns the
  sibling `defaultWeapon` and discards its `EntityId` (~line 104); return it.
  `netcode/mod.rs` (~line 1168, `owners.set(pawn, client_id)`) records the map.

**Key grounded findings.** (1) Pawn and weapon are SEPARATE entities with no
stored link. The host's own pawn uses the single global `self.active_wieldable`
(`main.rs`); a remote pawn's sibling weapon id is spawned then discarded, so the
host cannot find it today - the pawn->weapon map is the load-bearing new state.
(2) Fire already rides the wire: `InputCommand.fire_button` has round-tripped
since Phase 3 (`wire_convert` comment: "Phase 5 consumes it"); only the host
APPLY was missing, so reload is the sole new wire field. (3) The wire carries
`facing_yaw` only (no pitch), so remote fire is yaw-only/horizontal; the host's
own pawn keeps full camera pitch via `camera.aim_ray()`.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
| --- | --- | --- | --- | --- | --- |
| Reload intent | `SimCommand::reload` | `InputCommand.reload` (bitcode `bool`); `WIRE_VERSION` 6->7 | n/a | n/a | n/a |
| Pawn->weapon map | new host map beside `MovementOwners` | n/a (engine-side, registry-blind net crate never sees `EntityId`) | n/a | n/a | n/a |
| Remote fire application | per-pawn `WeaponFireCommand` via `weapon::tick_resolved` | consumes existing `InputCommand.fire_button` | n/a | n/a | n/a |

## Open questions

- **Reload-field ownership (recommendation, needs confirmation).** This spec
  introduces `reload` on `SimCommand` + wire `InputCommand` and owns the
  `WIRE_VERSION` bump, because it owns the input transport-and-apply substrate and
  the ammo spec depends on it (dependency points ammo -> here). The ammo spec then
  consumes `reload` for magazine/reserve transfer and must NOT re-add the field or
  bump the wire again. Coordinate with the ammo draft so exactly one bump lands.
- **Yaw-only remote aim.** Acceptable for a boomer-shooter first cut (Doom-style
  flat aim), and enough to prove host-authoritative fire. If early co-op testing
  wants vertical aim before the full hit-confirmation spec, a pitch/aim-vector
  wire field moves up. Confirm yaw-only is acceptable for this slice.
- **Reload before ammo exists.** Task 5 delivers the reload intent to the mapped
  weapon but defines no transfer (no ammo yet). The AC proves delivery, not
  effect. Confirm this is the intended split versus deferring the reload wire
  field entirely to the ammo spec.
