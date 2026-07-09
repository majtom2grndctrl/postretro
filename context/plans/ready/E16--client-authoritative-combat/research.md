# Research — Client-Authoritative Combat

Investigation and feasibility notes. Decisions live in `index.md`; this file is
the source-grounding trail and the reasoning behind the settled calls.

## Grounded anchors (confirmed against source)

| Fact | Location | Confirmed |
| --- | --- | --- |
| `weapon::tick_resolved(registry, active_wieldable, command, collision, store, anim_time, dt)`; splits weapon-state half (decay cooldown, `FireMode`/`wants_fire`, gate `cooldown_remaining_ms <= 0.0`, set `= stats.cooldown_ms`) from the cast half (`fire_hitscan` -> `cast_ray` + `nearest_entity_hit`) | `crates/postretro/src/weapon/mod.rs:133`, gate at `:165`, cast at `:167` | yes |
| `WeaponFireCommand { button, aim_origin, aim_direction, can_fire }`; `WeaponImpact { point, normal, target: Option<EntityId>, zone: Option<String>, outcome }`; `ActivationOutcome::{Hit,Effect,Spawned}` | `weapon/mod.rs:47,61,27` | yes |
| `nearest_entity_hit(registry, store, anim_time, origin, direction, range)` iterates `registry.iter_with_kind(ComponentKind::Health)`, skips `current == 0.0`, reads `Transform`, prefers `zone_bearing_entry`, else authored `hitbox` AABB | `hit_zones.rs:540`, iter at `:563` | yes |
| `coarse_fallback_hit(..., hitbox: Option<&Hitbox>, zones, ...)` resolves a hit from the derived reach bound when `hitbox` is `None` — a zone-bearing model with no hitbox is still hittable | `hit_zones.rs:717,722` | yes |
| Remote-enemy materialization attaches mesh only; NONE of `Brain`/`Agent`/`Health`/`Weapon`/`PlayerMovement` | `remote_materialize.rs:38-51`; `networking.md:204` | yes |
| Interpolated pose written into the readable base `Transform` at `render_server_tick` | `client.rs::sample_into_registry:1189` -> `set_presentation_transform:1213` | yes |
| Remote-enemy models uploaded to the client `HitZoneStore` at level load (`insert_from_load`) | `startup/lifecycle.rs:1100-1104` | yes |
| Host apply seam `apply_weapon_impact_damage(registry, active_wieldable, impact)` — weapon IS already a param; only the ATTACKER is hard-coded via `local_movement_pawn(registry)` | `sim/mod.rs:250`, attacker at `:277`, zone read `:278-287` | drift, see below |
| `apply_damage_with_context(registry, id: EntityId, payload, context)` takes a NAMED target; no-ops when the target has no `Health` (`else { return }`) | `crates/entities/src/components/health.rs:333`, guard `:339` | yes |
| Host `EntityId -> NetworkId` allocator, never recycled; NO reverse map | `netcode/mod.rs:485-492` | yes |
| Client `NetworkId -> EntityId` map (`ClientReplication.map`); NO reverse map | `client.rs:116-119` | yes |
| Connected client runs `client_predict_movement_tick` then `continue`s past `simulate_tick` — no weapon code | `main.rs:1771-1816` | yes |
| `PROTOCOL_ID: u32 = 0x_5052_4C33` ("PRL3"), comment "adds the state-slot message family"; `WIRE_VERSION: u32 = 6`; both packed by `transport_protocol_id()`; `protocol_version()` builds `ProtocolVersion { app_protocol_id, wire_version }` | `crates/net/src/transport.rs:46,52,60,67` | yes |
| `ClientMessage` is a bitcode-tagged enum on the reliable Input channel — `Input(InputCommand)`, `Ack`, `BaselineRefresh`, `TimeSync`, `StateBaselineRefresh`; new kinds are appended to preserve discriminant order | `crates/net/src/wire.rs:931` | yes |
| `InputCommand { client_tick, movement: WireMovementInput, fire_button }`; `WireMovementInput.facing_yaw` is the ONLY look datum on the wire (no pitch) | `wire.rs:836,815` | yes |
| `SimCommand { movement: MovementInput, fire_button: FireButtonState }` | `sim/mod.rs:28` | yes |
| Movement-authority metadata (`local_player`, `last_processed_client_tick`) rides per-record and is valid only on records carrying `PlayerMovementState` — the model for owner-scoped facts | `wire.rs:271-289`; `networking.md:80-82` | yes |
| `host_resolve_movement_inputs(owners, queues) -> Vec<(EntityId, MovementInput)>` keeps only `.movement`; `MovementOwners` map; `neutral_sim_command()` | `command_queue.rs:355,48,376` | yes |
| `owners.set(pawn, client_id)` at slot accept | `netcode/mod.rs:1168` | yes |
| `on_slot_accepted(...) -> Option<(EntityId, NetworkId)>` is the accept intermediary; calls `spawn_net_slot_pawn` at `:184` | `netcode/lifecycle.rs:148,184` | yes |
| `spawn_net_slot_pawn` spawns the sibling `defaultWeapon` and DISCARDS its `EntityId` (bound as `weapon_id`, used only to set map-kvps, then dropped) | `scripting/builtins/net_descriptor.rs:37`, sibling spawn `:86-118` | yes |
| `PredictedTick.command: InputCommand` retains the full command; `replay` is movement-only, registry-blind (no `EntityRegistry` param) | `prediction.rs:147,426` | yes |
| `reconcile_local_pawn` is `#[cfg(test)]`; production reconcile is `reconcile_local_pawn_with_mover_history` | `reconcile.rs:52,63` | drift, see below |
| Camera `PITCH_LIMIT` = 89 deg in radians (private const); `aim_ray() -> (position, direction)` | `camera.rs:15,139` | yes |
| `PlayerMovementComponent::standing_eye_height` | `crates/foundation/src/movement/player_movement.rs:203` | yes |
| `WeaponComponent` fields `range`, `cooldown_ms`, `cooldown_remaining_ms`, `credit_source`; `effective()` -> `EffectiveStats` | `crates/entities/src/components/weapon.rs:29-40` | yes |

