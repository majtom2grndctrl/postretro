# E17--movers-block-shots-and-sight — research

Read at `e7438238` (main) and `768446184` (`claude/gallant-franklin-xafi1n`, E10--ai-tick-scheduling, in review). Re-read the AI rows at E10's merge commit.

## Why the march, not a pose ring

The host never advances a connected client's splash projectile. At release it runs one cast over the whole travel distance (`resolve_authorized_splash_projectile_impact` → `resolve_projectile_impact`), testing static world and entities at their release-time positions. Release waits until host travel covers the declared contact or, for a world contact, the FIRE-frozen static contact (`shot_records::projectile_hold_ticks`, `projectile_static_contact_distance`).

Movers break both halves:

- **Replay.** A mover's pose at release is not its pose when the projectile reached it. A door moving across the frozen path detonates the host blast where the shooter never saw it.
- **Hold.** The FIRE-frozen distance is static. A rocket that strikes a door in front of a wall is held until host travel reaches the wall, so the authoritative blast and damage land late by (wall − door) / speed.

Both mechanisms meet the owner rule (2026-10-30: smoothest co-op presentation the network allows, no extra traffic).

| | Pose ring (old draft) | Per-shot march (chosen) |
|---|---|---|
| State | Every mover's pose per host tick, sized to the longest splash lifetime among loaded descriptors | One cursor per open client splash shot |
| Lifetime | Level install to unload; resized on a descriptor hot reload that lengthens a lifetime | Shot open to close |
| Edge | A replay reaching past the ring (old E6a: warn once, use oldest pose) | None |
| Code paths | Host projectiles step per tick; client splash shots replay through a second routine that must stay in parity | One per-tick contact step for both |
| Per-tick cost | Ring write per mover per tick | One combined sphere cast per open, unresolved client splash shot, the same cost as one host rocket |

The cheaper rival is to keep both steps and switch them to the combined cast: the FIRE-time hold distance and the release-time cast. Both read one moment's poses. A FIRE-time distance cannot see a door that closes or opens in flight. A release-time cast meets a door that closed behind a rocket already past it, and misses one the rocket struck that opened before release. Only a per-tick step sees each mover when the projectile reaches it.

The FIRE-frozen static distance existed for the late-start case: a start admitted late after a stall fires from the pawn's live eye, so the host ray can meet the world beyond the declared point. The march starts from the same frozen origin and finds that contact itself.

Entity contacts stay a release-time test. A march that also tested entities per tick would close `networking.md` §Combat authority's known gap (host ray misses a declared entity, nothing detonates). That changes entity-contact semantics and is left for a later brief.

## Corrections to the 2026-10-31 draft

| Old claim | Now |
|---|---|
| Lands before or with `E17--enemy-mover-interaction` | That plan landed. The owner ruled on 2026-11-06 that they land in either order and accepted the gap: an enemy held at a door can see and fire through it. |
| "Sight returns within the existing debounce" after a door opens | Holds only for a retained target. Under E10 a target survives a loss of sight while its activity engages (`compute.rs` `retains_target`). Once dropped, it is re-noticed only through its awareness meter, which needs two passing checks in a row (`AwarenessCheck::visit`). |
| Physics has no alloc probe | `postretro-physics` installs `alloc_probe` as its test global allocator. |
| `split_for_blocking` is the only `&mut` table use after Movers | SteerAgents calls `yield_agent_as_corpse` first. The Ai stage already holds a shared borrow (`MoverNav { poses }`). |
| Client shot vs presented pose: error under 0.1 m, one tick | `ready/E17--mover-relative-rider-presentation` smooths mover corrections, render-only. The drawn mover trails a correction until it converges (its research measures about 0.16 m under the mandated profile). Decision unchanged: presentation never feeds collision. |
| Paths `postretro/src/sim`, `weapon/mod.rs` | `crates/sim/src/{sim,weapon}/`. The tick entry is `sim/tick_start.rs`. `resolve_client_shot` and the local and remote weapon commands moved to submodules. |
| Docs to amend: scripting-reference splash note | Also `docs/level_design.md` §Doors and lifts in enemy routes, `context/lib/entity_model.md` §Splash, the `splash.rs` doc comment, and `content/dev/maps/mover-setpiece.README.md`. |

## AI line-of-sight sites (E10 branch)

- `perception::raw_target_perception` serves acquisition and the awareness scan. The scan reaches line of sight only through this function's closure in `compute.rs`, and dormant checks take the same path.
- `perception::perceive_target` serves the retained target each run and feeds the fire gate.
- `combat_positioning::scoring::score_candidate_with`, through `CombatQuery.collision_world`. That field is not an `Option`, and `placement_cache::BatchPlacements::for_batch` batches queries by comparing its pointer.

No single choke point. Rays per acquisition check are bounded by candidates walked until eight pass, plus unvisited meter members and ledger attackers. Each now also culls movers.

## Pin table

Host tick as built: declarations drain twice per tick, once before Movers publishes the tick's poses and once after the tick's shots are recorded. The blocking pass reads poses and never rewrites them, so Projectiles, Ai and Weapons see one table per tick.

| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Client rocket and host rocket fired on one tick along one path; a door slides across it | Two drains per host tick; the march's first step is on the tick after FIRE | Each march advances exactly one step per host tick against that tick's published poses; both rockets meet the door on the same tick |
| P2 | A door closes behind a client rocket the march has already carried past it, before release | Door closes after the cursor passes its plane | Host detonates at the next contact ahead, never at the door |
| P3 | A client rocket strikes a closed door; the door opens before the declaration arrives | March contact at the door on tick C; door opens; declaration in hand at R > C | Host detonates at the point found on tick C |
| P4 | Declaration in hand before the march reaches a closed door | Declaration first; march contact later | Releases at the door; the door shields its far side |
| P5 | Declaration arrives after the march reached a closed door | March contact on C; declaration at R > C | Releases on R at the door; the door shields its far side |
| P6 | A client rocket strikes a door on C; the door opens before release on R | Contact pose differs from release pose | Centre is the tick-C contact point; occlusion and the entity set read tick R's table |
| P7 | Client rocket declared on an enemy behind a door the host closed before the march reached it | March contact at the door before the entity hold expires | Blast at the door; the enemy takes no damage |
| P8 | Entity contact released while the march has no world contact | Release with the march unresolved | The entity test reaches no farther than the march has stepped |
| P9 | Declared world contact; the march reaches range or lifetime with no contact | March resolves empty | FIRE stands, hit refused, no host blast |
| P10 | March contact exactly at the range or lifetime boundary, including a lifetime of 0 | Contact in the final segment | Contact wins over expiry, as for a host rocket; a zero lifetime resolves empty on the first step |
| P11 | Client and host disagree on a door | Client declares the wall (door predicted open) while the host's door is closed at arrival; and the reverse | Host detonates at its own march contact both times |
| P13 | Door closes between an engaged NPC and its target | Fire gate open by grace for the window; NPC fires in it | The shot meets the door; the target takes no damage |
| P14 | Client frame spans several ticks while a door crosses its rocket's path | Client sweeps one frame's segment against its last predicted poses; host marches per tick | Disagreement bounded by one client frame's mover travel; the host's march decides |
| P15 | A door closes on tick T and the blocking pass reverses it on T | Projectiles, Ai and Weapons each query on T | All three see the pose Movers published on T; the reversal shows from T+1 |
