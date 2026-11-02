# Enemy–Mover Interaction

## Goal

Enemies treat movers as solid and as ground, as players already do. Closed doors and walls block them and their routes. Lifts, platforms and a room rising into place carry them. Sweepers push them, and crushers kill them only when they are actually pinned. This is the movement half of "movers are solid"; shots and sight are `drafts/E17--movers-block-shots-and-sight/`. Design research: `context/research/enemy-mover-interaction.md`, whose line citations predate current source. This spec re-grounds them.

## Scope

### In scope

- One body-side mover-contact substrate (carry → displace → combined sweep → ground ref → release velocity) shared by player and agent movement, in `postretro-physics`.
- Agents move against static + mover geometry, carry a ground reference instead of a grounded flag, and suppress stuck recovery while riding.
- Blocking pass treats agents like players: rider exemption and airborne grace, pin detection and yields (a non-crush mover holds or backs off instead of embedding an enemy), crush only under `crush` and only when pinned or engulfed, deterministic crush order, corpses ignored as obstructions.
- Nav gate: a mover docked at a terminus blocks the nav regions and portals it fills there; paths, reachability and replans respect it.
- Mover links: load-time links from a mover's walkable top at each terminus to the adjacent nav regions; enemies board a docked mover, ride, and exit at the far terminus. This covers lifts and a room that rises into place.
- Human docs: a movers section covering block policies for players **and** enemies, nav gating and lifts.
- A manual proof map with enemies against a door, a lift, a crusher, a sweeper and a rising room.

### Out of scope

- **Remote presentation of riding enemies** (an enemy ground ref on the wire, `WIRE_VERSION` bump). Owned by the roadmap item **Co-op presentation of NPCs on moving geometry** (Epic 17). Until it lands, a riding enemy renders off its mover on remote clients by roughly RTT/2 plus interpolation delay times mover speed, about 30–50 cm at 150 ms RTT on a 3 m/s lift. Host and listen-server views are exact.
- Enemies opening doors or summoning lifts. Triggers stay player-driven. Which enemies may operate doors, and what the rest do at a closed door, is the roadmap item **Enemy door operation** (Epic 17).
- A costed "wait at door" nav link. Closed doors are impassable to planning; enemies hold or detour. Enemy door operation revisits this.
- Bake-time nav region splits at mover footprints, and any NavMesh format change. Gate fidelity is measured first (Task 4).
- Two closing movers pinning a body between them. Neither the player nor the agent pin predicate counts another mover as static; the gap is shared and unchanged.
- Carrying an agent's yaw on rotating movers. Agent facing re-derives from steering.
- Agents using teleporters.

## Direction

**Problem.** Agent motion (`collide_and_slide`) sweeps the static trimesh only and keeps a bare grounded flag, so nothing an enemy does knows a mover exists. The nav graph is baked without movers and has no runtime gate or link.

**Prior commitments.**

- `movement.md` §6 and the shipped player substrate: carry is body-side, keyed on the previous tick's ground reference, and runs before displacement. Agents adopt the same order rather than a mover-side push (Quake 2 style), because player carry is predicted and must stay body-side. Research §2.
- `movement.md` §6 Pinned bodies: "No mover except `crush` leaves a player pawn pinned … Agents are never carried or displaced, so nothing pins them." **Extended** to agents: the premise no longer holds once agents are carried and displaced, so agents take the same pin test and yields. Owner rule (2026-10-30): normal doors never crush players or NPCs; `crush` movers crush both alike.
- `done/E17--rider-not-a-blocking-contact` AC8: "An enemy (agent) overlapping a Stop/Reverse/Crush mover still triggers the mover's policy — agents are unaffected." **Reversed** for agents riding the mover: a rider never stops or reverses its own mover. Agents in the sweep but not riding still trigger the policy.
- E17 crush decision: crush damage continues past death through the damage chokepoint. **Kept.** Corpses stay crush victims but stop counting as stop/reverse obstructions.
- `development_guide.md` §Workspace: movement and mover collision live in `postretro-physics`. Agent body motion moves there instead of physics widening `pub(crate)` APIs for `sim`.
- `networking.md`: enemies are host-simulated; clients never run `Agent`. No prediction or digest change. Nav gating derives from data already in the level content digest.
- `build_pipeline.md` §Navigation bake: off-mesh links are "future portal kinds — additive". This spec derives links at load time instead, so no format change happens yet. That keeps the door open for a baked link kind later.

**Alternatives rejected.**

