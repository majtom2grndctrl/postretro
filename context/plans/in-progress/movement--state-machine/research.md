# Research notes — movement--state-machine

Code-grounding anchors confirmed against source while drafting. Ephemeral — for the implementer/reviewer to verify fast; not durable architecture.

## Movement tick partition
`crates/postretro/src/movement/mod.rs` — `tick(component, input, collision_world, gravity, dt, position) -> (Vec3, MovementEvents)`.
- Intent half = numbered steps 1–6 (gravity; jump from grounded; air-jump gated on `air_jumps_remaining > 0` && `velocity.y <= air.jump_ceiling`; grounded vs air `pm_accelerate`; airborne cap at the effective ground speed; ground friction when no input).
- Substrate half = steps 7–8: the `for _ in 0..4` sweep-and-slide loop, `step_up_lift`, `floor_push_remaining` (capped `SKIN_DISTANCE + NORMAL_NUDGE`), stuck-stop (`multi_wall_contact_seen` + `stuck_stop_threshold`), ground-stick down-cast, `is_grounded`/`air_ticks` reset, `landed` (gated on `air_ticks >= 3`) / `jumped` resolution.
- `MovementInput`: `wish_dir: Vec2`, `jump_pressed: bool`, `facing_yaw: f32`, `running: bool` (running added in the uncommitted walk/run change).
- `MovementEvents`: `landed: bool`, `jumped: bool`.

## Double-jump already works
Airborne jump branch fires when `!is_grounded && jump_pressed && air_jumps_remaining > 0 && velocity.y <= air.jump_ceiling`, using `ground.jump_velocity`. `air_jumps_remaining` resets to `air.jumps` on any tick with floor contact. So a descriptor with `air.jumps >= 1` yields a working double-jump today. Task 5 is consolidation + tests, not new mechanics.

## Component + descriptor
- `PlayerMovementComponent` (`scripting/components/player_movement.rs`): carries `capsule`/`ground`/`air`/`fall`, `cos_walkable`, `is_grounded`, `velocity`, `air_jumps_remaining`, `air_ticks`, stuck-stop fields. Materialized via `from_descriptor`. Add the state enum + dash timers here.
- `PlayerMovementDescriptor` / `GroundParams { speed: SpeedParams { walk, run }, accel, jump_velocity, step_height, max_slope }` / `AirParams { forward_steer, accel, max_control_speed, bunny_hop, jumps, jump_ceiling }` / `FallParams { terminal_velocity }` in `scripting/data_descriptors.rs`. JS parser `movement_descriptor_from_js`, Luau parser `movement_descriptor_from_lua` — symmetric validation via `validate_non_negative_finite` etc.

## Engine-internal invariant (drives Decision D1)
`entity_model.md` §7b: "Movement is purely engine-internal. Scripts cannot read or write `PlayerMovement` through `worldQuery`; the movement system owns it exclusively." §5: movement tick is update order 1 (before camera follow); events collected across ticks, drained after the tick loop.

## Input
`crates/postretro/src/input/types.rs` — `enum Action { MoveForward, MoveRight, MoveUp, LookYaw, LookPitch, Sprint, Jump, Use, Shoot, AltFire, Reload }`. No `Dash` (or `Crouch`) yet — Task 4 adds `Dash`. Default bindings in `input/defaults.rs` (e.g. `Sprint` → `ShiftLeft`).

## Event mapping
`main.rs` `run_movement_tick` maps `events.landed`/`events.jumped` → `"landed"`/`"jumped"` reaction event strings. New events extend the same way. The `sprint` bool is read in the tick loop and threaded into `run_movement_tick`.

## SDK type emission
Descriptor sub-structs are registered for type emission via `scripting/primitives/mod.rs` + name maps in `scripting/typedef.rs`, emitted to `sdk/types/postretro.d.ts` / `.d.luau` (`gen-script-types` bin; debug builds also emit at startup). Drift test in `typedef.rs` fails on mismatch. `SpeedParams` was added through exactly this path in the walk/run change — `DashParams` follows it.
