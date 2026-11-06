# Enemy–Mover Interaction — research

Derivation behind the brief. Prior art, engine survey and the first source read: `context/research/enemy-mover-interaction.md` (line citations there predate current source). Facts below re-read at `7dec6ed0`.

## Source facts

**Agent motion.**
- `sim/src/agent/mod.rs::collide_and_slide` sweeps static geometry only: the slide loop casts against the bare `CollisionWorld`; step-up and snap use `CombinedCollisionWorld::static_only`. Returns `SlideResult { position, grounded, velocity, overlap }`.
- `AgentComponent` (`entities/src/components/agent.rs`) holds `is_grounded: bool`, `knockback_velocity`, `blocked`, `stuck_ticks`, `unstick_window_remaining`. No yaw; facing lives in `ai/src/facing.rs`.
- `agent_steering::tick_impl` drops quiescent (zero-HP or committed-to-removal, `scripting_systems/health.rs::is_quiescent`) and inventory-owned agents from its snapshot. A corpse gets no gravity and no slide; it freezes where it died. `is_quiescent` is `pub(crate)` in `sim`; physics cannot call it.
- `tick_impl` already allocates per tick (snapshot `Vec`, `admit_replans` vectors). So does `blocking.rs::agent_capsules`.
- Two tests assert today's static-only agent and break by design: `agent/overlap_tests.rs` `an_agent_ignores_a_mover_top_its_pawn_neighbour_steps_onto` and the source-text check `the_agent_controller_holds_no_step_up_or_snap_to_floor_of_its_own`.

**Tick order** (`sim/mod.rs::simulate_started_tick_with_presentation_aim`). Movers (`run_mover_pin_pretest`, `run_kinematic_mover_tick`; table `&mut`) → Movement → Triggers → Projectiles → Ai → Traps → Steering (SteerAgents, SteerPose, SteerMovers: auto-close timers, `split_for_blocking`, `run_mover_blocking_pass`) → Weapons → death sweep. SteerPose already passes `mover_colliders` and `&*mover_tick_states` to `update_presentation_pose_inputs`; a `CombinedCollisionWorld::new(collision_world, mover_colliders, &*mover_tick_states)` in SteerAgents compiles, its shared borrow ending before `split_for_blocking`. Player movement builds the same world the same way.

**Blocking pass** (`physics/src/kinematic_mover/blocking.rs`, 861 lines).
- Agents enter as `agent_capsules` (entity, position, capsule): no ground ref, quiescent agents included.
- Stop/Reverse: any leading contact with an agent triggers the policy. The rider exemption and `RIDER_GRACE_MS` grace read `PawnBody.ground`, players only.
- Crush: any agent push-penetration counts as pinned, so a crusher damages an agent in open air.
- Displace: skipped for agents.
- `pinned_crush_contacts` is a `std` `HashSet`; damage, events and `on_impact` run in its iteration order, which varies per process.
- Pins (`pin.rs`): `pawn_bodies` reads `PlayerMovement` only. `PinWorld::sweep_is_blocked` already counts static geometry **and other movers** (relative to their previewed translation). The two-movers pin is not a gap.

