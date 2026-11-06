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

**Bake order.** `level-compiler/src/pipeline.rs` encodes the KinematicGeometry section (movers, waypoints) before the navmesh stage; `bake_navmesh` reads static geometry only today, cached under `NAVMESH_STAGE_VERSION`. KinematicGeometry already carries closed-mover portal associations for the camera flood (`doors-as-occluders`), the precedent for baked per-mover nav data.

**Docked state.** `kinematic_mover.rs::mover_is_docked_closed` is public and conservative: it requires `!was_active_this_tick` and `!blocked`, and `was_active_this_tick` stays true through a started ping-pong mover's end-wait (test: an end-wait is not docked). `mover_is_at_open_terminus` / `mover_is_at_closed_terminus` are private and read index and elapsed time only. `terminus_events` fire Opened/Closed only at the two ends. `goToPathNode` sets `target_segment`; arrival there sets `completed` and clears it. A pretest Hold sets `blocked` with the mover still at its waypoint.

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

**Perf.** Stage scopes are wall-clock, reported only under windowed `POSTRETRO_CPU_TIMING`; headless output is byte-identical with timing on or off, so no automated budget test exists. The work lands in three stages: `sim_movers` (pin pretest), `sim_steer_agents` (agent motion, gate, links), `sim_steer_movers` (blocking pass). Baselines from `drafts/perf-ai-tick-cost/research.md` at 64 agents: `sim_steer_agents` 2.5 ms, `sim_steer_movers` 0.33 ms; particle load alone moves `sim_steer_movers` 0.07 → 0.32 ms. A 0.1 ms bound sits inside that noise, so the brief carries a tripwire (combined growth above 0.8 ms stops for the owner) rather than an ideal. Expected new cost per agent per tick is a mover broadphase scan per cast plus mesh casts only near a mover, and per-mover pin previews only for bodies in reach. `alloc_probe` (`sim/src/alloc_probe.rs`) counts allocations only where the sim test build installs its counting allocator.

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
- *Load-time gate over the static nav mesh* (the brief's first shape). Pose each mover's local AABB at its waypoints at load, inflate by agent radius, gate the regions and portals it overlaps. Rejected (owner, 2026-11-06) after review: a door mid-region gates a whole region, which cannot honor crossings-only without a side-of-door test (a same-region start and goal take a straight line with no portal test); one AABB per hollow mover gates every region around the rising room and traps its riders; the decomposition's fit was unknown and needed a measure-first stop. Baking splits regions at the real geometry and removes all three.
- *Split the physical half from navigation.* Rejected: doors that stop enemies physically but are not gated in planning reproduce the push-against-the-door jitter loop rejected under local avoidance alone. Collision and gating land together.
- *Split lift links into a follow-up brief.* This brief would ship solid, ground, crush and door gating only. Rejected (owner, 2026-11-06): the roadmap item names lifts and the rising room, links reuse the gate's dock predicate, and the rising-room setpiece is the motivating case.
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
| O11 | Corpse (quiescent: zero HP or queued despawn) in a `reverse` or `stop` doorway | — | Door closes through or pushes it; never holds or reverses for it. `crush` still damages it |
| O11a | Corpse on a lift that starts, or under a closing `displace` door | Carry / displace as a living agent | Rides or is pushed clear; pinned corpse is left embedded, no hold |
| O12 | Door docks closed across a chaser's route | Gate flips on dock | Replan within budget, then detour or latch `blocked` and hold, no stuck cycling |
| O13 | Door leaves its closed dock | Gate clears at once | Held agent replans and passes once collision allows |
| O14 | Gate flips twice before a replan admits | — | One replan, against the current gate |
| O15 | Lift docked at the boarding terminus | — | Link open; a routed agent walks on |
| O16 | Lift leaves while an agent is mid-board | — | Carried if grounded on it; else link closes and the agent replans |
| O17 | Riding agent | — | Steering suspended, footing kept, stuck detection paused |
| O18 | Mover docks at the far terminus | — | Rider exits along the link, replans from the landing region |
| O19 | Rising room starts with enemies inside | — | They ride up and exit through the doorway onto the arena |
| O20 | Mover with no waypoints, or with authored spin | — | No gate, no link; collision and blocking pass handle it |
| O21 | Level with zero movers | — | Agent motion, steering and paths identical to today |
| O22 | `goToPathNode` holds a door closed at a mid-path node across a route | Gate flips on dock at that node | Same as O12 |
| O23 | Multi-stop lift held at a middle node; mover passes a node without stopping | — | Link open only while docked there; passing through opens nothing |
| O24 | Ping-pong mover waiting at a waypoint (`wait_ms` > 0, still started) | Dock read from phase: at the waypoint, zero velocity, wait running | Docked for the whole wait: a door gates, a lift links. The occluder's settle-tick and active-tick tests do not apply |
| O25 | Mover docked at level load (never arrived), or docked at a middle node by `goToPathNode` | Gate and links derive from dock state every tick, not from Opened/Closed arrival events, which fire only at the two ends | A door closed at load gates from the first tick; a middle-node hold gates and links |
| O26 | Docked mover commanded away (start, reverse, `goToPathNode`, auto-close) but still at its waypoint: not yet ticked, or held there by a pin | Command → pretest hold → mover never leaves | Gate and link stay as docked until the mover leaves the waypoint; a hold or a pending command alone undocks nothing |
| O27 | Mover touches a waypoint without stopping: `ping_pong` with `wait_ms` 0, a wait shorter than one tick, a tick ending exactly on a node it passes | A waypoint reached and left within one tick's motion is no dock | No gate flip, no link, no replan |
| O28 | Gate closes on an agent's current path; its replan finds no route | Gate flip → admitted replan → no path | A kept path that crosses a gated portal is dropped and `blocked` latches; a failed replan whose path crosses no gate keeps it, as today |
| O29 | N agents, more than the replan budget, route through one door when it docks closed | Gate flip → drift-priority marks → budgeted admissions per tick | Every agent detours or latches `blocked` within ⌈N / budget⌉ ticks of the flip; none fires stuck recovery more than once after it |
| O30 | Agent or goal in a gated region, clear of the door body | Gate flips with start or goal inside it | Start plans out; goal reached from its own side; only crossings through the door are refused |
| O31 | Mover docks or undocks on tick N | Gate update runs once, after the mover tick and before Ai | Ai reachability, combat-position scoring and the SteerAgents replan read one gate generation on tick N |
| O32 | Gate flips between acquisition strides | `targetReachable` cached on the stride | Reflects the flip by the brain's next acquisition-due tick, never later; steering may latch `blocked` first |
| O33 | Idle agent (no destination) on a lift or riser | Idle settle path runs the same carry → displace → sweep → ground ref → release | Rides as a steering agent does |
| O34 | Lift leaves while a rider is mid-exit | — | Still grounded on the lift: carried, stays in Ride, exits at the next dock with a link toward its goal. Grounded on the landing: replans from there |
| O35 | `stop` halts a lift between waypoints with a rider | — | Rider stays in Ride and holds; exits at the next dock after `start` |
| O36 | Rider knocked or displaced off mid-ride | Ground leaves the mover | Ride ends; steering and stuck detection resume; replans from where it lands |
| O37 | Agent already on a mover when it starts, not boarded through a link | Ground becomes a moving mover | Enters Ride as a boarded agent does |
| O38 | Rider mid-ride with a moving goal | — | No replan admitted mid-ride; the first replan after exit starts from the landing region |
| O39 | Living agent killed on tick N (crush in the blocking pass, or Weapons) while another mover pins it | Liveness read once at pass start | The pinning mover holds on tick N; the corpse rule applies from the tick N+1 pretest |
| O42 | Agent latched `blocked` inside a non-crush mover's course when the mover comes onto it | Pin → hold → agent unlatched, steers clear of the course | The mover completes its travel and docks; the agent then boards or replans |
| O43 | Gate flips while other agents' target drift fills the replan budget | Gate-marked agents admitted ahead of target drift; an undock marks every blocked agent | O29's bound holds under contention; blocked agents replan within ⌈N / budget⌉ ticks of an undock |
| O45 | Route crosses a docked mover's top between two regions linked at the same waypoint (a lift docked flush with a floor, the rising room's roof at its lowered dock) | Walked as floor while docked; Ride starts only if the mover leaves with the agent on it (O37) | Steering stays live across the top; no agent stops on a docked mover to wait for it to move |
| O46 | Mover reaches a waypoint mid-tick and rests past the tick's end (wait, finished run, `goToPathNode` hold) | The arrival tick moved, so it is no dock; the dock starts on the first tick that begins and ends at rest | A wait covering no whole tick never docks (O27); a finished run or hold docks from the tick after arrival |
| O47 | Pinned agent becomes a corpse on tick N while its pinning mover is held | Pretest adds yields only; the tick N+1 blocking pass releases | No new hold or onset on tick N+1; the mover stays held through the tick N+1 mover tick, releases in that pass, and moves from tick N+2 |
| O48 | Ai evaluates a rider mid-ride on an acquisition-due tick | Reachability holds its boarding-tick value until exit (owner, 2026-11-06) | Never resolves the rider to the floor region under the shaft; recomputes after exit |
| O49 | Level unloads and another loads, with a door docked and gating in the first | Gate, links and generation rebuild from the new level's movers and graph before their first read | No bit, link or dock memory carries over |
| O50 | `reverse` mover docked at a middle node (`goToPathNode` hold), commanded on, its course pinning a body | Pretest back-off: reversed course moves and pins nothing | Leaves the node the other way with its target cleared; gate and links there clear that tick, as O13. At a terminus the reversal goes nowhere, so it holds and stays docked (O26) |
| O41 | Start and goal on opposite sides of a docked door; agent or goal beside it | Baked split puts the door across portals; gated portals refuse crossings | No path through the door; each side plans normally; a kept path crossing it is dropped |
| O44 | Hollow mover docked (room, cage, railed lift) | Gate tests the agent capsule against the posed solid mesh per portal | Only portals its walls block are gated; its doorway stays crossable |
| O51 | Crush-immune agent (health resists the crush type at 0) under a crusher | Crusher never yields; zero damage | Passes through unharmed; displacement frees it on the first unblocked tick |
| O52 | Corpse pinned in a closed `stop` door is revived | Living from the revive tick | The door holds while it pins the agent; the agent steps clear (O42) |
| O53 | Agent at a lift's boarding landing, goal reached only from its far landing, lift docked at the boarding waypoint | Open boarding link, then a ride edge to the waypoint the lift next docks at on its own | Path boards, rides, exits at the far landing; a lift that moves only on a trigger or script offers no ride edge |
| O54 | Rider's planned exit is a waypoint the mover passes without docking, or the mover docks and holds where the rider has no exit (`goToPathNode`, completed `once` run) | Dock with no planned exit | Ride ends at that dock; rider replans from that waypoint's links, then exits or latches `blocked` on the mover; no ride outlasts one full cycle |
| O55 | Agent boards a lift docked mid-wait, at least 3 s of wait left | Boarded; mover still docked | Holds on the lift: no stuck recovery, no walk-off; Ride begins when the lift leaves |
| O56 | Agent on one walkable surface of a docked mover with several (rising room floor and roof) | Resolve only through links whose boarding edge lies at its surface | No route over the roof from inside; holds through the wait and rides up |
| O57 | Mover with a spin axis and zero authored speed, spun by `setSpinRate` | Spinner-capable from its authored axis | No gate or link at a pose the bake did not pose |
| O58 | Only a mover changes between two builds on a warm stage cache | NavMesh stage key covers mover geometry, origin, path and spin inputs | Cache miss; rebake equals a cold bake |
| O59 | Goal, combat slot or move target whose capsule overlaps a docked door's posed solid | — | Combat scoring rejects such a slot; no agent presses into a docked door |
| O60 | v3 gate or link names an absent mover, a waypoint past its path, or an out-of-range portal or region; or mover spawn fails | Checked after mover spawn | Bad records dropped with a warning; spawn failure turns every gate and link off |
| O61 | Living agent pinned by a non-crush mover with no reachable point outside its course (a pit the lift fills) | Nudge finds no clear point | Yields as a corpse: mover proceeds and docks; agent left embedded, unharmed, pushed out when unblocked |
| O40 | Agent walks into a closed mover; the mover opens; an unpinned agent sits in a `displace` sweep | Combined sweep, then displace | Stops at the skin; passes once open; pushed clear |
