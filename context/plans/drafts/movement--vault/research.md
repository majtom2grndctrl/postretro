# movement--vault — research

Read at a307e3f1. Informs; does not decide. Symbols are for the executor to re-verify.

## Probe timing

- Forwarded contact has a replay cost. `WireMovementState::Sliding` carries `floor_normal` because replay needs the authoritative slope on its first replayed tick. Last tick's contact lives on `PlayerMovementComponent.last_floor_normal`, which is not on the outer wire state.
- A ledge result forwarded from tick T to tick T+1 would need the same treatment: an outer-state field paid on every snapshot, in every state. Otherwise an ack landing between probe and entry replays T+1 with no ledge and mispredicts entry.
- A pre-intent probe has no such cost. It is a pure function of tick-start pose and the collision world, as `resolve_at_tick_start` is for the environment. Replay recomputes it, and host and client agree by construction against static geometry.
- After entry, the vault state's payload carries the target, so the probe no longer drives the vault.

## Substrate facts

- `integrate_collision` sees wall hits as `CombinedCastHit` (normal, `CollisionSource::{Static, Mover(id)}`, `classification`). It keeps only the locals `last_wall_normal` and `multi_wall_contact_seen`. There is no hit point; parry witness points are dropped in `hit_from_parts`.
- `step_up_lift` runs grounded-only, with nonzero horizontal velocity: sweep forward, lift by `step_height + STEP_UP_LIFT_MARGIN`, re-sweep, then sweep down for a walkable normal. The climb limit is about `stepHeight` (0.5 in `content/dev/scripts/player.ts`); the geometric bound is about 0.55, derived from code and not tested.
- Query primitives: `cast_capsule_combined`, `cast_ray_combined`, and `clearance_probe` with its wrappers (`upward_`, `standup_`, `anchored_standup_clearance_probe`). Clearance sweeps use `stop_at_penetration: false`, and `distance <= 0` reads as clear. There is no combined `intersection_test`, so "capsule fits at P" must be a sweep from a known-clear point, such as the down-sweep landing.
- Movers: the world is built as `CombinedCollisionWorld::new(collision_world, mover_colliders, mover_tick_states)` in sim. `ground_ref_from_hit` maps a mover hit to `GroundRef::Mover`.
- NPCs: `crates/sim/src/agent` has its own static-only `collide_and_slide` and `step_up_lift`.

## Swim facts

- `environment_transition` (`swim_transition.rs`) handles both swim entry and exit at the seam:
  - Entry from any state at immersion ≥ `enter_depth`, gated by `swim_reentry_latched`.
  - The latch clears on any tick below `exit_depth`.
  - `fluid_jump_clears` requires the ballistic rise to uncover the capsule below `exit_depth`, plus headroom.
- `SwimParams.fluid_jump_velocity` is required and > 0 today. `keep_or_drop_swim_block` holds the warn-and-drop policy.

## Netcode checklist (Swim precedent, commit 3c469a87)

1. `MovementState`, `MovementStateKind`, `kind()` (foundation `player_movement.rs`).
2. `WireMovementState` variant plus `all_finite` and `has_valid_state_contract` arms, and the per-variant float-count drift guard (`net/src/wire/components.rs`).
3. `player_movement_is_finite` (`netcode/src/lib.rs`).
4. `movement_state_enum_to_wire`, `wire_to_movement_state_enum`, `movement_state_to_wire`, `merge_wire_into_movement_state_checked`, and the round-trip test `movement_state_variants_round_trip_through_wire` (`netcode/src/movement_state.rs`).
5. `WIRE_VERSION` (`net/src/handshake.rs`, 29) and `SNAPSHOT_VERSION` (`net/src/wire.rs`, 19). Update the doc comments and `networking.md` §Version gates.
6. `included_dash` in `ClientPrediction::predict_tick_with_events` (`netcode/src/prediction.rs`). Tolerances: `ORDINARY_CORRECTION_MAX_M`, `DASH_CORRECTION_MAX_M`.
7. `refresh_plan.rs`: only `Swim` survives reload.
8. `carry_live_tick_state` carries whatever the wire carries for a client tuning install.
9. A new predict/reconcile harness file, beside `predict_reconcile_harness_swim.rs`.
10. Hand-built `WirePlayerMovementState` literals in tests, if the outer struct changes. This brief adds no outer field.

Client prediction builds its own `CombinedCollisionWorld` (`client_predict_movement_tick`). Replay reads mover poses by server tick (`ReplayCollisionSource`, `MoverHistoryBuffer::pose_at_tick`). Client mover poses advance to `estimated_server_tick`, while the pawn predicts on `client_tick`, so mover-ledge vaults may mispredict and get corrected. The clock alignment was not verified.

## Fire-gate sites

- Host-local: `checked_command.can_fire &= pawn_alive` in `sim/src/sim/weapon_stage/commands/local.rs`.
- Host-remote: the `can_fire` expression in `sim/src/sim/weapon_stage/commands/remote.rs`. It already reads pawn components from the registry, and `PlayerMovementComponent` is a registry component (`trigger_system.rs` reads it).
- Client-predicted: the client's own fire command path. `weapon/client_resolution.rs` resolves an accepted shot with `can_fire: true`. Confirm where the client refuses a predicted start before resolution.
- Ordering: movement is update order 1 (`entity_model.md` §5), before the weapon stage. Confirm the host applies a remote command's movement before that command's fire.

## Descriptor and presentation sites

- Types: `PlayerMovementDescriptor`, `SwimParams` (foundation `data_descriptors/types/movement.rs`). SDK `.d.ts` / `.d.luau` are generated from `crates/scripting-core/src/typedef/`.
- Parse: `movement_descriptor_from_lua` / `_from_js` (`scripting-core/src/data_descriptors/{lua,js}/movement.rs`), `swim_block.rs`.
- Enable: `PlayerMovementComponent::from_descriptor`.
- Events: `state_entry_address`, `state_exit_address` (physics `movement/mod.rs`). The reaction table lives in `docs/scripting-reference.md`.
- Impulse: `ImpulseStates`, `state_index`, `state_params_by_index` (`postretro/src/view_feel/impulse.rs`), and the JS and Luau view-feel parsers.
- Input: `jump_pressed` is a level signal. `derive_jump_edges` and `advance_forgiveness` handle coyote time, the jump buffer, and `jump_spent`.
- Fixtures:
  - Unit worlds: `ledge_and_wall_world`, `CollisionWorld::from_triangles_for_test`, `slab`, `box_leaf`.
  - Dev maps: `content/dev/maps/movement-feel.map` (AI-focused, no ledges), `e24-environment-test.map` (water).
