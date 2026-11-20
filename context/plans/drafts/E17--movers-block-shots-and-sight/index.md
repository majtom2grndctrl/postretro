# E17--movers-block-shots-and-sight

Brief · compact · Epic 17 · reads: `context/lib/networking.md` §Combat authority, §World-LOS-only validation · `context/lib/entity_model.md` §Components (Splash), §7c · `context/lib/development_guide.md` §Workspace · `in-progress/E10--ai-tick-scheduling/` · `research.md` · read at e7438238, AI rows at E10 branch 768446184

**Sequence:** builds after `E10--ai-tick-scheduling` merges; re-read the AI rows at its merge commit.

## Problem
Owner decision (2026-10-30), now a live defect: since `E17--enemy-mover-interaction` landed, an enemy held at a closed door still sees and shoots through it, and players' bullets, rockets and blasts pass through doors, sliding walls and rising rooms. Cause: every shot and sight query reads the static collision world; movers join only the combined static-plus-mover world that movement, steering and foot probes use. When done, a mover at its pose stops hitscan, projectiles, muzzle launch, splash damage and NPC sight, for the host and a connected client alike; an open gap passes as empty space. A connected client's rocket lands its authoritative blast where the client saw it hit, unless the host has a wall or door nearer on its path. Otherwise a level with no movers behaves exactly as today.

## Decisions
- **Combined line of sight and sphere cast in `postretro-physics`** (`development_guide.md` §Workspace). Line of sight matches static `line_of_sight` on the static part. The exact sphere cast acts as a ray at zero radius, and the nearest of static and mover wins. Both are public, with a public read of whether a hit came from a mover; mover-aware enemy cover (roadmap E17, `drafts/E10--enemy-cover`) consumes them next.
- **A mover contact is a world contact**: same tie rule (world wins), impact presentation, host validation and zero-radius splash nudge. No consumer branches on hit source.
- **Each vantage reads one pose source.**

  | Query | Vantage | Mover pose |
  |---|---|---|
  | Hitscan, projectiles, muzzle, splash (host player, NPCs, traps); NPC sight; the rebuild of a client's FIRE launch (muzzle, convergence) | Host tick | The tick's published table, the same for every stage of the tick |
  | Own hitscan, muzzle, predicted projectile | Connected client | Its predicted mover table, the one its movement prediction reads; never the presented pose, which is render-only (`ready/E17--mover-relative-rider-presentation`) |
  | Client splash shot (host blast) | Host | Each tick's published table as the march reaches it |
  | Hit validation | Host | Static only (`networking.md` §World-LOS-only validation, kept) |

- **Validation stays static because the shooter's own cast sees movers.** A host mover check would reject honest hits near a moving edge, where host and predicted poses differ by up to a round trip of travel; catching a lying client is anti-cheat (`index.md` §4).
- **A projectile enclosed by a mover detonates in place.** One that starts its step inside a mover's posed hull, because a door closed or snapped onto it, detonates there that tick, and the mover shields its far side. Host projectiles, client predicted projectiles and the host march alike.
- **The host marches each open client splash shot and credits the client's on-screen impact (owner).** Each host tick, in the Projectiles stage, it steps the shot's frozen path one tick with the host projectile's world contact step against that tick's poses. With the declaration in hand, the blast lands on the first tick the march reaches either its own world contact or the client's declared distance along the host path, whichever is nearer, before AI. Entities never cut the host path short, so a client who saw a direct hit or a closed door gets that blast; a host world contact nearer than the declared distance still wins, so no blast passes a door closed on the host. This replaces the release-time cast and the FIRE-frozen hold distance, and amends `networking.md` §Combat authority: the client selects only how far along the host's own path, one authoritative blast for every peer. The pose-ring and release-cast rivals are in `research.md`.
- **Movers occlude splash; entities still do not.** Reverses `entity_model.md` §Components (Splash) for movers.
- **Every NPC sight test reads the combined world**: perception, the fire gate, awareness meters, dormant checks and combat-slot scoring, for enemies and allies alike. Sight timing stays E10's.
- **Placement stays static.** Combat-slot capsule placement and the other placement queries are not shot or sight tests. A spawner or trap authored inside a mover's swept path stays a content error.
- **Docs.** At promotion: `networking.md` §Combat authority and `entity_model.md` §Splash. At landing: the splash note and `brain.targetVisible` in `docs/scripting-reference.md`, the `brain.targetVisible` guard comments in `sdk/lib/brain.ts` and `brain.luau` and the typedef templates they are generated from, the doors note in `docs/level_design.md`, and `content/dev/maps/mover-setpiece.README.md`. Names and shapes do not change, so there is no scripting surface.
- **Non-goals:**
  - A door-aware mask on the cell-visibility relation (roadmap E17-F). That is a later broad phase, which must err toward visible and never reject a line this cast passes.
  - Rewinding movers to a shooter's past tick (`index.md` §4).
  - Mover-specific impact presentation.

## Acceptance

