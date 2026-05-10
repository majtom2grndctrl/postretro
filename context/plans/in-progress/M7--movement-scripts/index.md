# Player Movement (M7)

> **Status:** ready
> **Depends on:** Collision Foundation (M7) (landed), Gravity Primitives (M7) (landed), Player Spawn (M7) (landed)
> **Related:** `context/lib/scripting.md` · `context/lib/entity_model.md` §5, §7

---

## Goal

Player movement is a built-in Rust system. A declaration script registers the `"player"` entity type with a `movement` descriptor; Rust snapshots those base parameters onto the player at spawn and drives the per-tick movement loop. All parameters are modder-configurable through `registerEntity` — no live VM involvement during movement.

---

## Tasks

### 1. `PlayerMovementDescriptor` on `EntityTypeDescriptor`

Add `movement` to the `components` block on `EntityTypeDescriptor` in `crates/postretro/src/scripting/data_descriptors.rs`, matching the existing `light` / `emitter` pattern. The Rust struct gains a new field; the JS/Lua parsers read it from the `components` sub-object alongside `light` and `emitter`:

```rust
pub(crate) struct EntityTypeDescriptor {
    pub(crate) classname: String,
    pub(crate) light: Option<LightDescriptor>,
    pub(crate) emitter: Option<BillboardEmitterComponent>,
    pub(crate) movement: Option<PlayerMovementDescriptor>,
}

pub(crate) struct PlayerMovementDescriptor {
    pub(crate) capsule: CapsuleParams,
    pub(crate) ground: GroundParams,
    pub(crate) air: AirParams,
    pub(crate) fall: FallParams,
}

pub(crate) struct CapsuleParams {
    pub(crate) radius: f32,
    pub(crate) half_height: f32,
}

pub(crate) struct GroundParams {
    pub(crate) speed: f32,
    pub(crate) accel: f32,
    pub(crate) jump_velocity: f32,
    pub(crate) step_height: f32,
    pub(crate) max_slope: f32, // degrees on the wire; converted to cosine threshold at materialization
}

pub(crate) struct AirParams {
    pub(crate) forward_steer: f32,
    pub(crate) accel: f32,
    pub(crate) max_control_speed: f32,
    pub(crate) bunny_hop: bool,
    pub(crate) jumps: u32,
    pub(crate) jump_ceiling: f32,
}

pub(crate) struct FallParams {
    pub(crate) terminal_velocity: f32,
}
```

Wire shape: `registerEntity({ classname: "player", components: { movement: { capsule: {...}, ground: {...}, air: {...}, fall: {...} } } })`. The `movement` block has four sub-objects, each grouping a related set of knobs. All fields are required when their parent sub-object is present.

**`capsule`** — collision shape.

| Field | Type | Description |
|-------|------|-------------|
| `radius` | number | Capsule radius, m. `(0, +∞)`. |
| `halfHeight` | number | Capsule half-length of the cylindrical segment (excluding hemispheres), m. Total capsule height = `2 * (halfHeight + radius)`. `(0, +∞)`. |

**`ground`** — ground locomotion.

| Field | Type | Description |
|-------|------|-------------|
| `speed` | number | Ground move speed (target velocity), m/s. `[0, +∞)`. |
| `accel` | number | Ground acceleration, m/s². Higher = quicker direction change. Quake `sv_accelerate` analogue. `[0, +∞)`. |
| `jumpVelocity` | number | Vertical velocity applied on jump (instantaneous Δv), m/s. Despite the underlying physics term, this is a velocity, not an impulse. `[0, +∞)`. |
| `stepHeight` | number | Max automatic step-up height, m. `[0, +∞)`. |
| `maxSlope` | number | Max slope angle counted as ground, degrees. `[0, 90]`. |

**`air`** — air control (mid-jump and bunny-hop physics).

| Field | Type | Description |
|-------|------|-------------|
| `forwardSteer` | number | On forward/back input, blend the input direction toward the player's facing direction before air acceleration runs. `0.0` = pure strafe-physics (input direction drives air control; allows directional speed build-up); `1.0` = full steer (you go where you look). Formula: `wishdir = normalize(lerp(input_dir, facing_dir, forwardSteer))`. `[0, 1]`. |
| `accel` | number | Air acceleration constant. Low values (~0.7) produce responsive strafe physics with directional momentum; high values (~50) produce arcade-style instant air control. `[0, +∞)`. |
| `maxControlSpeed` | number | Per-tick cap on the desired velocity magnitude while airborne, m/s. Controls how much air-control authority the player has per tick. `[0, +∞)`. |
| `bunnyHop` | boolean | When `true`, horizontal speed can accumulate past `ground.speed` through consecutive air-strafes. When `false`, horizontal speed magnitude is capped to `ground.speed` mid-air (no accumulation). |
| `jumps` | integer | Number of jumps allowed while airborne, not counting the ground jump. `0` = no air jumps, `1` = double-jump, `2` = triple-jump. `[0, +∞)`. |
| `jumpCeiling` | number | Maximum vertical velocity (m/s) at which an air jump is allowed. Player's upward `vy` must be ≤ this to trigger an air jump. `0.0` = past apex only; large positive (e.g. `100.0`) = free air jump any time; negative = must already be falling at that speed. Only meaningful when `air.jumps > 0`. `(-∞, +∞)`. |

