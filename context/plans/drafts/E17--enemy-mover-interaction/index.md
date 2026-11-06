# E17--enemy-mover-interaction

Brief · resumable · Epic 17 · reads: `context/lib/movement.md` §6 · `context/lib/entity_model.md` §7, §7c · `context/lib/build_pipeline.md` §Navigation bake · `context/lib/networking.md` §Current contract · read at 7dec6ed0

## Problem

Owner-requested capability, Epic 17: enemies treat movers as solid and as ground, as players already do. Today agent motion sweeps static geometry only and keeps a bare grounded flag, and the nav graph has no runtime notion of a mover, so enemies walk through closed doors, stand inside or fall through lifts, are never carried or pushed, and a crusher damages an enemy it merely overlaps in open air. When done, closed doors stop enemies and their routes; lifts, platforms and a room rising into place carry them; a `displace` mover pushes them clear; a mover pinning one holds or backs off exactly as for a player; `crush` damages them only when pinned or engulfed; and enemies route across a docked lift. Derivation: `research.md`.

## Decisions

- **One mover-contact substrate.** Carry → displace → combined sweep → ground ref → release, generalized from the player path into a body-agnostic `postretro-physics` module that both player and agent motion call. Agent body motion moves from `sim` into physics. Body-side carry matches `movement.md` §6; physics owns movement and mover collision (`development_guide.md` §Workspace). The player path stays bit-identical.
- **Agent ground is a `GroundRef`.** `AgentComponent.is_grounded: bool` becomes a ground reference with an `is_grounded()` projection. It is the single source for carry, rider exemption, stuck suppression and link resolution.
- **Enemies follow the player block rules (owner, 2026-10-30).** `displace` pushes an unpinned enemy clear. A pinned living enemy makes `displace` and `stop` hold and `reverse` back off, exactly as a pinned player does; no non-crush mover damages or embeds a living enemy. `crush` damages enemies only when pinned against static geometry or another mover, or engulfed, and keeps damaging past death. A rider never stops or reverses its own mover. This supersedes `movement.md` §6 "agents are never carried or displaced" and reverses `done/E17--rider-not-a-blocking-contact` AC8 for riders.
- **Corpses ride, never block (owner, 2026-11-06).** A zero-HP agent keeps gravity, carry and displacement but no steering. It never stops, reverses or holds a mover, and a pin against it is no yield. `crush` still damages it. Diverges from the player rule "dead pawns count": a dead player may be revived, a corpse waits only for the mod's despawn.
- **Crush effects run in a fixed order.** Multi-victim damage, `Crushed` events and impact callbacks are deterministic across runs.
- **Closed doors are impassable to planning.** A mover docked at a terminus gates the nav regions and portals it fills there; paths, reachability and replans respect the gate. Enemies detour or latch `blocked` and hold. The gate changes only on dock and undock, never on live posed bounds, so sweepers, crushers and mid-travel doors stay collision's job. A runtime side table, load-time derived: no NavMesh format change.
- **Gate fidelity is measured before it is built.** Whether a doorway gets its own region or portal in the rectangular decomposition decides whether a load-time region and portal gate is enough. If a door overlaps a large region, the build stops for the owner before the gate rule; a bake-time split would bump `NAVMESH_VERSION`.
- **Lifts are runtime links.** Load-time links from a mover's walkable top at each terminus to adjacent regions, open only while it is docked there. Enemies board only an already docked mover, ride with steering and stuck detection suspended, and exit at the far terminus. This covers lifts and the rising room.
- **Host-only.** Agents are host-simulated (`networking.md`); no wire, prediction or digest change. Gating derives from data already in the content digest.
- **Non-goals.**
  - Remote presentation of riding enemies: roadmap **Co-op presentation of NPCs on moving geometry**. Until it lands a remote client draws a riding enemy off its mover by RTT/2 plus interpolation delay times mover speed (about 30–50 cm at 150 ms on a 3 m/s lift); host and listen-server views are exact.
  - Enemies opening doors or summoning lifts, and a costed wait-at-door link: roadmap **Enemy door operation**.
  - Movers blocking shots and sight: `drafts/E17--movers-block-shots-and-sight/`. The two land in either order; enemies seeing through a closed door until then is accepted.
  - Carrying agent yaw on rotating movers: facing re-derives from steering. Agents using teleporters.

## Acceptance

