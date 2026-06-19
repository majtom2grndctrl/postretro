# M10 — Enemy Stuck Recovery (unwedge from concave geometry)

> **Wave:** M10 enemy-AI follow-up (refinement of the shipped `M10--enemy-ai-behavior` / `M10--pathfinding-path-following` foundation). Surfaced in manual play-testing: the enemy jams against a pillar when the player rounds it and stays immobile.
>
> **Adjacent specs:** `M10--enemy-steering-feel` (accel/turn-rate/arrival — different concern, same `agent_steering::tick` file) and `M10--enemy-locomotion-animation`. This spec is the **collision-recovery** gap; those are feel/animation.
>
> **Grounding note:** file:line anchors below are from a code-grounded investigation but predate a `/review-draft-spec` codebase-anchor pass — fact-check them at review before promotion.

## Goal

When a navigation agent's capsule wedges against concave world geometry (a pillar corner) and stops making progress despite having a valid route, detect the stall and recover — slide out and resume the chase — instead of staying frozen. Today there is **no stuck-detection anywhere in the steering loop**, so a wedged enemy is permanently and invisibly stuck.

## Background (the cause)

The wedge is a **collision-recovery gap**, not a routing failure:

- The brain re-aims the destination at the player's *raw* position every tick (`crates/postretro/src/scripting/systems/ai.rs:489-491`), and `goal_velocity` re-issues full `move_speed` straight at the current waypoint each tick (`agent_steering.rs:440-472`).
- When the capsule contacts a concave corner, `collide_and_slide` projects the velocity to ~0 — two near-perpendicular wall normals consume it within the 4-iteration budget (`agent/mod.rs:141-190`; `NORMAL_NUDGE` is only `1e-4`, `agent/mod.rs:42`). So the agent pushes into the corner and the harness correctly refuses to penetrate, yielding near-zero motion.
- Nothing in `tick` diffs the resolved position against the prior position — it writes `t.position = result.position` (`agent_steering.rs:366-370`) but never measures progress (`agent_steering.rs:228-375`). `AgentComponent` carries no `last_position` / `stuck_ticks` / progress field (`scripting/components/agent.rs:34-95`).
- Replans fire only on destination drift (`REPLAN_DEST_THRESHOLD` 0.5 m, `agent_steering.rs:67`) or staleness (~30 ticks), via `admit_replans` (`agent_steering.rs:395-432`). `blocked` stays **false** because the path is perfectly routable — the capsule simply can't physically traverse it. A fresh route still ends in a waypoint the wedged capsule must *move* to reach, so re-pathing alone does not unwedge it.
- Compounding: the navmesh erosion is coarse (Chebyshev/cell-quantized, `navmesh_bake.rs:399-441`; funnel routes to region **edges**, `nav/path.rs:244-309`), so a sub-cell sliver near pillar corners still lets the capsule make contact; and the separation pass can push neighbors *deeper* into a wedge in waves (`agent_steering.rs:481-523`).

## Scope

### In scope (Option A — the recommended, confined fix)

