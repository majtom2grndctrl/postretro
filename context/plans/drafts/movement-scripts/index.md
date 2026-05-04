# Movement Scripts (M7)

> **Status:** draft
> **Depends on:** Collision Foundation (M7), Gravity Primitives (M7), Player Spawn (M7)
> **Related:** `context/lib/scripting.md`

---

## Goal

Reference player movement scripts in TypeScript and Luau, with enforced feature parity. The engine exposes collision and gravity primitives (prior plans); this plan delivers the scripts that use them. All movement parameters are modder-configurable data fields. A contract test asserts both scripts produce identical output.

---

## Tasks

### 1. Data scripts

`content/tests/scripts/player-movement-data.ts` and `player-movement-data.luau`. Each contains a `registerEntity` call for the `"player"` classname with these required fields:

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

Engine errors at entity spawn if any required field is absent.

### 2. Behavior scripts

`content/tests/scripts/player-movement.ts` and `player-movement.luau`. Each implements a `levelLoad` handler and per-tick loop. Both implement the full feature checklist:

- [ ] Walk on flat surfaces
- [ ] Walk on slopes within `walkableAngle`
- [ ] Cannot walk through walls or fall through floors
- [ ] Wall slide — project remaining velocity onto collision plane
- [ ] Step-up — automatically step over ledges up to `stepHeight`
- [ ] Gravity accumulation (from `world.getGravity()`) + terminal velocity cap
- [ ] Jump — vertical impulse when grounded; re-establishes grounded state on landing
- [ ] Strafe — move direction relative to player facing
- [ ] Air control — `forwardSteer`, `strafeAccel`, `wishSpeedCap`, and `allowSpeedExceedMax` all respected

### 3. Contract tests

`crates/postretro/tests/movement_parity.rs`:

- Construct a minimal `CollisionWorld` with a known flat floor and a step-up ledge of exactly `stepHeight`.
- Register both movement scripts in separate script runtime instances backed by that collision world.
- Feed an identical deterministic input sequence (walk forward, jump, step up, slide into wall) to both runtimes tick-by-tick.
- Assert position and velocity match at each tick within a tolerance of `1e-4` m on position and `1e-3` m/s on velocity.
- Test fails if either script skips a feature or the outputs diverge.

---

## Acceptance criteria

- Player walks through a PRL level: no clipping, wall slide, step-up, jump, gravity all work.
- TypeScript and Luau scripts produce matching results on the contract test input sequence, including all four air control parameters.
- Engine errors at entity spawn if any required physics field is absent from the data script.
- Movement scripts hot-reload during gameplay (debug build).
