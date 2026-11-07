# E17--enemy-mover-interaction — plan of record

mode: resumable
status: approved
read at: 1ace6e3ec
branch: claude/e17-enemy-mover-interaction

Source since the brief's `read at` (7dec6ed0) changed only model import and ghost rendering. Every symbol cited by Decisions and Path was re-read at `1ace6e3ec`. No Decision premise is false.

## Corrections

- **Bake portal sweep** — Path: "sweeping the baked agent capsule across it against the posed mesh." → `postretro-physics` is a dev-dependency of `level-compiler`, so no capsule cast exists in production bake code. Planning around it by testing each candidate portal against posed convex brushes: reuse `mover_light_gate/pose.rs` (`path_waypoints`, `PosedBrush::translated`), inflate each brush's planes by agent radius, and clip the capsule axis's swept quad against them, one lane per cell along the portal. A portal is gated when no lane passes. Same contract, no new dependency.
- **Posing** — `mover_light_gate/pose.rs` poses termini only. → The bake needs every waypoint, so it walks `path_waypoints` itself. Authored brushes sit at waypoint 0, so pose k is a translate by (waypoint k − waypoint 0). The spinner test is the same `spin_axis != [0; 3]` as `pose.rs`.
- **Auto-close in the ride edge** — Path: the ride edge opens from live phase "including any resolved `auto_close_ms` countdown." → `arm_opened_termini` already parks a `ping_pong` mover with `started = false, completed = true`, and `Stop` clears `started`. Phase alone covers every parked case, so the gate never reads `MoverAutoCloseTimers`, which is `RefCell` host state outside the mover table. Ride edge predicate: `mode == PingPong && started && !completed && target_segment.is_none()`.
- **Dock predicate needs no new state** — `current_linear_velocity` is `tick_delta / dt`, written every tick, zero under a hold and at spawn. `segment_elapsed_ms <= EPS && velocity == 0` therefore reads "began and ended this tick at rest on waypoint `segment_index`", which gives O24–O27 and O46 without memory or `terminus_events`.
- **Quiescence** — Path: "physics needs its own quiescence test." → `is_depleted` is a one-line health read and `DeferredEffectComponent` lives in `entities`, so the single definition (`is_depleted`, `is_terminally_committed_to_removal`, `is_quiescent`) moves down to `entities::components::health`, which physics and `sim` both read; `sim` re-exports it. No second copy to drift.
- **`find_path` takes more than the gate** — Path lists the gate alone. → Link resolution (O56) needs the mover surface an agent stands on, so the start input carries the agent's `GroundRef` too. Callers: `agent_steering`, `ai/src/compute.rs`, `combat_positioning/scoring.rs`. `bake_navmesh(&geo, &params)` stays as the no-mover wrapper for `nav/bake_contract_tests.rs`.
- **Row 27 dump scope** — the row wants a byte-identical headless dump, but `is_grounded` becomes `ground` by Decision and the dump serializes `AgentComponent`. → The dump filters to `Transform` (`DumpSpec.component`), per tick, so the bytes stay comparable and motion stays what is proved. New `AgentComponent` fields use `skip_serializing_if`.
- **Row 27 and corpses** — the row's first clause says agent motion in a mover-free level stays identical, but the Decision *Corpses ride, never block* gives every corpse gravity, in every level. Today a corpse freezes where it died. → Read the row as covering living agents; the proof run has no deaths. A corpse killed in the air now falls in mover-free levels too. Approved by owner with the plan.
- **Corpse knockback** — the Decision lists "gravity, carry and displacement" and leaves knockback unsaid. → The corpse branch clears `knockback_velocity` and adds no release velocity, so a killing blow does not start a slide that today's frozen corpse never showed. Approved by owner with the plan.
- **`AgentComponent` on the wire** — `ComponentKind::Agent` maps to a wire kind id, but no codec encodes the component body. Replacing the field changes no wire bytes. Confirms Decision *Host-only*.
- **`to_section_for_test`** — the `ai` test macro rebuilds `NavGraph` through this adapter, so it must carry the gate and link tables or `ai` tests lose them silently.
- **Descender row** — row 2's "a descender beyond ground-stick reach drops it" cannot apply to a grounded rider: carry follows the mover at any speed, as for a player. Read as a body above a descender with no floor in reach, which falls under gravity. Proved by `an_agent_above_a_descender_beyond_ground_stick_falls_under_gravity`.
- **"A hold always ends" covers pins** — the Decision sits under *A pinned enemy steps clear*. An unpinned `stop` contact is no pin: an authored `Freeze` agent standing clear in a `stop` door's sweep holds the door as a player standing there does (O6, and the non-goal *Keeping enemies out of a door's course*). `agent_yields` records pins only. **To flag in the report.**
- **Inventory-owned agents** stay out of the blocking pass entirely, crush included; the `held_world_participation_*` tests forbid crushing held items.
- **Corpse-yield marks** also skip the `stop`/`reverse` contact reaction, or `stop` would hold anyway. Marks drop under `crush`.
- **Bake reuse of `pose.rs`** — `navmesh_bake` compiles in both `lib.rs` and `main.rs` but `mover_light_gate` is bin-only, and `sim` tests need the lib. `PosedBrush`, `path_waypoints` and `BROKEN_PATH_REASON` move to `mover_pose.rs`; stage logic moves to `navmesh_bake/stage.rs`.
- **Brush inflation is horizontal only** (`radius * |n_xz|`). Full inflation lifts a lift top into the agent band and lowers a raised door's underside, falsely gating its open pose. The band already gives the vertical allowance.
- **Bake limits, accepted** — a gap narrower than the capsule yet wide enough to keep walkable cells (about 0.58–0.8 m beside a wall) stays in one region, so no portal gates it and collision stops the agent; a door filling its frame is unaffected. A region boundary parallel to a docked solid within radius + half a cell gates, over-gating by at most 0.125 m. Links do not check for a floor gap between rim and landing.
- **Bodies inside a hull** — contact reads the nearest face from inside, so its normal points deeper in: a trailing face dragged an engulfed body along, and a bottom face read as a floor to ride. Agents now take `displace_out_of_hull` first (pushed out through the nearest face, or left in place when static geometry blocks it), and a mover hull a body stands inside is no ground. Agent-only, so the player path stays bit-identical. A thin door closing through a corpse can therefore pop it out the far side; `docs/level_design.md` must not promise "left inside" unconditionally.
- **Escape walk** — an embedded body's sweeps stop at time zero, so the walk treats the mover it is embedded in as absent (`escape_slide`). A body merely touching or riding the mover keeps it solid. Clear points cover the whole course, not just the remainder, so a `ping_pong` mover's return leg is covered.
- **Headless runs** — row 27 assumes `--headless` works. → The `observability` build did not compile at HEAD (`SimCommand` lost `assist_held` to revive-assist) and needs debug assertions (`scripting_core::watcher`), so no release headless build exists. Fixed the one field in Task 1; dumps come from `cargo build -p postretro --features observability`, debug profile, before and after.
- **`movement-feel` carries 15 agents**, not 4; six chase the scripted player in the baseline run, so the dump exercises steering.
- **Row 36 "before"** needs no measurement yet. `stress-env-volumes.prl` is unbuilt and `stress-env-volumes-baseline.prl` predates the Nov 5 PRL bump, so a cold bake is due either way. The pre-change release `postretro.exe`, `prl-build.exe` and `scripts-build.exe` (built at `105bbec03`) are saved in the session scratchpad `before/`; rebuild from that commit if lost. Task 10 bakes with them for version 2 and with the current tools for version 3 (warm cache misses only nav and pack), then runs both engines back to back on one machine state.
- **Row 37 nav time** — the nav stage runs ahead of every bake-heavy stage and takes under 1.2 s to reach cold on all four maps. Baselines are in `baseline/nav-report-before.json`, taken by timing a cold bake to the `NavMesh:` log line and stopping it. The `nav_seconds` field is below log-flush resolution; `reached_nav_at` is the comparable figure.

