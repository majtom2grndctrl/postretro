# E10 — Enemy Facing Slew (bounded yaw turn rate)

> **Wave:** E10 enemy-AI follow-up. Runs **after** `E10--enemy-steering-feel` — its acceptance test compares this spec's facing rate against steering's turn-rate constant, and steering's smoothed velocity heading is one of the two headings this spec slews toward.
>
> **Premise correction vs. earlier planning notes:** facing *arbitration* already exists and is correct — `crates/postretro/src/scripting/systems/ai.rs:541-564`: agents in `Alert`/`Attack` face their **velocity heading** while moving (XZ speed above `FACING_MOVE_SPEED_EPSILON`, ai.rs:77) and face the **player** when stopped-but-engaged; `Idle`/`Death` facing is untouched. This spec does not change who the agent looks at. The gap is *how fast*: the chosen rotation is snapped into `Transform.rotation` in one tick.

## Goal

Bound the enemy's yaw angular speed so target-heading switches — start/stop transitions (velocity heading ↔ player), re-aggro reversals, separation-shove flips — rotate over several ticks instead of snapping within one, making bodies read as turning, not teleporting their facing.

## Background (the cause)

- The facing block (`ai.rs:541-564`) computes a target rotation via `yaw_rotation_toward` (`ai.rs:106`; zero-length guard `MIN_XZ_LEN_SQ` at ai.rs:110; mesh forward is +Z, `MESH_FORWARD` at ai.rs:94) and writes it directly: `transform.rotation = rotation` (ai.rs:555-562). No gameplay-side slew exists.
- The renderer slerps between previous-tick and current-tick rotation (`interpolated_transform`, `crates/entities/src/registry.rs:944-960`), so any flip — including 180° — completes within one tick interval. Render interpolation smooths *between* ticks; it cannot spread a rotation *across* ticks.
- `E10--enemy-steering-feel` adds a turn-rate limit to the **velocity heading**, which indirectly smooths facing while moving. It does not cover the moving↔stopped target switch (velocity heading → player direction), stationary player-tracking, or re-aggro flips — those still snap. This spec closes that gap at the facing layer.
- No persisted facing state is needed: enemy yaw lives in `Transform.rotation`, written only by this block (plus spawn), and all rotations produced here are yaw-only — current yaw is recoverable exactly from the quaternion.
- `run_ai_tick` receives the fixed tick `dt` (`crates/postretro/src/sim/mod.rs:67`), so the per-tick clamp is `FACING_TURN_RATE * dt`.

## Scope

### In scope

- **Slew helper.** Pure function: current yaw, target yaw, max delta → new yaw, rotating along the shortest arc, clamped to the max delta, exact on arrival.
- **Integration.** In the facing block, extract current yaw from `Transform.rotation` (rotate `MESH_FORWARD` by the quat, `atan2` the XZ), slew toward the target yaw produced by the existing arbitration, write back `Quat::from_rotation_y` of the result. Arbitration, state gates, epsilon, and zero-length guards unchanged.
- **Tuning constant.** `FACING_TURN_RATE` (rad/s) as a module constant beside `FACING_MOVE_SPEED_EPSILON`, following the steering-constant precedent.
- **Cross-spec constraint.** Facing must not lag the body's own movement: `FACING_TURN_RATE` ≥ the steering max turn rate from `E10--enemy-steering-feel`, asserted by a unit test comparing the two constants.

### Out of scope

- Changing the arbitration (who to face, per state) — already shipped and correct.
- Pitch/roll; aim poses; head-look separate from body yaw.
- Turn-in-place or strafe locomotion animation (movement direction ≠ facing blending) — a future animation pass.
- Player facing/camera; anything outside the enemy facing block.

## Acceptance criteria

- [ ] When the target heading changes by a large angle (e.g. a 180° re-aggro), the agent's per-tick yaw change never exceeds the configured rate × tick dt, and yaw converges exactly to the target over successive ticks (runnable unit test on the slew helper driven across ticks).
- [ ] Slew takes the shortest arc, including across the ±π seam (e.g. from −170° to +170° rotates through 180°, not through 0°) (runnable unit test).
- [ ] Arbitration behavior is preserved: a moving `Alert` agent tracks its velocity heading, a stopped `Attack` agent tracks the player, and `Idle`/`Death` facing is untouched — with rotation now rate-limited in all cases (existing ai tests remain green; new test asserting the slewed yaw target matches the arbitration's chosen heading for both sources).
- [ ] Zero-length direction inputs leave facing unchanged, exactly as today — no NaN, no drift (existing `MIN_XZ_LEN_SQ` guard tests remain green).
- [ ] Deterministic under the fixed tick: identical inputs produce an identical yaw sequence (covered by the helper being pure and dt-driven; asserted by running the same sequence twice).
- [ ] `FACING_TURN_RATE` ≥ steering's max turn rate constant (runnable unit test comparing the two constants; documents the "body never lags its own movement" constraint).

## Tasks

### Task 1: Slew helper
Add a pure `slew_yaw(current: f32, target: f32, max_delta: f32) -> f32` beside `yaw_rotation_toward` in `ai.rs`: shortest-arc difference (wrap to ±π), clamp to `max_delta`, exact arrival when within it. Unit tests: clamped large turn converges over ticks, ±π seam crossing, exact arrival, determinism (same sequence twice).

### Task 2: Facing-block integration + constant
In the facing block (`ai.rs:541-564`), derive the target yaw from the existing arbitration's chosen direction, extract current yaw from `Transform.rotation` (rotate `MESH_FORWARD`, `atan2` XZ), apply `slew_yaw` with `FACING_TURN_RATE * dt` (dt from `run_ai_tick`'s tick parameter), and write back the yaw quat. Add `FACING_TURN_RATE` beside `FACING_MOVE_SPEED_EPSILON` with the ≥-steering-turn-rate unit test. Existing facing tests updated only where they asserted single-tick snap.

## Sequencing

**Phase 1 (sequential):** Task 1 — the pure helper everything asserts against.
**Phase 2 (sequential):** Task 2 — consumes the helper; same file.

## Open questions

- **Rate value.** Start around 2–3× the steering max turn rate (fast enough to never read as sluggish tracking during strafes, slow enough that a 180° flip visibly turns). Tune on the movement-feel fixture map's arena ring with the agent-diagnostics overlay; the constant-inequality test only pins the floor.
- **Attack-swing tracking.** A stopped attacker now tracks the player at bounded rate; if melee swings visibly miss a circling player because facing lags, the fix is a higher rate (or an attack-windup facing lock), decided from the fixture playtest — not built speculatively here.
