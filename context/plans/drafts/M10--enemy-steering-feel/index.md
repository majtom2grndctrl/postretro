# M10 — Enemy Steering Feel (acceleration, turn-rate, arrival easing)

> **Wave:** M10 enemy-AI follow-up (refinement of the shipped `M10--enemy-ai-behavior` / `M10--pathfinding-path-following` foundation). **Deferred / larger** — the meatiest of the manual-test refinements. Surfaced in play-testing as "movement reads like a chess piece moving onto squares, not an immersive FPS chase."
>
> **Composes with:** enemy facing/orientation, the velocity-driven locomotion-animation spec (`M10--enemy-locomotion-animation`), and the existing separation pass. Build after those land so facing and idle/walk selection read the smoothed velocity.

## Goal

Give navigation agents believable locomotion dynamics — accelerate up to speed, ease into a stop, and turn at a bounded rate — instead of teleport-style instantaneous velocity that snaps to full `move_speed` toward each waypoint and snaps direction at every corner. The path itself is already string-pulled by the funnel; this is about how the agent *moves along* it.

## Background (the cause)

The steering tick sets horizontal velocity fresh and instantaneously every tick:

- `agent_steering::goal_velocity` (`crates/postretro/src/agent_steering.rs:437-471`) returns `(to_target / dist) * agent.move_speed` toward the current waypoint — **full speed in one tick**, and exactly `0` the moment there is no path or the agent has arrived. No acceleration, no deceleration, no arrival slowdown.
- `tick` (`agent_steering.rs:228`) adds `separation(...)`, clamps the combined horizontal vector to `move_speed`, then calls `collide_and_slide(...)` with that horizontal `desired` plus the vertical (gravity) component, and stores `agent.velocity = result.velocity`.
- Because the target direction jumps discretely as `waypoint_cursor` advances (`goal_velocity` lines 448-460), and speed is always full or zero, the agent visually snaps heading at waypoints and starts/stops abruptly — the "chess piece" feel. `AgentComponent::velocity` already persists across ticks, so the smoothing state has a home; it simply is not used as an integration input today.

## Scope

### In scope

- **Acceleration / deceleration.** Drive the agent's horizontal velocity *toward* the goal+separation target by a bounded change per tick (an acceleration limit, units/s²), using the persisted `AgentComponent::velocity` as the integration state, instead of assigning the target velocity directly. Ramp up from rest and ramp down when the target speed drops.
- **Arrival easing.** Scale the goal speed down within an arrival band of the *final* waypoint so the agent eases to a stop rather than cutting from full speed to zero.
- **Turn-rate limiting.** Bound the per-tick angular change of the horizontal heading (max turn rate, rad/s) so the agent curves toward a new waypoint direction over a few ticks instead of snapping — smoothing the discrete waypoint corners.
- **Composition, preserved invariants.** Keep the separation push, the `move_speed` clamp, gravity/vertical handling, the collide-and-slide harness, determinism (fixed tick), and the replan/path-preservation behavior from `M10--pathfinding-path-following` intact. The smoothed velocity feeds facing and locomotion-animation selection.
- **Tuning surface.** Acceleration, turn rate, and arrival-slowdown radius as named steering parameters (module constants in the steering precedent of `ARRIVAL_RADIUS_FACTOR` / `SEPARATION_STRENGTH`, derived from the capsule / `move_speed` where natural).

### Out of scope

- Player movement (this is enemy/agent locomotion only).
- Path **post-smoothing** beyond the funnel string-pull (spline/corner-cut of the waypoint list) — considered but deferred; see Open questions. Turn-rate limiting is expected to deliver most of the visual win without re-shaping the path.
- Walk/run animation blending or speed-scaled playback rate.
- Promoting steering tuning to per-archetype descriptor fields (`components.ai` / a new steering block) — kept as engine constants here; descriptor promotion is a follow-up if modders need per-enemy feel.
- Dynamic obstacle avoidance beyond the existing inter-agent separation; flocking; pursuit prediction / lead-targeting.

## Acceptance criteria

