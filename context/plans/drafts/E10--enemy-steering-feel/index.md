# E10 — Enemy Steering Feel (acceleration, turn-rate, arrival easing)

> **Wave:** E10 enemy-AI follow-up (refinement of the shipped `M10--enemy-ai-behavior` / `M10--pathfinding-path-following` foundation). **Deferred / larger** — the meatiest of the manual-test refinements. Surfaced in play-testing as "movement reads like a chess piece moving onto squares, not an immersive FPS chase."
>
> **Composes with:** enemy facing/orientation, the velocity-driven locomotion-animation spec (`E10--enemy-locomotion-animation`), and the existing separation pass. Steering-feel is independent at the code seam — consumers read `velocity` through the unchanged `path_state` surface; no API change is required when smoothing lands. Landing after facing/locomotion only improves visual polish. Note: the one-tick lag (`run_ai_tick` precedes `run_agent_tick`) already applies to facing, so the smoothed velocity reaches facing/animation one tick later — acknowledged, not a same-tick guarantee.

## Goal

Give navigation agents believable locomotion dynamics — accelerate up to speed, ease into a stop, and turn at a bounded rate — instead of teleport-style instantaneous velocity that snaps to full `move_speed` toward each waypoint and snaps direction at every corner. The path itself is already string-pulled by the funnel; this is about how the agent *moves along* it.

## Background (the cause)

The steering tick sets horizontal velocity fresh and instantaneously every tick:

- `agent_steering::goal_velocity` (`crates/postretro/src/agent_steering.rs:440-472`) returns `(to_target / dist) * agent.move_speed` toward the current waypoint — **full speed in one tick**, and exactly `0` the moment there is no path or the agent has arrived. No acceleration, no deceleration, no arrival slowdown.
- `tick` (`agent_steering.rs:228`) adds `separation(...)`, clamps the combined horizontal vector to `move_speed`, then calls `collide_and_slide(...)` with that horizontal `desired` plus the vertical (gravity) component, and stores `agent.velocity = result.velocity`.
- Because the target direction jumps discretely as `waypoint_cursor` advances (`goal_velocity` lines 448-457), and speed is always full or zero, the agent visually snaps heading at waypoints and starts/stops abruptly — the "chess piece" feel. `AgentComponent::velocity` already persists across ticks, so the smoothing state has a home; it simply is not used as an integration input today.

## Scope

### In scope

- **Acceleration / deceleration.** Drive the agent's horizontal velocity *toward* the goal+separation target by a bounded change per tick (an acceleration limit, units/s²), instead of assigning the target velocity directly. Ramp up from rest and ramp down when the target speed drops. **IMPORTANT — integration-state source:** do NOT use `AgentComponent::velocity` as the integration state. In `tick` today, `agent.velocity` is overwritten by the post-collision `result.velocity` (agent_steering.rs:362), which diverges from the pre-collision desired whenever the agent grazes a wall — breaking AC1's monotonic-ramp guarantee. The integration state is the smoothed PRE-collision horizontal target velocity, carried across ticks in its own field (a new `AgentComponent` field, e.g. `steer_velocity: Vec3`). AC1's monotonic ramp is asserted over this pre-collision smoothed velocity (or on a collision-free fixture), so wall-grazing collision clamping does not falsify it.
- **Arrival easing.** Scale the goal speed down within an arrival band of the *final* waypoint so the agent eases to a stop rather than cutting from full speed to zero.
- **Turn-rate limiting.** Bound the per-tick angular change of the horizontal heading (max turn rate, rad/s) so the agent curves toward a new waypoint direction over a few ticks instead of snapping — smoothing the discrete waypoint corners.
- **Lookahead / corner anticipation.** Aim the turn-rate clamp at a short lookahead point along the remaining path corridor, not only the current waypoint center, so heading starts to rotate before a visible corner. This does not reshape the path. The target stays on the existing funnel path/corridor and falls back to the current waypoint when no lookahead segment exists.
- **Composition, preserved invariants.** Keep the separation push, the `move_speed` clamp, gravity/vertical handling, the collide-and-slide harness, determinism (fixed tick), and the replan/path-preservation behavior from `M10--pathfinding-path-following` intact. The smoothed velocity feeds facing and locomotion-animation selection. Normative order of operations in `tick`: (1) compute goal velocity (with arrival easing) toward the waypoint; (2) turn-rate-limit the GOAL heading toward the lookahead target when present; (3) add the separation push AFTER the turn-rate clamp (so separation un-stacking stays prompt and is NOT throttled by turn-rate limiting); (4) accel-limit the magnitude toward the target; (5) clamp to `move_speed`; (6) collide-and-slide.
- **Tuning surface.** Acceleration, turn rate, and arrival-slowdown radius as named steering parameters (module constants in the steering precedent of `ARRIVAL_RADIUS_FACTOR` / `SEPARATION_STRENGTH`, derived from the capsule / `move_speed` where natural). Non-degeneracy constraint: named constants must be chosen so the speed ramp spans >= 2 ticks and the arrival band spans >= 2 ticks of travel at `move_speed` — otherwise AC1/AC2 (ramp/decel over multiple ticks) are untestable.