- *Moving navmesh, or rebaking regions on mover motion.* No engine surveyed does this; lifts are always off-mesh links (research §2).
- *Keep agent motion in `sim` and widen physics visibility.* It spreads mover-contact rules across two crates and duplicates the player's carry, displace and pin logic.
- *Gate on the mover's live posed bounds every tick.* It re-flips for anything crossing a region's height band (sweepers, crushers, mid-travel doors), churning replans against the budget, and it gates planning through a half-open door that shots and sight pass through. Dock state changes only on dock and undock; a door finishing its close is held by collision until the gate flips.
- *Rely on local avoidance alone at closed doors.* That reproduces Source's `OnDoorBlocked` loop: a chaser jitters and stuck-recovers against the door indefinitely.

## Behavior decisions

D1 and D3 are owner decisions (2026-10-30). The rest are recommended defaults (research §9), confirmed or changed at review.

| # | Question | Decision |
|---|---|---|
| D1 | Block policy for enemies | Same rules as players. `displace` pushes an unpinned enemy clear. A pinned enemy makes `displace` and `stop` hold and `reverse` back off, exactly as a pinned player does; no non-crush mover crushes or embeds anyone. `crush` crushes enemies and players alike. Authors mark a crusher with the existing `block_policy` key |
| D2 | Corpses | Not carried. Ignored as stop/reverse obstructions. Still crush victims |
| D3 | Closed door in planning | Impassable while docked closed; enemies detour or hold `blocked`. Door operation by enemies is a later roadmap item |
| D4 | Gate fidelity | Load-time region and portal gate first; measure door footprints before any bake split |
| D5 | Lifts | Enemies board only a mover already docked at the boarding terminus; no summoning |
| D6 | API | `AgentComponent.is_grounded` becomes a ground reference with an `is_grounded()` projection; agent body motion moves into physics |

## Orderings and edges

| # | Scenario | Expected outcome |
|---|---|---|
| O1 | Tick order | Mover stage publishes poses → … → agent steering applies carry → displace → combined sweep → ground ref → release → blocking pass (agents with ground refs) → weapons. AI reads pre-carry agent poses, one tick behind the mover; documented, not fixed |
| O2 | Agent on a lift that starts, reverses or stops | Carried by the tick delta at each edge; ground stays `Mover(id)` throughout |
| O3 | Agent on a riser when the mover starts | Displaced upward, then grounded on the mover the next tick |
| O4 | Agent on a descender | Ground-stick keeps footing within its reach; beyond it the agent falls under gravity |
| O5 | Agent steps off a moving mover | Release velocity enters the knockback layer, so it decays with knockback drag and steering does not wipe it |
| O6 | `reverse`/`stop` lift carrying an agent | Lift never reacts to its own rider, including through the airborne grace window |
| O7 | `crush` lift carrying an agent through open air | No damage |
| O8 | `crush` mover pins an agent against static geometry | Damage at the mover's cadence, continuing past death |
| O9 | Fast or thick crusher engulfs an agent in one tick | Counts as pinned (inside-hull fallback) |
| O9a | `displace` or `stop` door closes on an agent against the frame | Door holds until the agent leaves; no damage, no embedding |
| O9b | `reverse` door closes on an agent against the frame | Door backs off when its reversed course pins nothing; otherwise holds |
| O9c | Agent and player pinned by one mover on one tick | One hold or back-off decision covering both; same outcome as two players |
| O10 | Several victims crushed on one tick | Damage, events and impact callbacks in a fixed order, identical across runs |
| O11 | Corpse in a `reverse` doorway | Door closes through it, so the corpse does not hold the door open; crush still applies under `crush` |
| O12 | Door docks closed across a chaser's route | Gate flips on dock, the agent replans within the existing budget, then detours or latches `blocked` and holds without stuck cycling |
| O13 | Door leaves its closed dock | Gate clears at once; a held agent replans and passes once collision lets it |
| O14 | Gate flips twice before a replan admits | One replan, against the current gate |
| O15 | Lift docked at the boarding terminus | Link boardable; an agent routed through it walks on |
| O16 | Lift leaves while an agent is mid-board | Agent is carried if grounded on it; otherwise the link closes and the agent replans |
| O17 | Riding agent | Steering suspended, footing kept, stuck detection paused |
| O18 | Mover docks at the far terminus | Rider exits along the link and replans from the landing region |
| O19 | Rising room starts with enemies inside, below the arena | Enemies ride up; at the top terminus they exit through the room's doorway onto the arena |
| O20 | Mover with no terminus (pure spinner, endless path) | No links and no gate; collision and the blocking pass handle it |
| O21 | Level with zero movers | Agent motion, steering and paths identical to today |

## Acceptance criteria