## Delegated answers

- Region split granularity and link joining tolerances — split on `nav_cell_size`, spans merging only under an identical pose list; a span counts as covered when a brush reaches the cell square across the band [floor + step, floor + agent height], with 1e-4 m slack. Gate lanes: one per cell at cell centres, sweeping half a cell either side of the portal, so a door thinner than a cell always meets a portal around its footprint. Links join within `step_height` + 1e-4 vertically and agent radius + 2.5 cells horizontally (1.025 m at defaults): erosion can set a landing back as far as radius + 2.21 cells, so the first estimate of + 1.5 missed some rim alignments. The runtime `region_snap_tolerance` is unchanged.

## Design notes

**Pinned-agent step clear** (the brief fixes the behavior, not the mechanism). The blocking pass and pretest record each living agent they yield to in `MoverBlockingState`, host-only. SteerAgents reads that and runs a sibling `agent_steering/pin_escape.rs`: search ring points around the agent for a capsule placement outside the mover's whole course, then steer there at `move_speed`, ignoring motion verb, `control` and knockback scale, with `blocked` dropped. Reach is 1.25 s of the agent's own travel. Search runs on the pin edge only, so it is off the per-tick path. No clear point, or a walk still unfinished after 90 ticks, marks the (agent, mover) pair yielded, the pin test skips it, and the mover proceeds (O61). The mark clears once the agent is out of contact.