## Drift found and how it is handled

1. **`apply_weapon_impact_damage` already parameterizes the weapon.** The anchor
   brief said it "hard-codes `local_movement_pawn`" — accurate for the ATTACKER
   only (`sim/mod.rs:277`). The weapon (`active_wieldable`) is already a
   parameter. So Task 6's refactor threads the ATTACKER as a new parameter and
   keeps the weapon parameter, rather than parameterizing both from scratch. The
   spec's Task 6 paragraph states this precisely.

2. **`reconcile_local_pawn` is test-only.** `reconcile.rs:52` is `#[cfg(test)]`;
   the production entry is `reconcile_local_pawn_with_mover_history`. Task 7
   references the reconcile LOOP (prune-through-ack + merge-authoritative-subset)
   by behavior, not by the test-only symbol, so no AC keys on the wrong name.

3. **No pitch on the wire — the prior draft's aim field is dropped.** The earlier
   host-authoritative-fire draft added `pitch: f32` because the HOST cast the
   ray and needed the client's exact `aim_direction`. In this reshape the CLIENT
   casts the ray locally against its own rendered world, and the resolved hit
   `point` crosses the wire instead. The host validates plausibility from the
   attacker eye toward the declared point — it needs no pitch. So this spec does
   NOT add pitch; the declared `point` carries the spatial payload. The boundary
   inventory records aim/pitch as client-local (does not cross the wire) to make
   the deletion explicit.

## Feasibility reasoning

**Client-authoritative HIT is safe under this engine's threat model.** PvP and
anti-cheat are declared non-goals (`networking.md:208`, `index.md` §4). Co-op is
PvE among cooperating players, so trust-with-cheap-validation is acceptable: the
host does not re-simulate the client's shot, it sanity-checks the declaration.
The integrity surface that matters is fire-rate and ammo (how OFTEN and how MANY
shots) — which stay host-authoritative on the FIRE path — not the per-shot
geometry. That is why the `shot_id` binding (HIT-accept depends on
FIRE-authorization) is the load-bearing security check: without it a client whose
predicted fire the host rejected (cooling / empty magazine) could still declare
unbounded free hits.

**World-LOS only, never live-enemy-pose LOS.** The client aims at the
INTERPOLATED (past) enemy pose it renders (`client.rs:1210-1213`). The host is in
the present. Re-checking LOS against the live enemy pose would false-reject
legitimate shots on moving enemies — silently rebuilding the staleness miss this
whole design exists to remove, and the same reason server rewind is ruled out.
So the host validates LOS against STATIC WORLD GEOMETRY only (`cast_ray` from the
attacker eye toward the declared point, reject if a wall is nearer). Dynamic
occluders are intentionally not validated — a co-op teammate briefly eclipsing a
target is not worth a false-reject.

**Double application is impossible by construction, not by a runtime guard.**
Remote enemies carry no client-side `Health` (`remote_materialize.rs`), and
`apply_damage_with_context` no-ops on an entity with no `Health`
(`health.rs:339`). So the client's local hit query can TARGET a remote enemy
(Task 1 gives it a hittable basis) yet structurally cannot mutate its HP — it
only shows a hitmarker. Enemy HP and death live on the host and replicate down;
the client never predicts them, so there is no enemy-HP rollback path.

**Two-constant bump, one per axis.** The handshake gates on two independent
constants (`networking.md:56-64`): `PROTOCOL_ID` (message VOCABULARY) and
`WIRE_VERSION` (byte LAYOUT). This spec adds a NEW message family (the
hit-declaration `ClientMessage` variant + the owner-private per-shot ack) — that
is vocabulary, so `PROTOCOL_ID` bumps (mirroring its current "adds the state-slot
message family" note). It also adds the `reload` field to `InputCommand` and the
new messages' own byte layout — that is layout, so `WIRE_VERSION` bumps 6 -> 7.
Both drift-guard sides (net crate + engine) must be updated and both gates
asserted.

## Oversized-file watch

- `main.rs` is very large (~6.9k lines). Task 2 adds a client fire path here.
  Keep the new logic as a small helper the frame loop calls (mirroring
  `client_predict_movement_tick`), not inline growth. Not a split gate for this
  spec, but flagged.
- `sim/mod.rs` weapon stage and `hit_zones.rs` (~1k lines) are extended by Tasks
  4/6/1. Cohesive; no split required, but watch `hit_zones.rs` growth.
