# Player Movement (M7)

> **Status:** draft
> **Depends on:** Collision Foundation (M7) (landed), Gravity Primitives (M7) (landed), Player Spawn (M7) (landed)
> **Related:** `context/lib/scripting.md` · `context/lib/entity_model.md` §5, §7

---

## Goal

Player movement is a built-in Rust system. A declaration script registers the `"player"` entity type with a `movement` descriptor; Rust snapshots those parameters onto the player at spawn and drives the per-tick movement loop. All parameters are modder-configurable through `registerEntity` — no live VM involvement during movement.

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
```

Wire shape: `registerEntity({ classname: "player", components: { movement: { capsuleRadius, capsuleHalfHeight, ... } } })`. Required fields:

| Field | Type | Description |
|-------|------|-------------|
| `capsuleRadius` | number | Player capsule radius, m. World collision shape. |
| `capsuleHalfHeight` | number | Player capsule half-height, m. World collision shape. |
| `terminalVelocity` | number | Max fall speed, m/s |
| `moveSpeed` | number | Ground move speed, m/s |
| `jumpImpulse` | number | Vertical impulse on jump, m/s |
| `stepHeight` | number | Max automatic step-up height, m |
| `walkableAngle` | number | Max slope angle counted as ground, degrees. `[0, 90]`. |
| `forwardSteer` | number | Velocity pull toward facing on fwd/back input. `0.0` = VQ3-style, `~0.3` = arcade. `[0, 1]`. |
| `strafeAccel` | number | Air acceleration on left/right input. `~0.7` = QuakeWorld, `~50` = arcade. `[0, +∞)`. |
| `wishSpeedCap` | number | Max desired speed per tick while airborne, m/s. `[0, +∞)`. |
| `allowSpeedExceedMax` | boolean | When `true`, the QW `PM_Accelerate` projection clamps `addspeed` against the *projection cap* rather than the absolute speed cap, allowing bunny-hop accumulation. `false` = strict speed cap, no bunny-hopping. |

_Type column shows script-facing types (JS Number is f64). Engine coerces to f32 via `serde_json` deserialization then `as f32` (IEEE-754 round-to-nearest-even for normal values). Negative or non-finite values on positive-domain fields error at registration._

Both `entity_descriptor_from_js` (`data_descriptors.rs:387-445`) and `entity_descriptor_from_lua` (`data_descriptors.rs:654-720`) parse the `movement` block inside the `components` sub-object, following the same path as `light` and `emitter`. If `movement` is present, every required field must be present and finite — otherwise `registerEntity` errors at the call site, before any spawn. Per-field validation rules follow the same shape as `setFogParams` in `scripting.md §10.2`.

### 2. Rust movement system

`crates/postretro/src/movement/mod.rs` (new module). At player spawn, the data-archetype sweep (`scripting/builtins/data_archetype.rs`) materializes `descriptor.movement` onto the spawned player entity — the same path that materializes `light` and `emitter` components. The movement system then snapshots `PlayerMovementDescriptor` from the entity at spawn; respawn re-reads from the registry; mid-life registry mutation does not affect a live player.

Per-tick movement loop, applied during the player update (Order 1, per `entity_model.md §5`):

- [ ] Walk on flat surfaces and on slopes within `walkableAngle`
- [ ] Cannot walk through walls or fall through floors
- [ ] Wall slide — project remaining velocity onto collision plane
- [ ] Step-up — automatically step over ledges up to `stepHeight`
- [ ] Gravity accumulation (caller reads `ScriptCtx::gravity` and passes `gravity: f32` into `movement::tick(...)`, mirroring `scripting/systems/particle_sim.rs::tick`) + terminal velocity cap
- [ ] Jump — vertical impulse when grounded
- [ ] Ground-state detection on landing (airborne → grounded transition)
- [ ] Ground locomotion — input mapped relative to player facing; `moveSpeed` sets the ground target velocity
- [ ] Air control — QW `PM_Accelerate` projection: `addspeed = wishspeed - dot(velocity, wishdir); accel = clamp(strafeAccel * dt * wishspeed, 0, addspeed)`. `wishSpeedCap` caps `wishspeed` per tick. `forwardSteer` blends a velocity pull toward facing on fwd/back input. `allowSpeedExceedMax` controls the speed cap branch: when `true`, `addspeed` is clamped against the *projection cap* (allowing speed to grow past `moveSpeed` — bunny-hop accumulation); when `false`, also clamp the post-add horizontal speed magnitude to `moveSpeed` (strict cap, no accumulation).

**Collision query.** World collision uses a capsule-vs-trimesh shape cast against the `parry3d` `TriMesh` held by `CollisionWorld` (parry3d 0.17). Implement a helper in `collision/` wrapping `parry3d::query::cast_shapes` for a `parry3d::shape::Capsule` (axis +Y, matching engine up-axis) against the world mesh. Signature:

```rust
pub(crate) fn cast_capsule(
    world: &CollisionWorld,
    pos: Point<f32>,
    capsule: &Capsule,
    dir: Vector<f32>,
    max_toi: f32,
) -> Option<ShapeCastHit>
```

`ShapeCastHit` exposes `time_of_impact`, `normal1` (world-space contact normal), and `witness1` (contact point) — all three are needed for wall-slide and step-up. Entity-entity collision continues to use the AABB derived from capsule extents per `entity_model.md §7`.

**Tick rate / determinism.** Movement integrates at the fixed game-logic tick rate (semi-implicit Euler). Movement code must not use `f32::mul_add`, `std::simd`, or `#[target_feature]` — Rust's default codegen does not contract FMA, so avoiding these is sufficient to keep results consistent across macOS/Linux/Windows for the integration test.