**Links in A\*.** A link adds a virtual node per (mover, waypoint). `find_path` walks open links as extra edges, and an open ride edge joins two nodes of one lift. The funnel runs only up to the first ride. `NavPath` then carries a planned ride (mover, boarding waypoint, exit waypoint, exit link), and the agent replans after exit (O18). A walk across a flush lift (O45) stays one funnel.

**Gate ownership.** `NavGraph` stays immutable once installed; it holds per-portal and per-region record lists, validated after mover spawn (O60). Live state is a per-mover dock table inside `MoverTickStateTable`, which every `simulate_tick` caller already passes and which each level rebuilds (O49): docked waypoint and ride readiness, written in the mover tick for movers with records only, plus a generation bumped when one changes. `sim/nav/gate.rs` is a `Copy` read view over that table, so Ai, scoring and SteerAgents read one generation per tick (O31) with no separate update pass or per-call-site `&mut` threading. Replaces the earlier `&mut NavGate` sketch; same contract.

## Hot paths

| Path | Constraint | Measured by |
|---|---|---|
| SteerAgents mover contact | No per-tick allocation; zero-mover cost is the empty-slice early-outs; mesh casts only past the bound-sphere cull | Row 35, row 36 |
| Gate update | Once per tick, bit flips on dock edges only, no allocation, no work for movers without records | Rows 25, 35 |
| Replans | Existing `REPLAN_BUDGET_PER_TICK`; gate tier reorders admission and adds no budget | Rows 9, 10 |
| Pin pretest and blocking pass | `any_body_in_reach` bounds per-mover work now that bodies include agents | Row 36 |
| Nav bake | Domain is non-spinner movers × waypoints × portals near their posed bounds; stage-cached | Rows 31, 37 |

## AC-to-proof