_Air physics lineage: QW `PM_Accelerate` projection. `accel` and `forwardSteer` dial between directional-momentum and arcade feels._

**`fall`** — gravity behavior.

| Field | Type | Description |
|-------|------|-------------|
| `terminalVelocity` | number | Max fall speed (magnitude), m/s. `(0, +∞)`. |

_Type column shows script-facing types (JS Number is f64). JS path coerces to f32 via `serde_json` deserialization then `as f32` (IEEE-754 round-to-nearest-even for normal values). Lua path reads `mlua::Value::Number` and casts via `as f32` — both paths reject non-finite values (`f64::is_finite` check) before the cast. Negative or out-of-range values on bounded fields error at registration. At descriptor materialization, `ground.maxSlope` is converted once to a cosine threshold: `cos_walkable = max_slope.to_radians().cos()`; the runtime component stores the cosine, not degrees._

Both `entity_descriptor_from_js` (`data_descriptors.rs:387-445`) and `entity_descriptor_from_lua` (`data_descriptors.rs:654-720`) parse the `movement` block inside the `components` sub-object, following the same path as `light` and `emitter`. If `movement` is present, every required field must be present and finite — otherwise `registerEntity` errors at the call site, before any spawn. Per-field validation: missing required field, out-of-range value (`ground.maxSlope ∉ [0, 90]`, `air.forwardSteer ∉ [0, 1]`), negative value on a positive-domain field, non-finite value, or `air.jumps > 0` without `air.jumpCeiling` present → `entity_descriptor_from_{js,lua}` returns an error before any spawn. No clamping; the caller sees a thrown script exception.

### 2. Rust movement system

`crates/postretro/src/movement/mod.rs` (new module). At player spawn, the data-archetype sweep (`scripting/builtins/data_archetype.rs`) materializes `descriptor.movement` onto the spawned player entity — the same path that materializes `light` and `emitter` components. These are base values — the player's state with no power-ups or ability modifications applied. Gameplay systems may mutate `PlayerMovementComponent` fields directly at runtime. Respawn resets to base values by re-reading from the registry. Mid-life registry mutation does not affect a live player.

Per-tick movement loop, applied during the player update (Order 1, per `entity_model.md §5`):

- [ ] Walk on flat surfaces and on slopes within `ground.maxSlope`
- [ ] Cannot walk through walls or fall through floors
- [ ] Wall slide — project remaining velocity onto collision plane
- [ ] Step-up — automatically step over ledges up to `ground.stepHeight`
- [ ] Gravity accumulation (caller reads `ScriptCtx::gravity` and passes `gravity: f32` into `movement::tick(...)`, mirroring `scripting/systems/particle_sim.rs::tick`) + terminal velocity cap
- [ ] Jump — vertical impulse when grounded
- [ ] Air jump — if `air_jumps_remaining > 0` and `vy ≤ air.jumpCeiling`, consume one count and apply `ground.jumpVelocity`; `air_jumps_remaining` refills to `air.jumps` on ground contact
- [ ] Ground-state detection on landing (airborne → grounded transition)
- [ ] Ground locomotion — input mapped relative to player facing; same `PM_Accelerate` shape as air control with `ground.speed` as the target and `ground.accel` as the acceleration constant. (Not instant velocity-set — ground retains weight.)
- [ ] Air control — QW `PM_Accelerate` projection: `addspeed = wishspeed - dot(velocity, wishdir); accel = clamp(air.accel * dt * wishspeed, 0, addspeed)`. `air.maxControlSpeed` caps `wishspeed` per tick. On fwd/back input, `wishdir` is computed as `normalize(lerp(input_dir, facing_dir, air.forwardSteer))` before the projection runs. `air.bunnyHop` controls the speed cap branch: when `true`, `addspeed` is clamped against the *projection cap* (allowing horizontal speed to grow past `ground.speed` — bunny-hop accumulation); when `false`, also clamp post-add horizontal speed magnitude to `ground.speed` (strict cap, no accumulation).

**Collision query.** World collision uses a capsule-vs-trimesh shape cast against the `parry3d` `TriMesh` held by `CollisionWorld` (parry3d 0.17). Implement a helper in `collision/` wrapping `parry3d::query::cast_shapes` for a `parry3d::shape::Capsule` against the world mesh. Construct the capsule as `Capsule::new(Point::new(0.0, -half_height, 0.0), Point::new(0.0, half_height, 0.0), radius)` where `half_height` and `radius` come from `capsule`. `half_height` is the half-length of the cylindrical segment (excluding hemispheres); total capsule height = `2 * (half_height + radius)`. Signature:

```rust
pub(crate) fn cast_capsule(
    world: &CollisionWorld,
    pos: parry3d::math::Point<f32>,
    capsule: &parry3d::shape::Capsule,
    dir: parry3d::math::Vector<f32>,
    max_toi: f32,
) -> Option<parry3d::query::ShapeCastHit>
```