- [ ] A closed mover stops an agent at its skin; once open, the agent passes. A `displace` mover pushes an unpinned agent clear (O12, O13; D1).
- [ ] An agent grounded on a moving mover reports mover ground every tick, and its displacement equals the mover's carry within 1e-4 m across start, reverse and stop (O2–O5).
- [ ] Riders never stop or reverse their own mover. A non-crush mover never damages or embeds an agent: it holds or backs off exactly as for a pinned player. A `crush` mover damages an agent only when pinned against static geometry or engulfed. Multi-victim crush order is identical across runs (O6–O11, O9a–O9c).
- [ ] With the only route closed, a chasing agent latches `blocked` and holds: position spread ≤ 0.05 m over 5 s, stuck recovery fires at most once. When the door opens, it passes within 2 s (O12–O14).
- [ ] Agents route through a docked lift: they board, ride without foot-sliding (host view) and exit at the far terminus. Enemies placed inside the rising room in `mover-setpiece` exit onto the arena after it rises (O15–O20).
- [ ] Player movement and the predict/reconcile suites pass unchanged. Levels with no movers keep agent behavior unchanged (O21).
- [ ] `sim_steer_agents` on `stress-env-volumes` grows by at most 0.1 ms, and the new code adds zero per-tick heap allocations (`alloc_probe`).

## Tasks

### Task 1: Shared mover-contact substrate and agent motion in physics

Behavior-preserving move and generalization:
- Move `collide_and_slide` (`sim/src/agent/mod.rs`) into a new `postretro-physics` module, `agent_motion`.
- Generalize the player's carry, displace, release-velocity and pin pieces into a body-agnostic module under `physics/src/collision/` that both the player substrate and agent motion call. Those pieces are `apply_mover_carry`, `displace_from_movers`, `apply_mover_release_velocity` and `mover_push_is_blocked_by_static` in `movement/mover_carry.rs`, plus `ground_ref_from_hit`.
- The player path stays bit-identical: same call order in `integrate_collision`, same results under the existing movement and predict/reconcile suites.
- `sim` calls the moved agent motion through its existing `pub(crate) use postretro_physics::…` re-export.
- No agent behavior changes in this task.

### Task 2: Agents move against movers (thin slice)

Thin slice through physics → steering → blocking pass, falsifying "agents can share the player substrate without steering or stuck recovery fighting it."