| AC | Row (short) | Proof | Status |
|---|---|---|---|
| 1 | Closed mover stops; opens; `displace` pushes (O40) | `sim` agent_steering: `agent_stops_at_closed_mover_and_passes_once_open`, `displace_mover_pushes_unpinned_agent_clear` | achievable as stated |
| 2 | Carry within 1e-4 m; descender; release decay (O2–O5, O33) | `sim` lift scenario, moving and idle agent, across start, reverse, stop | achievable as stated |
| 3 | Riders and grace exempt; non-rider triggers (O6) | `physics` blocking_tests | achievable as stated |
| 4 | Non-crush pin holds or backs off; one decision (O9a–O9c) | `physics` pin_tests, agent and agent+player | achievable as stated |
| 5 | Crush only pinned or engulfed; past death (O7–O9) | `physics` blocking_tests | achievable as stated |
| 6 | Crush order, ≥4 victims, ≥8 runs (O10) | `physics` test over repeated runs, comparing order | achievable as stated |
| 7 | Corpses and queued despawn; killed-while-pinned timing (O11, O11a, O39, O47) | `physics` pin_tests + `sim` corpse-on-lift; `fixed_tick_queued_despawn_quiesces_brain_agent_steering` rewritten | achievable as stated |
| 8 | Latch `blocked`, spread ≤ 0.05 m/5 s, stuck ≤ 1, pass ≤ 2 s; one replan; kept path (O12–O14, O28) | `sim` gate scenario | achievable as stated |
| 9 | 16 agents within ⌈16/budget⌉ ticks (O29) | `sim` gate scenario | achievable as stated |
| 10 | Contention on `stress-env-volumes` (O43) | Routine `sim` fixture at N=64 with drift filling the budget, plus an `#[ignore]` harness over the compiled map, run once at the gate | achievable as stated |
| 11 | Lift returning onto blocked or `Freeze`, knockback 0; pit (O42, O61) | `sim` scenarios, 2 s bound | achievable as stated |
| 12 | No path through a docked door; both sides plan; hollow room (O28, O30, O41, O44) | `sim` nav/gate tests over a real bake | achievable as stated |
| 13 | `targetReachable` tracks gate; one generation (O31, O32) | `ai` test | achievable as stated |
| 14 | Mid-exit, mid-shaft, knocked off, riser, no mid-ride replan (O34–O38) | `sim` mover_link tests | achievable as stated |
| 15 | Rider holds `targetReachable` (O38, O48) | `ai` test | achievable as stated |
| 16 | Route through lift; no ride on parked or trigger lift; shaft safety; ride bounds (O15–O18, O35, O53, O54, O62, O63) | `sim` mover_link tests | achievable as stated |
| 17 | Holds through ≥3 s wait (O15, O17, O55) | `sim` mover_link test | achievable as stated |
| 18 | Rising room lowered: no roof route, rides up, exits (O19, O44, O56) | `sim` test over a real bake of a room fixture | achievable as stated |
| 19 | No combat slot overlapping a docked door (O59) | `sim` combat_positioning test | achievable as stated |
| 20 | Walks across a flush lift (O37, O45) | `sim` mover_link test | achievable as stated |
| 21 | Unload and reload starts clean (O49) | `sim` gate test | achievable as stated |
| 22 | Gate and link every tick of a wait; stop on leaving (O24, O46) | `physics` dock predicate + `sim` gate test | achievable as stated |
| 23 | Door spawned closed gates from tick one (O25) | `sim` gate test | achievable as stated |
| 24 | Held at dock keeps gate; `reverse` undocks at mid-node (O26, O50) | `sim` gate test | achievable as stated |
| 25 | Sweeper and mid-travel door: no generation change (O27) | `sim` gate test over a full cycle | achievable as stated |
| 26 | `goToPathNode` node gates and links; pass-through does not (O22, O23) | `sim` gate test | achievable as stated |
| 27 | Spinner none; zero-mover dump identical (O20, O21, O57) | Bake test, then headless `Transform` dump of `movement-feel` (0 movers, 4 agents) against the Task 1 baseline. See Corrections | achievable as stated |
| 28 | Player suites pass; recorded mover run bit-identical | Golden trace test pinned in Task 1 before any move, plus existing movement and predict/reconcile suites | achievable as stated |
| 29 | Source-text gate covers mover-contact primitives | `the_agent_controller_holds_no_step_up_or_snap_to_floor_of_its_own`, moved and widened | achievable as stated |
| 30 | v3 round-trip; v2 load + one warning; deterministic bake; empty tables; no wire/digest diff | `level-format` tests, `level-compiler` bake test, grep gate at preflight | achievable as stated |
| 31 | Mover edits miss the stage; result equals cold (O58) | `level-compiler` stage-key test | achievable as stated |
| 32 | Faulty records dropped; spawn failure disables all (O60) | `sim` gate test with log capture | achievable as stated |
| 33 | Crush-immune, corpse impact cadence, revived corpse (O51, O52) | `physics` + `sim` tests | achievable as stated |
| 34 | Agent-on-lift determinism | `an_agent_on_a_lift_replays_identically` in `agent_steering/mover_contact_tests.rs` | achievable as stated |
| 35 | New paths allocate nothing | `alloc_probe` test in `sim`, with the one-allocation control window | achievable as stated |
| 36 | Cost tripwire, 0.8 ms combined | Windowed `POSTRETRO_CPU_TIMING` before and after; baseline captured in Task 1 | manual, executor runs and reports |
| 37 | Bake report on four maps | Region, portal, gate and link counts plus nav bake time; baseline in Task 1 | manual, executor runs; owner's terminal if a bake exceeds 2 h |
| 38 | `mover-setpiece` in-engine | Owner, in-engine | manual |
| 39 | Enemy-movers map; README checks | Owner, in-engine | manual |
| 40 | `docs/scripting-reference.md` wording | Doc edit, owner review | manual |
| 41 | `docs/level_design.md` §Movers wording | Doc edit, owner review | manual |