### Automated
- [ ] A closed mover stops an agent at its skin; once open, the agent passes. A `displace` mover pushes an unpinned agent clear (O12, O13).
- [ ] An agent grounded on a moving mover reports mover ground every tick, and its displacement equals the mover's carry within 1e-4 m across start, reverse and stop. A descender beyond ground-stick reach drops it under gravity; stepping off adds release velocity that decays with knockback drag (O2–O5).
- [ ] Riders, including within the airborne grace, never stop or reverse their own mover; a non-rider agent in the sweep still triggers `stop` and `reverse` (O6).
- [ ] A non-crush mover pinning a living agent holds (`displace`, `stop`) or backs off when its reversed course pins nothing (`reverse`), with no damage and no embedding; an agent and a player pinned on one tick get one decision (O9a–O9c).
- [ ] A `crush` mover carrying an agent through open air deals no damage; pinned against static geometry, another mover, or engulfed in one tick, the agent takes damage at the cadence past death (O7–O9).
- [ ] Multi-victim crush yields identical damage, event and callback order across runs (O10).
- [ ] A corpse in a `stop` or `reverse` doorway never holds or reverses the door; a corpse on a starting lift rides it; `crush` still damages a pinned corpse (O11, O11a).
- [ ] With the only route closed, a chasing agent latches `blocked` and holds: position spread ≤ 0.05 m over 5 s, stuck recovery fires at most once. The door opening, it passes within 2 s. Two gate flips before admission cost one replan (O12–O14).
- [ ] An agent routes across a docked lift: boards, rides with steering and stuck detection paused, exits at the far terminus and replans. A lift leaving mid-board carries a grounded agent and otherwise closes the link (O15–O18).
- [ ] A mover with no terminus produces no gate and no link (O20). A level with zero movers keeps agent motion, steering and paths identical to today (O21).
- [ ] Player movement and the predict/reconcile suites pass unchanged.
- [ ] A determinism scenario with an agent on a lift produces identical state across runs.
- [ ] `sim_steer_agents` on `stress-env-volumes` grows by at most 0.1 ms, and the new per-tick paths (agent mover contact, gate update, link resolution) allocate nothing (`alloc_probe`, measured on those paths).

### Manual
- [ ] Gate fidelity report: how door footprints fall in the nav decomposition on `closet-reveal`, `stress-warren-mini` and `mover-setpiece`, delivered before the gate rule is built.
- [ ] `mover-setpiece`: a chaser behind the sliding wall waits without jitter and passes when it opens; enemies inside the rising room ride up and exit onto the arena (O19).
- [ ] A small enemy-movers map: enemies ride a lift without foot-sliding (host view), a sweeper pushes them, a crusher kills them only against a wall, a corpse rides a lift. Checks recorded in the map's README.
- [ ] `docs/level_design.md` §Movers describes what each block policy does to enemies and corpses, that `displace` leaves pinned bodies inside the mover (use `crush`, `reverse` or `stop` for traps), that enemies do not call lifts, and how doors gate enemy routes.

## Path

- **First slice:** move `sim/src/agent/mod.rs::collide_and_slide` into physics and generalize `movement/mover_carry.rs` (`apply_mover_carry`, `displace_from_movers`, `apply_mover_release_velocity`, `mover_push_is_blocked_by_static`) plus `ground_ref_from_hit` behind `substrate.rs::integrate_collision`, behavior-preserving. Then thread `CombinedCollisionWorld` into `agent_steering::tick` at SteerAgents (built as player movement builds it). This falsifies the riskiest premise: steering and stuck recovery do not fight the shared substrate (door jitter, lift self-bounce, stuck loop).
- Release velocity enters `knockback_velocity`. Stuck detection pauses while ground is a moving mover. Corpses need a motion-only branch, since `tick_impl` drops quiescent agents today; physics needs its own quiescence test (`is_quiescent` is `sim`-private).
- Blocking pass: agent tuples carry their ground ref; `pin.rs::pawn_bodies` and `run_mover_pin_pretest` take living agents; crush uses `PinWorld::push_is_blocked` plus `point_inside_mover` on the capsule axis for engulfment; `pinned_crush_contacts` becomes ordered or is sorted; `agent_capsules` keeps corpses for crush only. Rival: a separate agent pin module, rejected because it duplicates the predicate.
- Nav gate: `sim/src/nav/gate.rs`, a reusable bitset with a generation counter. Fill set per terminus: mover local AABB posed there, XZ overlap inflated by agent radius, band [floor + step height, floor + agent height], so a lift top at floor level fills nothing. `MoverCollider` needs a local AABB; its bound sphere is too loose for a long door. A public docked-at-terminus predicate beside `mover_is_docked_closed`. `find_path` takes the gate (callers: `agent_steering`, `ai/src/compute.rs`, `combat_positioning/scoring.rs`); a generation change marks affected agents drift-priority in `admit_replans`; `endpoints_are_topologically_connected` and `blocked_destination_now_directly_routable` consult it; `region_components` becomes a static superset.
- Links: one per mover per terminus from upward walkable triangles posed there, joined to regions within step height and agent radius. `NavGraph` has no mutation API, so links need a construction path or a side structure `find_path` reads. An agent on a docked mover resolves its region through the link; `region_at` otherwise picks the floor under it in XZ. Board → Ride → Exit in a new `agent_steering/mover_link.rs`.
- Split first: `agent_steering.rs` (1239 lines) and `sim/mod.rs` take threading edits only; `blocking.rs` (861) splits along the player/agent seam if it would pass ~1000.
- Two `agent/overlap_tests.rs` tests assert static-only agents and are rewritten, not kept.
- Allocation: `tick_impl` and `agent_capsules` already allocate per tick; the zero-allocation check scopes to new paths or compares against a baseline count.
- At promotion: `movement.md` §6 (agents carried, pinned; corpses), `build_pipeline.md` §Navigation bake ("no format break" is false for portal kinds; links are runtime).

## Open questions

- Fill-set band and inflation constants, and whether the gate rule holds on the measured maps — **delegated**: the executor measures and reports; a door overlapping a large region stops for the owner per Decisions.