Agent ground state:
- Replace `AgentComponent.is_grounded: bool` with a `GroundRef` field (the foundation crate's enum) plus an `is_grounded()` projection.
- Update its writers and readers (`agent.rs` init and `add_knockback`, `agent_steering.rs` knockback decay and the two `result.grounded` writes) and their tests.

Steering:
- `agent_steering::tick` takes `&CombinedCollisionWorld`. In `simulate_tick_with_presentation_aim`, the SteerAgents stage builds it from `collision_world`, `mover_colliders` and a shared borrow of the tick's table that ends before `split_for_blocking`.
- Agent motion runs carry → displace → combined sweep → ground ref → release (O1). Release velocity goes into `knockback_velocity`.
- Stuck detection and recovery do not run while the agent's ground is a moving mover.
- `agent_steering.rs` (1239 lines) and `sim/mod.rs` take threading edits only; new logic lives in physics or a new `agent_steering/` sibling.
- Add an agent-on-lift scenario to `sim/determinism_tests`.

Covers AC 1, AC 2, AC 6 and AC 7 for the motion side.

### Task 3: Blocking pass treats agents like players

In `physics/src/kinematic_mover/blocking.rs` (857 lines; split along the player/agent seam first if this task would push it past ~1000):

- **Riders.** Agent tuples carry their ground ref, and the rider exemption and `RIDER_GRACE_MS` grace apply to agents grounded on the mover (O6).
- **Pins and yields.** Extend `kinematic_mover/pin.rs` (`pawn_bodies`) and `run_mover_pin_pretest` to living agents, keyed on the agent's ground ref for the rider test, so `displace` and `stop` hold and `reverse` backs off for a pinned agent exactly as for a pinned player (D1; O9a–O9c). The first-yield warning and onset memory stay per mover.
- **Crush.** Agent pinning for `crush` uses the player's pin predicate (penetration blocked by static), plus a `point_inside_mover` check on the capsule's axis points for engulfment (O7–O9).
- **Order.** `pinned_crush_contacts` becomes an ordered collection, or is sorted before iteration, so damage, `Crushed` events and impact callbacks run in a fixed order (O10).
- **Corpses.** `agent_capsules` keeps quiescent agents for crush only and excludes them from stop/reverse obstruction (O11; D2).

Covers AC 3.

### Task 4: Nav gate

Add `sim/src/nav/gate.rs`.

At level load:
- Add a public "docked at terminus k" predicate beside `mover_is_docked_closed` (`physics/src/kinematic_mover.rs`); its private terminus helpers stay private. Task 5 reuses it.
- For each mover terminus, pose the mover's local AABB there and record the regions and portals it fills: XZ footprint overlap (inflated by agent radius) and overlap with the band [floor + step height, floor + agent height]. A lift top resting at floor level fills nothing; a full-height door fills its doorway. Give `MoverCollider` (or a sibling table) a local AABB; its bound sphere is too loose for a long door.

Each tick:
- A terminus's fill set is gated while its mover is docked there, and clear otherwise. Gate generations change only on dock and undock.
- The gate is a reusable bitset with a generation counter and no per-tick allocation.

Plumbing:
- `find_path` takes `&NavGate`; update its callers (`agent_steering`, `ai/src/compute.rs` reachability, `combat_positioning/scoring.rs`).
- A generation change marks affected agents drift-priority in `admit_replans`, inside the existing budget (O14).
- `endpoints_are_topologically_connected` and `blocked_destination_now_directly_routable` consult the gate. `region_components` becomes a static superset.

Measure first: report how door footprints fall in the rectangular decomposition on `closet-reveal`, `stress-warren-mini` and `mover-setpiece` (does a doorway get its own region or portal?). If a door overlaps a large region, stop and report before building the gate rule, because D4 is decided on that measurement. Covers AC 4.

### Task 5: Mover links

At level load, derive one link per mover per terminus:
- the mover's upward-facing walkable triangles posed at that terminus,
- joined to nav regions whose floor band is within step height and whose XZ edge is within agent radius.

Planning and riding:
- Planning treats a link as a portal that is open only while the mover is docked at that terminus (Task 4's predicate; O15, O16; D5).
- Board → Ride → Exit lives in a new `agent_steering/mover_link.rs`. Ride suspends steering and stuck detection (O17). Exit fires on docking at the far terminus and triggers a replan (O18).
- An agent grounded on a docked mover resolves its nav region through that mover's link instead of the static region beneath it in XZ.

Movers with no terminus get no links (O20). The rising room in `mover-setpiece` is the acceptance case (O19). Covers AC 5.

### Task 6: Docs and proof map

- Add a movers section to `docs/level_design.md`; today block policies are documented only as FGD hints. It covers what each block policy does to players and enemies; that `displace` pins leave a body inside the mover, so use `crush`, `reverse` or `stop` for traps; that enemies do not call lifts; and how doors gate enemy routes.
- Add enemies to `content/dev/maps/mover-setpiece.map`: one chaser behind the sliding wall, two inside the rising room. Add a small `enemy-movers` section or map with a lift, a crusher and a sweeper.
- Record the manual checks in its README.

## Sequencing

**Phase 1 (sequential):** Task 1 — refactor; everything builds on the moved substrate.
**Phase 2 (sequential):** Task 2 — thin slice, falsifies substrate sharing.
**Phase 3 (concurrent):** Task 3 (blocking pass), Task 4 (nav gate). Disjoint files.
**Phase 4 (sequential):** Task 5 — consumes Task 4's gate and Task 2's ground ref.
**Phase 5 (sequential):** Task 6.

## Rough sketch

- `sim/mod.rs` stage order: Movers → Movement → Triggers → Projectiles → Ai → Traps → Steering (SteerAgents, SteerPose, SteerMovers = blocking pass) → Weapons. The mover table is `&mut` only at the Movers stage and in `split_for_blocking`.
- `region_at` has no height cap: an agent on a raised mover resolves to the region under it in XZ. Task 5's link resolution replaces that for docked movers.
- `find_path` today: endpoint resolution, A* over portal crossings (XZ midpoint costs), inset, funnel, endpoint clearance. Neither `NavRegionRecord` nor `NavPortal` has a flags field, so the gate stays a runtime side table.
- `KinematicWaypointRecord {name, next, origin}` gives terminus poses at load. The mesh is mover-local, so posing it at a terminus is a translate (plus rotation for spin movers, which get no links).

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Player mover contact is bit-identical after the substrate move | Task 1 | Tasks 2–3 editing shared mover-contact code | AC 6 |
| An agent's ground ref is the single source for carry, rider exemption, stuck suppression and link resolution | Task 2 | Tasks 3, 5 reading a separate grounded flag or pose test | AC 2, 3, 5 |
| A path never routes through a gated region or portal, or a link whose mover is not docked at its terminus | Tasks 4, 5 | Every `find_path` caller and reachability check | AC 4, 5 |
| Crush effects apply in a fixed order | Task 3 | Any new crush collection | AC 3 |

## Open questions

- Confirm or change D2, D4, D5 and D6.
- Gate fidelity (D4) depends on Task 4's measurement. A bake-time split would add a NavMesh version bump.
- **Landing order.** Land `E17--movers-block-shots-and-sight` first or together, so enemies held at closed doors cannot fire through them.