## Tasks

Sequential on the feature branch: the machine is disk-bound, each task needs compile feedback, and the seams share `AgentComponent` and `sim/mod.rs`. Each row below is one commit with its `plan.md` update. A delegated worker gets the full brief, `research.md`, routed docs and the Acceptance list, and neither commits nor edits this file.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Thin slice. Pin baselines first: golden player mover trace, `movement-feel` `Transform` dump, row 37 "before" numbers. Move `collide_and_slide` to `physics::movement::agent_motion`, generalize `mover_carry.rs` to body-agnostic primitives, make `AgentComponent` ground a `GroundRef`, thread `CombinedCollisionWorld` into `agent_steering::tick`, pause stuck detection while riding. Falsify the premise with agent-on-lift, door-press and stuck-loop tests. Rows 1, 28, 29 | integrating executor | — | done: golden trace bit-identical after the move; door, displace, lift and stuck-loop tests in `agent_steering/mover_contact_tests.rs` pass; gate widened |
| 2 | Agent mover contact complete: idle branch, release into knockback, corpse motion branch and single quiescence test, rewrite the two tests that assert static-only agents, determinism scenario. Rows 2, 34 | integrating executor | 1 | done: `agent_steering/corpse.rs`, `settle_corpse`; `mover_contact_tests` covers rows 2, 34 and the corpse half of row 7; library gate green |
| 3 | Blocking pass: agent ground refs, rider grace, pins for living agents, crush predicate with engulfment, ordered crush effects, corpses for crush only, O39/O47 timing. Split `blocking.rs` first if it would pass ~1000 lines. Rows 3–7 | worker (physics) | 2 | done: `blocking_tests`, `pin_tests`, `mover_pin_tick_tests` cover rows 3–7; ai 247, netcode 1014, physics 511, sim 1878 passed; seven tests that asserted overlap-only crush rewritten with a real pin |
| 4 | Pinned-agent step clear and yield-as-corpse fallback (see Design notes). Rows 11, 33 | integrating executor | 3 | done: `kinematic_mover/pin_escape.rs`, `agent_steering/pin_escape.rs`; `mover_pin_tick_tests` covers rows 11 and 33 |
| 5 | NavMesh version 3: tables, `from_bytes`, v2 decode, one rebake warning, `to_section_for_test`, section literals. Row 30 (format half) | worker (format) | 1 | done: `navmesh::tests`, 241 level-format tests passed |
| 6 | Bake: pose non-spinners per waypoint, split regions, sweep portals for gates, derive links, widen the stage key, bump `NAVMESH_STAGE_VERSION`. Rows 27 (spinner), 30, 31, 37 | worker (compiler) | 5 | done: `navmesh_bake::mover_tests`; prl-build 2325 and lib 694 passed; ai 247, level-loader 330, sim 1879 passed on the merged tree |
| 7 | Runtime gate: dock predicate, `sim/nav/gate.rs`, gated `find_path`, kept-path drop, replan tier, reachability, `region_components` as superset, combat slot rejection, load validation and reset. Rows 8–10, 12, 13, 19, 21–26, 32 | integrating executor | 2, 5 | |
| 8 | Links and rides: A* edges, ride edge phase, `agent_steering/mover_link.rs`, link-based region resolution, `targetReachable` hold. Rows 14–18, 20 | integrating executor | 6, 7 | |
| 9 | Docs and proof content: `docs/level_design.md`, `docs/scripting-reference.md`, `mover-setpiece` enemies, enemy-movers map and README. Rows 38–41 | worker (docs/content) | 8 | |
| 10 | Close the proofs: `alloc_probe` window, row 27 dump comparison, `stress-env-volumes` harness, rows 36 and 37 "after". Rows 10, 27, 35–37 | integrating executor | 9 | |

Stop conditions during the build: Task 1 failing its falsification tests goes to the owner, since it undercuts shared primitives. Row 36 growth above 0.8 ms stops with the profile. Any new false Decision premise blocks.
