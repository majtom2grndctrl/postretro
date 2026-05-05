# Player Movement (M7)

> **Status:** draft
> **Depends on:** Collision Foundation (M7), Gravity Primitives (M7), Player Spawn (M7)
> **Related:** `context/lib/scripting.md`

---

## Goal

Player movement is a built-in Rust system. A declaration script registers the `"player"` entity type with the movement parameter fields below; Rust reads them at entity spawn and drives the movement loop each tick. All parameters are modder-configurable through `registerEntity` — no live VM involvement during movement.

---

## Tasks

### 1. Entity declaration

`content/tests/scripts/player.ts` (or `.luau`). One `registerEntity` call for the `"player"` classname with these required fields:

| Field | Type | Description |
|-------|------|-------------|
| `capsuleRadius` | number | Player capsule radius, m |
| `capsuleHalfHeight` | number | Player capsule half-height, m |
| `terminalVelocity` | number | Max fall speed, m/s |
| `moveSpeed` | number | Horizontal move speed, m/s |
| `jumpImpulse` | number | Vertical impulse on jump, m/s |
| `stepHeight` | number | Max automatic step-up height, m |
| `walkableAngle` | number | Max slope angle counted as ground, degrees |
| `forwardSteer` | number | Velocity pull toward facing on fwd/back input. `0.0` = VQ3-style, `~0.3` = arcade. |
| `strafeAccel` | number | Air acceleration rate on left/right input. `~0.7` = Quake QW, `~50` = arcade. |
| `wishSpeedCap` | number | Max desired speed per tick while airborne. |
| `allowSpeedExceedMax` | boolean | Projection-capped acceleration active. `false` disables bunny-hopping. |

_Type column shows script-facing types. Engine coerces to f32/bool internally._

Engine errors at the `registerEntity` call if any required field is absent — registration fails fast, before any spawn.

### 2. Rust movement system

A Rust movement system reads movement parameters from the entity-type registry at player spawn and drives the per-tick movement loop. The system implements:

- [ ] Walk on flat surfaces
- [ ] Walk on slopes within `walkableAngle`
- [ ] Cannot walk through walls or fall through floors
- [ ] Wall slide — project remaining velocity onto collision plane
- [ ] Step-up — automatically step over ledges up to `stepHeight`
- [ ] Gravity accumulation (from the world gravity primitive) + terminal velocity cap
- [ ] Jump — vertical impulse when grounded; re-establishes grounded state on landing
- [ ] Strafe — move direction relative to player facing
- [ ] Air control — `forwardSteer`, `strafeAccel`, `wishSpeedCap`, and `allowSpeedExceedMax` all respected

Movement events emitted by the system (provisional — enumerate fully before promoting to `ready/`):

| Event | When |
|-------|------|
| `landed` | Airborne → grounded transition |
| `jumped` | Jump impulse applied |
| `stepped` | Step-up triggered |
| `wallSlide` | Velocity projected onto collision plane |

Events emitted during the player-priority tick pass are dispatched before default-priority entities tick, so world entities can respond in the same frame. Dispatch model documented in `context/lib/entity_model.md` (pending update).

### 3. Integration test

`crates/postretro/tests/movement_integration.rs`:

- Construct a minimal `CollisionWorld` with a known flat floor and a step-up ledge of exactly `stepHeight`.
- Spawn a player entity with a known set of parameter values declared via `registerEntity`.
- Feed a deterministic input sequence (walk forward, jump, step up, slide into wall) tick-by-tick.
- Assert position and velocity at each tick within a tolerance of `1e-4` m on position and `1e-3` m/s on velocity.
- Test verifies the Rust movement system honors the declared parameters.

---

## Acceptance criteria

- Player walks through a PRL level: no clipping, wall slide, step-up, jump, gravity all work.
- Movement system reads all eleven parameters from the entity-type registry and respects them at runtime.
- Engine errors at `registerEntity` call time if any required physics field is absent from the declaration.
- Integration test passes against a known input sequence with the documented tolerances.