**Player substrate** (`physics/src/movement/`). `substrate.rs::integrate_collision` runs carry (`mover_carry.rs::apply_mover_carry`, keyed on the previous tick's `GroundRef`) → displace (`displace_from_movers`, refused by `mover_push_is_blocked_by_static`) → combined sweep with step-up and ground-stick → `ground_ref_from_hit` → `apply_mover_release_velocity`. `GroundRef` lives in `foundation/src/movement/player_movement.rs`.

**Docked state.** `kinematic_mover.rs::mover_is_docked_closed` is public; `mover_is_at_open_terminus` / `mover_is_at_closed_terminus` are private. `goToPathNode` sets `target_segment`; `handle_arrival_at_waypoint` holds the mover there and clears it.

**Nav** (`sim/src/nav/`).
- `find_path(graph, start, goal)` is a pure function of an immutable `NavGraph`. Non-test callers: `agent_steering` replan, `ai/src/compute.rs` `target_reachable` (cached on the acquisition stride), `combat_positioning/scoring.rs` (`score_candidate`, `has_tactically_direct_target_path`). `ai` depends on `sim`, so a `sim` gate type reaches all callers.
- `region_at` filters by XZ and picks the nearest floor band, no height cap. An agent on a raised mover resolves to the floor region under it.
- Format: `NavRegion` is an axis-aligned XZ rectangle with one floor band; `NavPortal` is `{region_a, region_b, left, right}`. No flags or kind field. `NAVMESH_VERSION = 2`, fixed strides, other versions rejected. `build_pipeline.md` §Navigation bake calls off-mesh links "additive, no format break"; the format contradicts that. Runtime-built links avoid the question.
- `region_components`: DFS once at load. Sole reader `endpoints_are_topologically_connected`, the fallback in `scoring.rs::has_tactically_direct_target_path`.
- `admit_replans`: `REPLAN_BUDGET_PER_TICK = 4`, `REPLAN_STALENESS_TICKS = 30`, two passes, drift-driven agents first. Nothing dirties paths on a world change today.
- `AgentComponent.blocked` latches on a failed plan with an empty path; the agent holds. Stuck detection (`update_stuck_ticks`) requires `!blocked`; recovery after `STUCK_TICKS_THRESHOLD = 20` clears `planned_destination` and opens a tangent-bias window.

**Networking.** Agents are host-only: `Brain`, `Agent`, `Health` never attach on a client (`networking.md` §Connected-client spawn suppression). Clients render display copies at the interpolated past pose. No prediction or digest change.

**Content.**
- `mover-setpiece.map`: `setpiece_wall` (sliding, displace, ping-pong, start on spawn, seals arena from back room) and `setpiece_room` (rises 176 units at 1 m/s, displace, ping-pong; at the raised terminus its floor meets the arena floor and a west doorway opens onto the arena). No enemies today.
- `stress-env-volumes.map`: 64 `reference_enemy`, 12 movers (11 `reverse`, 1 `displace`).
- `docs/level_design.md` §Movers documents block policies for players and says "Enemies are never carried or pushed, so none of this applies to them."

**Perf.** `sim_steer_agents` is a `cpu.scope(SimStage::SteerAgents)` wall-clock scope, reported under `POSTRETRO_CPU_TIMING`. No budget test exists. Baseline from `drafts/perf-ai-tick-cost/research.md`: 2.5 ms per tick at 64 agents with ~10k particles. `alloc_probe` (`sim/src/alloc_probe.rs`) counts per-thread allocations: `AllocSnapshot::arm()` … `allocs_since()`.

## Commitments touched

- `movement.md` §6 Moving bases: carry is body-side and predicted for players; agents adopt the same order rather than a mover-side push. §6 Pinned bodies: "Agents are never carried or displaced, so nothing pins them" — superseded. "Dead pawns count" stays player-only.
- `done/E17--rider-not-a-blocking-contact` AC8 ("agents are unaffected") — reversed for riding agents.
- `done/E17--doors-blocking-movers`: crush continues past death through the damage chokepoint; no engine death concept (E16). Kept.
- `development_guide.md` §Workspace: movement and mover collision live in `postretro-physics`.
- `done/movement--static-overlap-invariant`: players and enemies share geometry primitives, not controllers.
- `done/E17--doors-as-occluders`: door→portal sealing is a baked per-mover association for BSP portals. Nav regions are a different decomposition, so the gate derives its own footprint.
- `done/gameplay-stack--ai-and-physics-crates`: physics stays a leaf crate while movement and movers are near-stable; agent motion adds churn there.
- `networking.md`: enemies host-simulated; no wire change here. Remote presentation of riding enemies is roadmap item **Co-op presentation of NPCs on moving geometry**.

## Rivals considered

- *Moving navmesh or rebake on motion.* No surveyed engine does it; lifts are off-mesh links everywhere.
- *Mover-side push (Quake 2).* Two carry semantics and two pin predicates: players must keep body-side carry because it is predicted, so a mover-side push for agents would be a second rule for the same mover.
- *Agent motion stays in `sim`, physics widens visibility.* Sharing primitives would duplicate nothing, but mover collision would then span two crates against `development_guide.md` §Workspace, and `pub(crate)` mover internals (`CombinedCastHit.source`, `deepest_mover_push_penetration`) would widen for one consumer.
- *Bake mover footprints into the nav decomposition.* Split regions at each mover's rest-pose footprint and store gates and links as baked portal kinds in a new NavMesh version; runtime toggles baked ids. Matches "baked over computed" and the `doors-as-occluders` bake pattern, and removes the fidelity measurement. Costs a `NAVMESH_VERSION` bump and rebake. Held as the fallback: the runtime cut is reversible, and the measurement decides whether it is needed.
- *Gate on live posed bounds each tick.* Re-flips for sweepers, crushers and mid-travel doors; churns replans; gates planning through a half-open door shots pass. Dock state changes only on dock and undock.
- *Local avoidance alone at closed doors.* Reproduces Source's `OnDoorBlocked` loop: jitter and stuck cycling.

## Orderings (pin table)

| Id | Scenario | Ordering | Expected |
|---|---|---|---|
| O1 | Tick order | Movers publish poses → … → SteerAgents: carry → displace → combined sweep → ground ref → release → blocking pass → Weapons | AI reads pre-carry agent poses, one tick behind the mover; documented, not fixed |
| O2 | Lift starts, reverses, stops under an agent | Carry by the tick delta at each edge | Ground stays `Mover(id)` throughout |
| O3 | Agent on a riser as it starts | Displaced up, grounded on it next tick | Rides |
| O4 | Agent on a descender | Ground-stick within reach; beyond it, gravity | No float |
| O5 | Agent steps off a moving mover | Release velocity into the knockback layer | Decays with knockback drag; steering does not wipe it |
| O6 | `reverse`/`stop` lift with an agent rider, incl. airborne grace | — | Lift never reacts to its own rider |
| O7 | `crush` lift carrying an agent through open air | — | No damage |
| O8 | `crush` pins an agent against static or another mover | — | Damage at cadence, continuing past death |
| O9 | Fast or thick crusher engulfs an agent in one tick | Inside-hull fallback | Pinned |
| O9a | `displace`/`stop` door closes on a living agent against the frame | — | Holds until the agent leaves; no damage, no embedding |
| O9b | `reverse` door closes on a living agent against the frame | Reversed course previewed | Backs off if the reverse course pins nothing; else holds |
| O9c | Agent and player pinned by one mover on one tick | — | One hold or back-off covering both |
| O10 | Several victims crushed on one tick | — | Damage, events, impact callbacks in a fixed order across runs |
| O11 | Corpse in a `reverse` or `stop` doorway | — | Door closes through or pushes it; never holds or reverses for it. `crush` still damages it |
| O11a | Corpse on a lift that starts, or under a closing `displace` door | Carry / displace as a living agent | Rides or is pushed clear; pinned corpse is left embedded, no hold |
| O12 | Door docks closed across a chaser's route | Gate flips on dock | Replan within budget, then detour or latch `blocked` and hold, no stuck cycling |
| O13 | Door leaves its closed dock | Gate clears at once | Held agent replans and passes once collision allows |
| O14 | Gate flips twice before a replan admits | — | One replan, against the current gate |
| O15 | Lift docked at the boarding terminus | — | Link open; a routed agent walks on |
| O16 | Lift leaves while an agent is mid-board | — | Carried if grounded on it; else link closes and the agent replans |
| O17 | Riding agent | — | Steering suspended, footing kept, stuck detection paused |
| O18 | Mover docks at the far terminus | — | Rider exits along the link, replans from the landing region |
| O19 | Rising room starts with enemies inside | — | They ride up and exit through the doorway onto the arena |
| O20 | Mover with no waypoints (pure spinner) | — | No gate, no link; collision and blocking pass handle it |
| O21 | Level with zero movers | — | Agent motion, steering and paths identical to today |
| O22 | `goToPathNode` holds a door closed at a mid-path node across a route | Gate flips on dock at that node | Same as O12 |
| O23 | Multi-stop lift held at a middle node; mover passes a node without stopping | — | Link open only while docked there; passing through opens nothing |