### Out of scope

- Player movement (this is enemy/agent locomotion only).
- Path **post-smoothing** beyond the funnel string-pull (spline/corner-cut of the waypoint list). Keep it out of immediate scope unless Task 3 measurement shows turn-rate plus lookahead still leaves visible corner snap.
- Walk/run animation blending or speed-scaled playback rate.
- Promoting steering tuning to per-archetype descriptor fields (`components.ai` / a new steering block) — kept as engine constants here; descriptor promotion is a follow-up if modders need per-enemy feel.
- Dynamic obstacle avoidance beyond the existing inter-agent separation; flocking; pursuit prediction / lead-targeting.

## Acceptance criteria

- [ ] An agent starting at rest with a path does **not** reach `move_speed` in a single tick: its horizontal speed ramps up over multiple ticks and converges to `move_speed` (runnable unit test on the steering tick asserting speed at tick 1 < `move_speed` and a monotonic ramp toward it).
- [ ] An agent approaching its final waypoint **decelerates** within the arrival band — horizontal speed decreases as it nears the destination instead of holding full speed until a hard zero; the deceleration is observed over multiple ticks (the arrival band spans >= 2 ticks of travel at `move_speed`, per the non-degeneracy constraint), not a single-tick cut from full speed to zero (runnable unit test asserting the speed profile over the final approach across multiple ticks).
- [ ] When the goal direction changes sharply (a waypoint turn or a re-plan), the agent's horizontal **heading rotates by at most the configured turn rate per tick** — the per-tick heading-angle delta is bounded, so direction is not snapped (runnable unit test asserting the bounded per-tick heading change across a corner).
- [ ] On a two-segment corner fixture, lookahead targets the path ahead of the current waypoint while staying on the existing corridor/funnel path, so heading begins rotating before the waypoint handoff; with lookahead disabled or unavailable, behavior falls back to current-waypoint turn-rate limiting (runnable unit test on the target-direction helper).
- [ ] The combined goal+separation velocity still never exceeds `move_speed`; agents still separate (do not stack into one body) — separation is applied after the turn-rate clamp so un-stacking remains prompt and is not throttled by turn-rate limiting; gravity/grounding still resolves through `collide_and_slide`; zero-length XZ direction is guarded by the existing XZ-length floor (cite `yaw_rotation_toward` / `MIN_XZ_LEN_SQ` precedent at ai.rs:112); the accel and turn-rate integration multiply by `dt` (`accel*dt`, `turn_rate*dt`), so a zero `dt` yields a zero step — no guard needed; arrival-band speed scaling (`dist / slowdown_radius`) is dt-free by construction and is unaffected; the goal-velocity and `move_speed`-clamp path do not divide by `dt` and need no dt guard (runnable unit tests / preserved existing steering tests stay green).
- [ ] The steering tick remains deterministic under the fixed tick (identical inputs → identical motion); the smoothing integration reads only the agent's own persisted (pre-collision) `steer_velocity` and the frozen position snapshot — never a neighbor's mid-tick-updated velocity — preserving the order-independence the separation pass already guarantees; the replan budget and path-preservation tests from `M10--pathfinding-path-following` (located in `crates/postretro/src/agent_steering/tests.rs`) remain green.