- [ ] An agent starting at rest with a path does **not** reach `move_speed` in a single tick: its horizontal speed ramps up over multiple ticks and converges to `move_speed` (runnable unit test on the steering tick asserting speed at tick 1 < `move_speed` and a monotonic ramp toward it).
- [ ] An agent approaching its final waypoint **decelerates** within the arrival band — horizontal speed decreases as it nears the destination instead of holding full speed until a hard zero (runnable unit test asserting the speed profile over the final approach).
- [ ] When the goal direction changes sharply (a waypoint turn or a re-plan), the agent's horizontal **heading rotates by at most the configured turn rate per tick** — the per-tick heading-angle delta is bounded, so direction is not snapped (runnable unit test asserting the bounded per-tick heading change across a corner).
- [ ] The combined goal+separation velocity still never exceeds `move_speed`; agents still separate (do not stack into one body); gravity/grounding still resolves through `collide_and_slide`; no `NaN`/`Inf` velocity is produced from zero-length directions or a zero dt (runnable unit tests / preserved existing steering tests stay green).
- [ ] The steering tick remains deterministic under the fixed tick (identical inputs → identical motion); the replan budget and path-preservation tests from `M10--pathfinding-path-following` remain green.

## Tasks

### Task 1: Acceleration / deceleration + arrival easing
Integrate horizontal velocity toward the goal+separation target under an acceleration limit, using the persisted `AgentComponent::velocity`. Add arrival-band speed scaling in (or alongside) `goal_velocity` so the target speed tapers near the final waypoint. Touches `goal_velocity` and the velocity-assembly in `tick`.

### Task 2: Turn-rate limiting
Bound the per-tick rotation of the horizontal heading toward the (post-Task-1) target direction. Touches the same velocity-assembly path in `tick`; depends on Task 1's smoothed target so the two compose into one horizontal velocity before `collide_and_slide`.

### Task 3 (optional, measured): Path post-smoothing evaluation
Only if Tasks 1-2 leave visible corner-snap: evaluate corner-cutting / spline smoothing of the waypoint list. Spike-and-measure, not a guaranteed ship — report whether it earns its place over turn-rate limiting alone.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the smoothed-velocity integration the rest build on; shares `agent_steering.rs`.
**Phase 2 (sequential):** Task 2 — consumes Task 1's target velocity in the same integration path.
**Phase 3 (optional):** Task 3 — only on measured need after Phases 1-2.

## Rough sketch

- The integration state is already on the component (`AgentComponent::velocity`). In `tick`, compute the goal+separation **target** horizontal velocity as today, then instead of assigning it, step the current horizontal velocity toward it by `accel * dt` (clamp the delta magnitude), apply the turn-rate clamp to the heading, re-clamp to `move_speed`, and pass that to `collide_and_slide`. Vertical velocity handling is unchanged.
- Arrival easing: in `goal_velocity`, when on the final waypoint within an arrival-slowdown radius, scale the returned speed by `dist / slowdown_radius` (clamped) so it tapers to near-zero at the destination, letting deceleration finish the stop.
- Turn-rate: derive the current and target headings from the XZ vectors; rotate current toward target by at most `max_turn_rate * dt`; guard zero-length vectors (no heading → no rotation, no `NaN`).
- New constants mirror the existing steering-constant style (`ARRIVAL_RADIUS_FACTOR`, `SEPARATION_STRENGTH`): e.g. an acceleration as a multiple of `move_speed` per second, a max turn rate in rad/s, an arrival-slowdown radius as a multiple of the capsule radius.
- **Split-before-extend watch:** confirm `agent_steering.rs` line count before extending; if it is past ~800 lines, split the steering tick / helpers along existing seams (goal / separation / integration) as a behavior-preserving Phase 0 before Task 1.

## Open questions

- **Turn-rate vs. path post-smoothing.** Whether bounded turn rate alone removes the "on-rails" feel or whether corner-cutting the waypoint list is also needed — decided by Task 3's measurement, not up front.
- **Tuning home.** Engine constants now vs. per-archetype descriptor fields. Constants ship first; promote to `components.ai` (or a steering block) only if per-enemy feel is requested — that promotion crosses the Rust ↔ TS ↔ Luau ↔ wire boundary and would need its own boundary inventory.
- **Backward gravity/air interaction.** Confirm the horizontal smoothing does not interfere with the grounded/airborne vertical resolution in `collide_and_slide` (e.g. an agent knocked off a ledge) — keep smoothing horizontal-only.