### Automated
Queries:
- [ ] Each combined query, against a closed mover, an open gap, a partly open mover, a rotated mover, two movers on one line and an origin inside a mover hull, returns the posed-geometry result: blocked at the nearest surface, clear through the gap, and today's static outcome for an origin inside geometry.
- [ ] With no movers, each combined query returns the static query's exact result, including a hit at the aim point counting as clear and a degenerate segment counting as blocked. A zero-radius combined sphere cast returns the combined ray's hit.
- [ ] Neither combined query allocates on the heap per call.

Host shots:
- [ ] Host-player and NPC hitscan stop at a closed mover with a world contact, and the target behind takes no damage. Through the gap the target is hit. A mover and an entity at equal distance give the world contact.
- [ ] Host-player, NPC and trap projectiles impact a closed mover, including a mover that moves into a projectile already in flight. A mover that opens before arrival lets it through.
- [ ] A composed muzzle on the far side of a mover from the eye launches from the eye. A connected client's composed muzzle across a door closed on the host launches the host's rebuilt FIRE, and its march, from the eye.
- [ ] A projectile a door closes or snaps onto, inside the door's hull at the start of its step, detonates there that tick, and a target on the door's far side takes no damage: for a host rocket, a client predicted rocket and the host march (P12).
- [ ] A mover between blast and target blocks splash damage, and a target on the blast side takes it. A blast centred on a mover face damages the blast side only. A zero-radius blast on either face of a rotated door damages that face's side only.
- [ ] On the tick a door closes, a host rocket in flight, an NPC's sight and the host player's hitscan all meet it, including when the blocking pass reverses the door that tick (P15).

Connected client:
- [ ] A client's hitscan stops at the mover in its prediction and declares a world contact, which the host accepts. With the mover open, the client hits the target behind and the host accepts. A mover the host has already closed, still open in the client's prediction, does not refuse the client's hit.
- [ ] Client predicted projectiles meet movers as host ones do. A connected client's composed muzzle across a mover closed in its prediction launches from the eye.
- [ ] A client rocket striking a closed door in front of a wall: the host blast and damage land at the door on the tick the march reaches it, before that tick's AI. That point is within the existing splash tolerance of the client's, and the tick is no later than with no wall behind the door.
- [ ] A door moving into a client rocket's path mid-flight: the host detonates on the door, where the client did, within the client's mover-prediction error plus one client frame's mover travel (P14). A door that opens before the rocket arrives: the host detonates at the wall behind.
- [ ] A host rocket and a client rocket fired on one tick along one path meet a door moving across it on the same tick, and an NPC near either blast reads its damage in that tick's AI. The client shot's march takes no step on its FIRE tick and one step per host tick after (P1).
- [ ] A door that closes behind a client rocket after the host's march has passed it does not detonate it: the host blast lands at the next contact ahead. A door the rocket struck that opens before the declaration arrives still takes the blast at the point the march found (P2, P3).
- [ ] A client rocket at a closed door releases at the door whether its declaration arrives before the march reaches the door or after. In both, the door shields a target on its far side (P4, P5).
- [ ] A client blast released after the door it struck has opened damages a target behind the door's old position: occlusion reads the release tick's poses (P6).
- [ ] A client rocket declared on an enemy behind a door the host closed before its march arrived detonates at the door, and the enemy takes no damage. A client rocket declared on an enemy that the host's flight passes by detonates at the declared distance along the host path, and the blast reaches entities from there (P7, P8).
- [ ] A client rocket whose host march reaches range or lifetime before both its own contact and the declared distance sets off no host blast, and its hit is refused. A contact in the march's final segment detonates there (P9, P10).
- [ ] A declaration that arrives after the march has passed its declared distance detonates at that distance on the tick it arrives (P16).
- [ ] A client that predicts a door open and declares the wall behind it, while the host's door is closed when the march arrives, gets the host blast at the door. With the doors reversed (client predicts the door closed, host has it open), the blast lands at the client's declared distance along the host path (P11).
- [ ] A client splash shot admitted late after a stall still detonates at the host's own first contact along the live-eye ray, mover or wall.
- [ ] A shot's march state is gone once the shot closes or expires, its client leaves, its pawn is removed, or the level unloads. A resolved march casts nothing more while its shot waits for a declaration. With no open client splash shots, the host does no march work.
- [ ] With no movers, a client world-contact blast lands at today's centre, on no later a tick. A declared entity contact the host's flight misses detonates at its declared distance, where today nothing detonates.

NPCs:
- [ ] An NPC facing a target behind a closed mover does not acquire it, fill its awareness meter, or fire; through the gap it acquires. A dormant check behaves the same.
- [ ] A door closing on an NPC's retained target stops its fire after the grace window. When the door reopens it fires on its first run after. A target its graph dropped while the door was closed is acquired again only once its awareness meter refills at its fill rate from where the closure drained it, never on the first passing check after the door opens.
- [ ] An NPC that fires inside the grace window after a door closes on its target hits the door, and the target takes no damage (P13).
- [ ] A combat slot behind a closed mover scores as having no line of sight; one through the gap scores as clear. With a closed mover across a candidate slot's line, the cached and uncached slot batches choose the same slots. An allied NPC behaves as an enemy does.