## Tasks

### Task 1: Acceleration / deceleration + arrival easing
Add `steer_velocity: Vec3` to `AgentComponent` as the pre-collision smoothed integration state; integrate it toward the goal+separation target under an acceleration limit. Add arrival-band speed scaling in (or alongside) `goal_velocity` so the target speed tapers near the final waypoint. Touches `AgentComponent`, `goal_velocity`, and the velocity-assembly in `tick`. **Construction plumbing:** initialize `steer_velocity` to `Vec3::ZERO` in `AgentComponent::new` (agent.rs:104) — the sole field-listing constructor that `from_nav_params` and `attach_agent` delegate to. Existing agent.rs construction tests may need the new field added.

### Task 2: Turn-rate limiting + lookahead target
Bound the per-tick rotation of the horizontal heading toward the (post-Task-1) target direction. Derive that target from a short lookahead point along the remaining path corridor when available; otherwise use the current waypoint. Touches the same velocity-assembly path in `tick`; depends on Task 1's smoothed target so the two compose into one horizontal velocity before `collide_and_slide`.

### Task 3 (spike, no ACs): Corner-feel comparison
Entry gate: manual review against `content/dev/maps/campaign-test` or an equivalent hand-built corner fixture. Compare (1) turn-rate-only targeting the current waypoint and (2) turn-rate plus lookahead targeting along the path corridor. Only evaluate spline/corner-cut path post-smoothing if both modes still leave visible corner snap. Deliverable: a written go/no-go recommendation, so the phase has a defined output even when it ships nothing.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the smoothed-velocity integration the rest build on; shares `agent_steering.rs`.
**Phase 2 (sequential):** Task 2 — consumes Task 1's target velocity in the same integration path.
**Phase 3 (optional):** Task 3 — only on measured need after Phases 1-2.

## Rough sketch

- Add `steer_velocity: Vec3` to `AgentComponent` as the pre-collision smoothed integration state (NOT `agent.velocity`, which is overwritten by post-collision `result.velocity` and would break the monotonic ramp). In `tick`, compute the goal horizontal velocity, turn-rate-limit its heading toward the waypoint/lookahead target, add separation, step `agent.steer_velocity` toward that combined target by `accel * dt` (clamp the delta magnitude), re-clamp to `move_speed`, and pass that to `collide_and_slide`. Vertical velocity handling is unchanged.
- Arrival easing: in `goal_velocity`, when on the final waypoint within an arrival-slowdown radius, scale the returned speed by `dist / slowdown_radius` (clamped) so it tapers to near-zero at the destination, letting deceleration finish the stop.
- Turn-rate: derive the current heading from the XZ velocity and the target heading from a short lookahead point along the remaining path corridor; rotate current toward target by at most `max_turn_rate * dt`; guard zero-length vectors (no heading → no rotation, no `NaN`).
- New constants mirror the existing steering-constant style (`ARRIVAL_RADIUS_FACTOR`, `SEPARATION_STRENGTH`): e.g. an acceleration as a multiple of `move_speed` per second, a max turn rate in rad/s, an arrival-slowdown radius as a multiple of the capsule radius.
- **Split-before-extend (RESOLVED):** `agent_steering.rs` is 526 lines (production code; tests live in `agent_steering/tests.rs`), well under the ~800 threshold — no pre-split needed. Task 1 and Task 2 land in place.

## Open questions

- **Turn-rate vs. lookahead vs. path post-smoothing.** Whether bounded turn rate alone removes the "on-rails" feel, whether corridor lookahead is enough, or whether corner-cutting the waypoint list is also needed — decided by Task 3's measurement, not up front.
- **Tuning home.** Engine constants now vs. per-archetype descriptor fields. Constants ship first; promote to `components.ai` (or a steering block) only if per-enemy feel is requested — that promotion crosses the Rust ↔ TS ↔ Luau ↔ wire boundary and would need its own boundary inventory.
