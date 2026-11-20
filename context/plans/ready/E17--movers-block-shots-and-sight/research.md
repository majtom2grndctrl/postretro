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

The FIRE-frozen static distance existed for the late-start case (see Credit rule). The march starts from the same frozen origin and finds that contact itself.

## Credit rule (owner, 2026-11-20)

The owner asked for the client to get credit for the impact it saw on screen, as far as the network allows. A bare "nearer of host contact and declared distance" fails in each case below, and the on-path rule answers it:

- **Late start.** The host fires from the pawn's live eye, so the two paths diverge. A distance measured on the host path lands mid-air short of the host's wall (the netcode late-strafe test: declared about 10.2, wall about 12). An off-path record takes the host's own contact.
- **Agreement at a wall.** Declared and host distances differ by float noise, and a declared point has no host surface for the zero-radius nudge. A host contact at the declared point decides with its own surface.
- **Direct hits.** A blast with no impact target loses the weapon's direct push, which the modder reference promises composes with splash (a splash rocket deals no direct health damage). A credited enemy hit takes the declared enemy as its target, including an enemy flush against a wall the host also has; a host contact at the point decides only for a world record.
- **Records.** The first valid record counts, as presentation reads today. An empty declaration means the client saw no impact.

Entities never cut the host path short, so a direct hit the host's flight passes by still detonates, which closes the gap where a declared entity the host's flight misses sets off nothing. That is a deliberate reversal of §Combat authority's "the client never selects a splash center", not a gap fix alone. The client selects only which on-path point counts, so one authoritative blast reaches every peer. A dishonest point is anti-cheat, a non-goal.

The tolerance's floor is not prediction error alone. The host rebuilds a non-late launch converging on the target's live position while the client converged on its rendered one, so the paths diverge by up to the muzzle's offset from the eye (P25).

Observer flight today: the end-point hook holds a display rocket still at its contact for about one round trip, until peers acknowledge, then flashes; the flight starts a tick ahead of the march. Client splash rockets instead step with the march and vanish at its deciding point without a flash (owner).

Rival: split into two briefs, movers blocking shots and sight (host march contact as the centre, the old rule kept) and the credit rule with observer flight. Rejected: the march's release rules (P2, P5, P16, P19, P20, P23, P27, P33) are built around the credit rule, so a split builds the release logic and rewrites the netcode splash tests twice.

## Corrections to the 2026-10-31 draft

| Old claim | Now |
|---|---|
| Lands before or with `E17--enemy-mover-interaction` | That plan landed. The owner ruled on 2026-11-06 that they land in either order and accepted the gap: an enemy held at a door can see and fire through it. |
| "Sight returns within the existing debounce" after a door opens | Holds only for a retained target. Under E10 a target survives a loss of sight while its activity engages (`compute.rs` `retains_target`). Once dropped, it is re-noticed only through its awareness meter: the first sighting after a miss gains nothing, and later ones refill at `fillMs` from where the closure drained it (`AwarenessCheck::visit`). |
| Physics has no alloc probe | `postretro-physics` installs `alloc_probe` as its test global allocator. |
| `split_for_blocking` is the only `&mut` table use after Movers | SteerAgents calls `yield_agent_as_corpse` first. The Ai stage already holds a shared borrow (`MoverNav { poses }`). |
| Client shot vs presented pose: error under 0.1 m, one tick | `ready/E17--mover-relative-rider-presentation` smooths mover corrections, render-only. The drawn mover trails a correction until it converges (its research measures about 0.16 m under the mandated profile). Decision unchanged: presentation never feeds collision. |
| Paths `postretro/src/sim`, `weapon/mod.rs` | `crates/sim/src/{sim,weapon}/`. The tick entry is `sim/tick_start.rs`. `resolve_client_shot` and the local and remote weapon commands moved to submodules. |
| Docs to amend: scripting-reference splash note | The brief's Docs decision lists the human-facing files. Also amend the `splash.rs` doc comment, which says movers do not participate. |

## AI line-of-sight sites (E10 branch)

- `perception::raw_target_perception` serves acquisition and the awareness scan. The scan reaches line of sight only through this function's closure in `compute.rs`, and dormant checks take the same path.
- `perception::perceive_target` serves the retained target each run and feeds the fire gate.
- `combat_positioning::scoring::score_candidate_with`, through `CombatQuery.collision_world`. That field is not an `Option`, and `placement_cache::BatchPlacements::for_batch` batches queries by comparing its pointer.

No single choke point. Rays per acquisition check are bounded by candidates walked until the meter bound passes, plus unvisited meter members and ledger attackers. Each now also culls movers.

## Pin table

Host tick as built: declarations drain before Movers publishes the tick's poses, and again after the tick's shots are recorded. The network is polled once per frame, before the tick loop, so a declaration never arrives between stages. The blocking pass reads poses and never rewrites them, so Projectiles, Ai and Weapons see one table per tick.

| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Client rocket and host rocket fired on one tick along one path; a door slides across it | Declarations drain each host tick; the march's first step is on the tick after FIRE | Each march advances exactly one step per host tick against that tick's published poses; both rockets meet the door on the same tick |
| P2 | A door closes behind a client rocket the march has already carried past it, before release; the client declared a contact beyond the door | Door closes after the cursor passes its plane | Host detonates at the next deciding point ahead, never at the door |
| P3 | A client rocket strikes a closed door; the door opens before the declaration arrives | March contact at the door on tick C; door opens; declaration in hand at R > C | Host detonates at the point found on tick C |
| P4 | Declaration in hand before the march reaches a closed door | Declaration first; march contact later | Releases at the door; the door shields its far side |
| P5 | Declaration arrives after the march reached a closed door | March contact on C; declaration at R > C | Releases on R at the door; the door shields its far side |
| P6 | A client rocket strikes a door on C; the door opens before release on R | Contact pose differs from release pose | Centre is the deciding point found by tick C; occlusion and the entity set read tick R's table |
| P7 | Client rocket declared on an enemy behind a door the host closed before the march reached it | March contact at the door, nearer than the declared enemy | Blast at the door; the enemy takes no damage |
| P8 | Declared entity contact the host's flight passes by | March reaches the declared distance with no world contact before it | Blast at the declared point, with the declared enemy as impact target: the direct push plus splash |
| P9 | The march reaches range or lifetime before its own contact and any on-path declared point | March resolves empty, whenever the declaration arrives | FIRE stands, hit refused, no host blast |
| P10 | March contact exactly at the range or lifetime boundary, including a lifetime of 0 | Contact in the final segment | Contact wins over expiry, as for a host rocket; a zero lifetime resolves empty on the first step |
| P11 | Client and host disagree on a door | Client declares the wall (door predicted open) while the host's door is closed at arrival; and the reverse | Host door closed: blast at the door (host contact nearer). Host door open: blast at the client's declared point |
| P12 | Door closes or snaps onto a projectile | Projectile inside the door's posed hull at the start of its step | Detonates that tick at its position; near side damaged per P21, far side shielded |
| P13 | Door closes between an engaged NPC and its target | Fire gate open by grace for the window; NPC fires in it | The shot meets the door; the target takes no damage |
| P14 | Client frame spans several ticks while a door crosses its rocket's path | Client sweeps one frame's segment against its last predicted poses; host marches per tick | Disagreement bounded by one client frame's mover travel; the credit rule decides |
| P15 | A door closes on tick T and the blocking pass reverses it on T | Projectiles, Ai and Weapons each query on T | All three see the pose Movers published on T; the reversal shows from T+1 |
| P16 | Declaration arrives after the march has passed the declared point | March past it on tick C; declaration at R > C | Blast on R at the declared point |
| P17 | Late start after a strafe; host wall beyond the declared point | Host launch point differs from the client's; declared point off the host path | Blast at the host's own march contact (the wall), never mid-air |
| P18 | Zero-radius client rocket at a wall the host also has | Declared point equal to, or a hair short of, the host's contact | The host's contact and surface decide: blast side damaged, far side shielded |
| P19 | March ends at range or lifetime; the declaration arrives later | March empty on tick C; declaration at R > C | Declared point within the path the march covered: blast on R there. Beyond it: FIRE stands, hit refused |
| P20 | As P2, but the client saw the door close in front of its rocket and declared it | Host door closes after the cursor passes its plane; declared point at the door | Blast at the door's plane; occlusion per P21 |
| P21 | Blast centre inside a mover hull at detonation or release | Enclosed at step start, or covered between contact and release | Side the projectile came from damaged; far side shielded |
| P22 | Declared point at the launch point, or a rocket launched inside a mover hull | Point reached before any step; FIRE on tick F | Blast on F+1 in the Projectiles stage, before AI; never on F |
| P23 | Splash declaration with several valid records | One shot, several records | The first valid record counts. If it is off-path, the host's own march contact decides and later records are ignored |
| P24 | A host rocket and a client blast land on one target on one tick | Both in the Projectiles stage | Client blasts resolve before host projectile impacts, as remote declarations ingest today; kill credit follows that order |
| P25 | Non-late client rocket at an enemy that moved between the client's view and the host's, from a muzzle off the eye | Client converges on the rendered enemy; the host's rebuild converges on the wall behind | Declared point counts as on-path: blast at it, the enemy as impact target, at short and full range |
| P26 | Declared point at the on-path tolerance | Just inside, with the host's wall beyond; just outside | Inside: blast at the declared point, short of the wall. Outside: the host's own march contact |
| P27 | Declared enemy killed or removed after the client's contact, before release | Enemy gone on tick D; release on R ≥ D | Blast at the declared point with the declared normal, no impact target, no direct push; never moved to the host's march contact |
| P28 | Declared enemy pressed against a wall or door the host also has | Host contact within the tolerance beyond the enemy's point | Blast at the declared point, the enemy as impact target, the wall shielding its far side |
| P29 | Splash declaration whose records are all invalid | Each non-finite or beyond range | As empty: no blast, FIRE stands, hit refused |
| P30 | Blast centred inside a hull with no outside position behind it: launched inside, or the snap covered the previous position | Enclosed at launch or at step start | The side the reverse flight direction leaves the hull through takes the blast; the far side is shielded |
| P31 | Projectile enclosed at step start with zero remaining range or lifetime | Enclosed and expiring on one step | Enclosure is a contact, and contact wins over expiry (P10): detonates per P30 |
| P32 | Observer flight of a client rocket | Declaration before the march contact; declaration on R after a contact on C; declared point behind the flight's end | Flight vanishes at the deciding point or march contact on C with no flash; a blast behind it appears on R at the declared point; with no contact, it ends at the path's end |
| P33 | Empty declaration after a march contact | March contact on C; empty declaration on R | No blast; FIRE stands, hit refused; the observer flight already ended on C |