The wrapper calls `parry3d::query::cast_shapes` with `ShapeCastOptions { max_time_of_impact: max_toi, stop_at_penetration: true, ..Default::default() }`.

`ShapeCastHit` exposes `time_of_impact`, `normal1` (world-space contact normal), and `witness1` (contact point) — all three are needed for wall-slide and step-up. Entity-entity collision (player vs enemies/pickups) is deferred to a later task. World collision via `cast_capsule` is sufficient for this milestone.

**Tick rate / determinism.** Movement integrates at the fixed game-logic tick rate (semi-implicit Euler). Movement code must not use `f32::mul_add`, `std::simd`, or `#[target_feature]` — Rust's default codegen does not contract FMA, so avoiding these is sufficient to keep results consistent across macOS/Linux/Windows for the integration test.

**Movement events.** Two events, dispatched through the existing `reaction_dispatch::fire_named_event` surface — collected during the player update, drained to audio/renderer/reaction-registry after game logic completes per `entity_model.md §5`. Reactions are tag-targeted declarations; no payload is passed to script handlers. If impact-speed thresholding is needed (e.g., "hard landing"), the Rust system dispatches distinct named events rather than adding a payload channel. Any such variants are deferred until a use case demands them.

| Event | When |
|-------|------|
| `landed` | Airborne → grounded transition (edge) |
| `jumped` | Jump impulse applied (edge) |

Gameplay causation (pressure plates, crushers, damage triggers, monster-closet thresholds) is **not** an event-dispatch concern — it resolves inline from the player's collision touches during the move step, per `entity_model.md §7`. Doom's `P_TouchSpecialThing` and Quake's `touch` are the lineage. `stepped` and `wallSlide` are deferred until a use case demands them.

### 3. `content/dev/scripts/player.ts`

One `registerEntity` call registering the `"player"` entity type with base movement parameters via `PlayerMovementDescriptor`. The mod's `start-script.ts` must `import "./player"` (Luau: `require("./player")`) so registration runs during mod-init — there is no auto-scan per `scripting.md §2`.

### 4. Integration test

`#[cfg(test)] mod tests` inside `crates/postretro/src/movement/mod.rs` (in-crate, not a `tests/` integration target — `CollisionWorld::mesh` is `pub(crate)` and the test needs to construct a custom trimesh, matching the existing pattern at `collision/mod.rs:115-128`):

- Build a minimal `CollisionWorld` with a flat floor and a step-up ledge of exactly `ground.stepHeight`. Use the same constants as `player.ts`: `capsule.radius = 0.4`, `capsule.halfHeight = 0.8`, `ground.stepHeight = 0.3`. (Adjust once canonical `player.ts` defaults are pinned — see Task 3.)
- Spawn a player with a known `PlayerMovementDescriptor` using those constants.
- Feed a deterministic input sequence as explicit `(tick, input)` tuples:
  - ticks 0–9: walk forward (ground locomotion)
  - ticks 10–11: jump input (jump + airborne)
  - ticks 12–25: walk into the step-up ledge (automatic step-up)
  - ticks 26–35: walk into a wall (wall slide)
- Assert position and velocity at each tick within `1e-4` m position / `1e-3` m/s velocity. Tolerances cover semi-implicit Euler accumulated round-off; tighter would be brittle to the integrator, looser would mask bugs.

---

## Acceptance criteria

- Player walks through a PRL level: walks on flat ground and slopes within `ground.maxSlope`, `fall.terminalVelocity` cap respected on falls, no clipping through walls or floors, wall slide projects velocity onto the collision plane, step-up clears ledges up to `ground.stepHeight`, jump applies `ground.jumpVelocity` when grounded, ground locomotion uses `ground.speed` and `ground.accel` with input relative to facing, air control honors `air.forwardSteer` / `air.accel` / `air.maxControlSpeed` / `air.bunnyHop`.
- `registerEntity` with a `movement` block snapshots every field defined in the descriptor tables onto the player at spawn as a `PlayerMovementComponent` (materialized through `data_archetype.rs`, the same path Light and Emitter follow). Fields exercised by the integration test: `capsule.radius`, `capsule.halfHeight`, `ground.speed`, `ground.accel`, `ground.jumpVelocity`, `ground.stepHeight`, `ground.maxSlope`. Fields `fall.terminalVelocity`, `air.forwardSteer`, `air.accel`, `air.maxControlSpeed`, and `air.bunnyHop` are wired but covered by end-to-end walk-through rather than unit assertions in this milestone.
- `air.jumps` and `air.jumpCeiling` wired: air jumps available, gate on `vy ≤ jumpCeiling`, counter refills on landing.
- Engine errors at the `registerEntity` call site if the `movement` block is present but missing any required field, or contains a negative/non-finite value on a positive-domain field.
- `landed` and `jumped` events dispatch through `reaction_dispatch::fire_named_event`; no payload is passed to script handlers.
- Capsule-vs-trimesh helper available in `collision/`.
- Integration test passes against the documented input sequence and tolerances.