Unchanged:
- [ ] Static line of sight still ignores movers. Host validation accepts and refuses exactly what it does today.
- [ ] Grep gate: every hitscan, projectile, muzzle, splash and NPC sight site calls a combined query; static line of sight remains only in host hit validation. No shot, splash or sight consumer branches on whether a hit came from a mover, and that read is public outside the physics crate.
- [ ] Grep gate: every client shot path (hitscan, muzzle, predicted projectile) builds its mover poses from the table its movement prediction reads, and none from a presentation pose.
- [ ] Grep gate: the human-facing docs and `content/dev` READMEs this brief amends no longer say movers pass shots, splash or sight.
- [ ] On levels with no movers, the existing weapon, projectile, splash, perception, awareness and determinism suites pass unchanged, and headless byte identity holds (regression guard). The netcode splash-hold tests that seed the retired FIRE-frozen wall distance are rewritten against the march, with the same centres.

Scale (measured, not gated; `testing_guide.md` §Resource bounds):
- [ ] On E10's multi-arena and packed stress rows, with closing doors added in each arena: NPC line-of-sight cost per acquisition check and per tick, reported against the same rows without movers, on the machine class, worker count and cache mode of E10's runs.
- [ ] Host march cost per tick, reported with four clients firing splash weapons continuously, on the same machine class, worker count and cache mode.

### Manual
- [ ] Listen host and one client with simulated lag, on `content/dev/maps/mover-setpiece.map`: fire at the sliding wall from both sides, closed and open. Rocket the moving wall, and check that the shooter's burst and the observer's land together.
- [ ] On `content/dev/maps/enemy-movers.map`: an enemy held at a closed door neither notices nor shoots the player through it, and engages once it opens.

## Path
- Seams:
  - Physics: `collision::moving` (`CombinedCollisionWorld`, `MoverPoseSource`, `cast_ray_combined`, `cast_capsule_combined`, `mover_bound_may_reach_segment`, and `point_inside_mover` for the enclosed-projectile check; `CombinedCastHit.source` is `pub(crate)`). `collision::{line_of_sight, cast_sphere_exact}`. Put the new queries in a sibling module.
  - Weapon: `resolve_world_hit`, `NearestHitQuery` (its `resolve_client_hitscan` doc comment is stale), `fire_hitscan`, `tick_resolved_component`, `resolve_activation_shot[_owned]` and `resolve_muzzle_launch_origin` in `crates/sim/src/weapon/`. Also `weapon::client_resolution::resolve_client_shot` and `weapon_stage::commands::{local, remote}`.
  - Projectiles: `projectile_stage::{advance, advance_predicted, advance_matching, resolve_projectile_impact, nearest_projectile_hit, projectile_static_contact_distance}`. Splash: `splash::{entities_in_sphere, emit_splash_damage}`.
  - Netcode: `HostHitIngestContext`, `resolve_authorized_splash_projectile_impact`, `shot_records::projectile_hold_ticks`, `OpenAuthorizedShots`. Declarations drain twice per host tick (`host_take_ready_hit_declarations`, before Movers in the frame loop and again in the post-sim flush); both ingest with the static world today.
  - Launch rebuild: `weapon_stage::commands::remote` resolves a client FIRE's launch pose and the frozen static distance.
  - AI: the AI sites in `research.md`.
- Host plumbing:
  - The Ai stage already borrows the table (`MoverNav { poses }`).
  - SteerAgents builds a combined world before its `&mut` `yield_agent_as_corpse`. Build the Projectiles- and Ai-stage worlds the same way, from shared borrows that end before SteerAgents.
  - Weapons follows SteerMovers' `split_for_blocking`.
- Client plumbing: `App` builds the prediction world from `collision_world`, `kinematic_mover_colliders` and `kinematic_mover_tick_states`. `run_client_fire_path_post_loop` and `main.rs`'s `advance_predicted` caller pass the static world today.
- `AiTickInputs` may need a separate lifetime for the combined world.
- `CombatQuery.collision_world` is not an `Option`, and `placement_cache` batches queries by comparing its pointer; batch on the static world and the pose source instead.
- March: one step per host tick at the point host projectiles advance, starting on the tick after FIRE as a host-spawned projectile does. Release a declaration from there, not from either drain. A drain-time step would double-step, step on the FIRE tick, or read the previous tick's poses (P1).
- The rival to putting the march in the projectile stage is a netcode step called from the frame loop; parity then needs care.
- First slice: the combined queries, then hitscan host and client end to end, including host acceptance. It falsifies the threading and pose-source assumptions before the fan-out.
- Files past ~800 lines: `weapon/mod.rs`, `sim/mod.rs`, `projectile_stage.rs`, `netcode/src/lib.rs` and `collision/moving.rs` take threading edits only. New logic goes in new modules.

## Open questions
- Where march state lives (on the open shot or beside it), which crate owns the step, and whether `projectile_static_contact_distance` is deleted or renamed as it retires from the hold — **delegated**.
