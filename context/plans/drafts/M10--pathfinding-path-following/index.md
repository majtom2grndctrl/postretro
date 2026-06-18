# M10 — Pathfinding + Path Following

> **Wave:** plan 1 of 2 in the M10 closing wave (one `/orchestrate` session). Build order: **this plan → `M10--enemy-ai-behavior`**. This plan publishes the runtime steering API; the enemy-AI plan is its first real consumer. Mirrors how `M10--navigation-representation` published the region graph this plan consumes.

## Goal

Runtime pathfinding over the baked navmesh — A* across walkable regions, funnel string-pull through portal segments — plus path following that steers a movable agent through the M7 collision world toward a destination without snagging on concave walls. Completes the "movable agent" foundation and publishes the steering API the enemy AI drives.

## Scope

### In scope

- **Always-on runtime nav graph.** Build the runtime `NavGraph` whenever a loaded map carries a navmesh section, in every build — not only under `dev-tools` (today it is gated, `startup/lifecycle.rs`). The debug *overlay* stays `dev-tools`-gated; the graph and pathfinding do not.
- **Pathfinding query (pure).** A* over the region/portal graph from a start region to a goal region, then a funnel string-pull over the corridor's portal segments → an ordered list of world-space waypoints. Unreachable goal → no path. Same-region start/goal → a direct two-point path.
- **Movable agent component** — capsule geometry, velocity, grounded flag, the current path + waypoint cursor, and a destination. Engine-internal, not script-visible through `worldQuery` (the `PlayerMovement` precedent). Capsule radius/height default to the navmesh's baked agent parameters so the agent matches what the bake eroded for.
- **Agent collide-and-slide harness** — a minimal capsule sweep-and-slide (iterative slide, step-up, gravity, ground-stick) built on `collision::cast_capsule` / `cast_ray`, in a **new module** — it does not extend the 6055-line player movement module. Moves an agent along a desired horizontal velocity each tick with full world-collision response.
- **Per-tick agent steering system** — for each agent with a destination: ensure/refresh a path to it, steer toward the next waypoint, move via the harness, advance the cursor on arrival-radius, and set arrived / blocked (no-path) state. Replan policy: recompute when the destination moves past a threshold, when the agent strays off its corridor, or on a staleness interval; replans are capped per frame (a shared budget, time-sliced so a wave of agents cannot all replan in one tick).
- **Runtime steering API** (the contract plan 2 consumes): set / clear an agent's destination (a world position), read its path state (has-path / arrived / blocked, distance-to-destination) and kinematics; plus a one-shot find-path query for callers that want a path without owning an agent.
- **Debug agent + path overlay** (`dev-tools`): a chord spawns a test agent that chases the player; render its corridor + funnel waypoints in-world via the existing debug-line overlay (the `nav_diagnostics` precedent). The plan's own end-to-end demonstration; superseded by plan 2's real driver.
- **Tests** — A* (corridor correctness, unreachable, trivial same-region), funnel (straight / L-bend / single-region), agent slide (routes around a wall fixture without penetration), replan-trigger and per-frame-budget logic.

### Out of scope

- Enemy AI / FSM / attack / death, and which entity sets a destination in production — all `M10--enemy-ai-behavior` (plan 2). This plan's only destination driver is the `dev-tools` debug agent.
- Line-of-sight / visibility queries, patrol paths.
- Multi-agent local avoidance (RVO / boids), crowd separation — agents path independently.
- Dynamic navmesh updates, dynamic obstacles.
- Off-mesh links (jump / drop-down) and region hints (cover) — future navmesh portal kinds.
- Multiple agent sizes — one baked canonical agent (the navmesh records it).
- Refactoring or unifying the player movement substrate — the agent harness is its own minimal thing.
- Path post-processing beyond funnel string-pull (spline smoothing, etc.).

## Acceptance criteria

- [ ] A* over a hand-built navmesh returns the region corridor connecting two regions; an unreachable goal returns no path; a goal in the start region returns a trivial direct path (runnable unit tests on hand-built `NavMeshSection`s).
- [ ] Funnel string-pull yields a direct two-point path through a straight corridor, bends only at the inside corner of an L-shaped corridor, and returns start→goal directly for a single region (runnable unit tests).
- [ ] An agent steered into a wall fixture slides along it and never ends inside the collider; an agent steered toward a point reachable only around an L-wall reaches it without penetrating the wall (runnable unit tests against a `CollisionWorld` trimesh fixture — no GPU).
- [ ] An agent that reaches its destination reports *arrived*; an agent whose destination has no path reports *blocked* and does not walk into the wall (runnable unit test on the steering state surface).
- [ ] Path replans are bounded per frame by a fixed budget — with more agents needing a replan than the budget allows, only up to the budget recompute in one tick, the rest the next (runnable unit test on the budget logic).
- [ ] Pathfinding and agent steering work in a default build with `dev-tools` **off**: the runtime nav graph and the steering tick are not `dev-tools`-gated; only the debug overlay and debug-agent spawner are (review-gate: feature-attribute inspection).
- [ ] With `dev-tools`, the debug agent set to chase the player routes around walls and corners on `content/dev/maps/campaign-test` without clipping, its path drawn by the overlay (review-gate: manual check).

## Tasks

### Task 1: Always-on runtime nav graph