- **Stuck detection** in `agent_steering::tick`: track per-agent XZ progress (start-of-tick vs. resolved position) and increment a `stuck_ticks` counter when progress is below an epsilon **while the agent has a path and is not `blocked`** (i.e. it *wants* to move and has a route but isn't translating). Reset on real progress.
- **Recovery** on crossing a `stuck_ticks` threshold: (a) force a drift replan (clear the planned-destination latch so `admit_replans` recomputes a route), and (b) apply a **deterministic perpendicular tangent bias** to the desired velocity for a few ticks so the capsule slides along the obstacle out of the corner rather than grinding into it. No RNG.
- **Gating** so detection never fires for a legitimately stationary agent (idle / no path) or a genuine no-route `blocked` state — recovery must not mask a real navigation failure.

### Out of scope

- **Capsule-exact navmesh** (Option B): Euclidean radius erosion + funnel corner-offset so no waypoint sits within a radius of geometry. This touches the bake algorithm (bumps `NAVMESH_STAGE_VERSION`), region decomposition, and the funnel, and forces a re-bake of all maps. Deferred — revisit only if play-testing *after* Option A still shows visible wall-hugging. (Tracked here as the fallback, not built.)
- **Unstick inside `collide_and_slide`** (Option C, rejected): the sweep harness is intentionally intent-agnostic and cannot know which direction is "out," so recovery there would jitter. Recovery belongs in the steering tick, which owns intent.
- Player movement; inter-agent deadlock resolution beyond what the tangent slide + replan already give; flocking.
- Steering *feel* (accel/turn-rate/arrival) — `M10--enemy-steering-feel`.

## Acceptance criteria

- [ ] An agent commanded toward a target whose straight path wedges it in a concave (pillar-corner) collider — near-zero XZ progress while it holds a route and is not `blocked` — detects the stall within a bounded number of ticks and recovers: its XZ position resumes changing and it escapes the corner within a bounded number of ticks (runnable unit test on the steering tick against a concave-corner collider; asserts stuck→recovered, not just that a flag flips).
- [ ] Recovery is deterministic under the fixed tick (identical inputs → identical escape path) and is a tangent slide perpendicular to the blocked heading, not a random nudge (asserted by reproducing the same recovery twice).
- [ ] Stuck-detection does **not** fire for an agent that is legitimately stationary with no path (idle), nor does it suppress/alter a genuine no-route `blocked` outcome — gated on `has_path && !blocked` (runnable unit tests for both negative cases).
- [ ] The change is confined to `agent_steering::tick` plus one `AgentComponent` progress field; no navmesh re-bake, no `NAVMESH_STAGE_VERSION` bump, no wire/PRL/format change.
- [ ] Existing path-preservation, replan-budget, separation, and determinism tests from `M10--pathfinding-path-following` remain green.

## Tasks

### Task 1: Progress field + stuck detection
Add a progress/`stuck_ticks` field to `AgentComponent` (`#[serde(default)]`, seeded at spawn). In `tick`, compute start-vs-resolved XZ progress and update `stuck_ticks`, gated on `has_path && !blocked`. Reset on progress.

### Task 2: Tangent-slide recovery + forced replan
On threshold, clear the planned-destination latch (route the next `admit_replans`) and bias `desired` along the obstacle tangent (perpendicular to the blocked heading) for a short, fixed window so the capsule slides free. Compose with the `move_speed` clamp and separation. Consumes Task 1's detection.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the detection signal and the component field.
**Phase 2 (sequential):** Task 2 — consumes Task 1's `stuck_ticks` in the same tick; shares `agent_steering.rs`.

## Rough sketch

- Field on `AgentComponent`: e.g. `stuck_ticks: u32` (+ reuse the existing per-tick start position, or store `last_position: Vec3` if the start isn't already in hand). Seed `0` / spawn position in `from_descriptor`.
- Detection: `progress = (resolved_xz - start_xz).length()`; `if has_path && !blocked && progress < STUCK_PROGRESS_EPSILON { stuck_ticks += 1 } else { stuck_ticks = 0 }`. Epsilon sized well below a tick's expected travel (`move_speed * dt`), generous enough to ignore separation jitter.
- Recovery at `stuck_ticks >= STUCK_TICKS_THRESHOLD`: clear `planned_destination` (force replan) and add a tangent component — rotate the blocked desired heading ±90° (deterministic sign, e.g. derived from the obstacle/neighbor geometry or a fixed handedness) and blend it into `desired` for `UNSTICK_WINDOW` ticks, then re-clamp to `move_speed`. Guard zero-length headings (no NaN).
- New module constants mirror the existing steering-constant style (`ARRIVAL_RADIUS_FACTOR`, `REPLAN_DEST_THRESHOLD`).

## Open questions

- **Thresholds.** `STUCK_PROGRESS_EPSILON`, `STUCK_TICKS_THRESHOLD`, and `UNSTICK_WINDOW` values — tune during implementation against a concave-corner test fixture; should the staleness window (~30 ticks) also shrink so re-pathing reacts sooner while the player circles within the 0.5 m drift threshold?
- **Tangent sign.** How to choose the slide direction deterministically (fixed handedness vs. derived from the contact/last-good-heading) so the agent picks the *shorter* way around — resolve with the test fixture; a wrong-side choice still escapes but may look briefly awkward.
- **Option B trigger.** Whether Option A alone removes the visible problem or capsule-exact navmesh is still needed — a post-implementation play-test decision, not an up-front one.