**Movement events.** Two events, dispatched through the existing `reaction_dispatch::fire_named_event` surface — collected during the player update, drained to audio/renderer/reaction-registry after game logic completes per `entity_model.md §5`. Reactions are tag-targeted declarations; no payload is passed to script handlers. If impact-speed thresholding is needed (e.g., "hard landing"), the Rust system dispatches distinct named events (e.g., `landed`, `landedHard`) rather than adding a payload channel.

| Event | When |
|-------|------|
| `landed` | Airborne → grounded transition (edge) |
| `jumped` | Jump impulse applied (edge) |

Gameplay causation (pressure plates, crushers, damage triggers, monster-closet thresholds) is **not** an event-dispatch concern — it resolves inline from the player's collision touches during the move step, per `entity_model.md §7`. Doom's `P_TouchSpecialThing` and Quake's `touch` are the lineage. `stepped` and `wallSlide` are deferred until a use case demands them.

### 3. `content/dev/scripts/player.ts`

One `registerEntity` call binding the `"player"` classname to a `PlayerMovementDescriptor`. The mod's `start-script.ts` must `import "./player"` (Luau: `require("./player")`) so registration runs during mod-init — there is no auto-scan per `scripting.md §2`.

### 4. Integration test

`#[cfg(test)] mod tests` inside `crates/postretro/src/movement/mod.rs` (in-crate, not a `tests/` integration target — `CollisionWorld::mesh` is `pub(crate)` and the test needs to construct a custom trimesh, matching the existing pattern at `collision/mod.rs:115-128`):

- Build a minimal `CollisionWorld` with a flat floor and a step-up ledge of exactly `stepHeight`.
- Spawn a player with a known `PlayerMovementDescriptor`.
- Feed a deterministic input sequence (walk forward, jump, step up, slide into wall) tick-by-tick.
- Assert position and velocity at each tick within `1e-4` m position / `1e-3` m/s velocity. Tolerances cover semi-implicit Euler accumulated round-off across the test's tick count; tighter would be brittle to the integrator, looser would mask bugs.

---

## Acceptance criteria

- Player walks through a PRL level: walks on flat ground and slopes within `walkableAngle`, terminal velocity cap respected on falls, no clipping through walls or floors, wall slide projects velocity onto the collision plane, step-up clears ledges up to `stepHeight`, jump applies `jumpImpulse` when grounded, ground locomotion uses `moveSpeed` with input relative to facing, air control honors `forwardSteer` / `strafeAccel` / `wishSpeedCap` / `allowSpeedExceedMax`.
- `registerEntity` with a `movement` block snapshots every field defined in the descriptor table onto the player at spawn; respawn re-reads from the registry.
- Engine errors at the `registerEntity` call site if the `movement` block is present but missing any required field, or contains a negative/non-finite value on a positive-domain field.
- `landed` and `jumped` events dispatch through `reaction_dispatch::fire_named_event`; no payload is passed to script handlers.
- Capsule-vs-trimesh helper available in `collision/`.
- Integration test passes against the documented input sequence and tolerances.