Move the runtime `NavGraph` construction out of the `#[cfg(feature = "dev-tools")]` block in `startup/lifecycle.rs` so the graph is built in every build when `LevelWorld.navmesh` is present; make the `nav_graph` field unconditional. Keep the navmesh debug overlay (`ToggleNavOverlay`, `nav_diagnostics::emit`) `dev-tools`-gated. No query-surface changes — `NavGraph::region_at` / `region_portal_iter` / `regions` / `portals` / `agent_params` (`crates/postretro/src/nav.rs`) are consumed as-is.

### Task 2: Pathfinding query (A* + funnel)

A pure pathfinding query over `NavGraph` (new module under `nav/`). A* over the region graph: nodes are regions, edges are portals (`NavPortal` joining `region_a`/`region_b`), edge cost and heuristic from portal-segment midpoints / region world centroids (`NavRegionRecord::world_min_xz` / `world_max_xz`). Resolve start and goal regions via `region_at`; outside-all-regions → no path. Produce a region corridor, then run the Simple Stupid Funnel algorithm over the corridor's ordered portal segments (`NavPortal.left` / `right`) to emit the tightest world-space waypoint list within the corridor. Output a path (waypoints) or none. No agent or collision dependency; unit-tested on hand-built sections.

### Task 3: Movable agent component + collide-and-slide harness

A new engine-internal agent component (new `ComponentKind` variant in `scripting/registry.rs`): capsule radius/height, velocity, grounded flag, current path + waypoint cursor, destination. Not exposed through `worldQuery`. Capsule defaults to `NavGraph::agent_params()` radius/height. A minimal capsule collide-and-slide harness in a new module: iterative slide via `collision::cast_capsule`, step-up reusing the `collision` step probe, gravity integration, and a ground-stick down-cast (`collision::cast_ray`) — takes a desired horizontal velocity + position + dt, returns resolved position + grounded. Does **not** touch `movement/mod.rs`. Unit-tested on a wall fixture.

### Task 4: Agent steering system + runtime steering API

A per-tick agent system (new module + a thin `run_agent_tick` wrapper in `main.rs`, hooked **after** the player movement tick — so agents chase the settled player — and **before** the weapon fire tick — so agent positions settle before hitscan reads them). Per agent with a destination: refresh the path (Task 2) under the replan policy + per-frame budget, steer toward `waypoints[cursor]`, move via the harness (Task 3), advance the cursor within an arrival radius, set arrived / blocked. The steering API (the seam plan 2 drives): set/clear destination, read path state + kinematics, and a one-shot find-path passthrough. Depends on Tasks 1–3.

### Task 5: Debug agent + path overlay (`dev-tools`)

A `dev-tools` chord (the `Alt+Shift+` namespace, alongside `ToggleNavOverlay`) spawns a test agent and sets its destination to the player each tick (the "chase me" demo). Render the active agent's corridor and funnel waypoints through the debug-line overlay (`render/nav_diagnostics.rs` / `DebugLineRenderer` precedent — all wgpu stays renderer-side). Depends on Task 4.

## Sequencing

**Phase 1 (concurrent):** Task 1 (always-on graph), Task 2 (pure A*/funnel — takes a `NavGraph` arg), Task 3 (agent component + harness) — mutually independent.
**Phase 2 (sequential):** Task 4 — the steering system + API consumes the graph (1), the path query (2), and the agent + harness (3).
**Phase 3 (sequential):** Task 5 — the debug agent + overlay consumes the steering API (4).

## Rough sketch

- Path query: `nav/path.rs` (or extend `nav.rs`) — `find_path(&NavGraph, start: Vec3, goal: Vec3) -> Option<Vec<Vec3>>`. A* over regions; Simple Stupid Funnel (Mikko Mononen) over `NavPortal` segments. Portal "left/right" handedness is already fixed by the bake (`region_a < region_b`, sorted) — verify orientation against the corridor direction when funneling.
- Agent harness: new module (e.g. `agent/` or `nav/steering.rs`) calling `collision::cast_capsule` / `cast_ray`. Model the slide loop on `movement::integrate_collision`'s structure (iterative project-and-advance, step-up, ground-stick) but standalone and agent-shaped — copy the *pattern*, not the player-coupled code.
- Steering tick: thin `App::run_agent_tick(dt)` wrapper in `main.rs` (the `run_weapon_fire_tick` precedent), iterating `registry.iter_with_kind(ComponentKind::Agent)`; system logic lives in the new module, keeping the `main.rs` delta to the wrapper + call site.
- `NavGraph` is read-only after load; the steering system borrows it from the `App`/level alongside the `CollisionWorld`.

## Boundary inventory

None. The agent component is engine-internal (the `PlayerMovement` precedent — no script surface, no FGD KVPs, `entity_model.md` §7b). The steering API is Rust-internal, consumed by plan 2's Rust AI system. No new wire / PRL / serde surface — `NavMeshSection` already exists.

## Open questions

- **Off-navmesh agents.** If an agent or its destination resolves outside all regions (`region_at` → `None`), the agent reports *blocked*. A nearest-region snap could be kinder but risks pathing into walls; deferred unless the debug demo shows agents stranding on `campaign-test`. Consumer-resolved by plan 2 (the AI decides what to do when blocked — idle, face player).
- **Funnel granularity vs. region fragmentation.** `M10--navigation-representation` flagged that single-floor-level regions fragment around steps (its Implementation Deviation). This plan's A* runs over whatever regions the bake produced; if fragmentation degrades path quality or replan cost on `campaign-test`, that is the feedback the bake's region-count logging was instrumented for — a bake-internal contour-tracer swap (out of scope there) is the remedy, not a pathfinding change.
