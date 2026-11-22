# E17--movers-block-shots-and-sight

Brief · resumable · Epic 17 · reads: `context/lib/networking.md` §Combat authority, §World-LOS-only validation · `context/lib/entity_model.md` §Components (Splash), §7c · `context/lib/development_guide.md` §Workspace · `in-progress/E10--ai-tick-scheduling/` · `research.md` · read at e7438238, AI rows at E10 branch 768446184

**Sequence:** builds after `E10--ai-tick-scheduling` merges; re-read the AI rows at its merge commit.

## Problem
Owner decision (2026-10-30), now a live defect: since `E17--enemy-mover-interaction` landed, an enemy held at a closed door still sees and shoots through it, and players' bullets, rockets and blasts pass through doors, sliding walls and rising rooms. Cause: every shot and sight query reads the static collision world; movers join only the combined static-plus-mover world that movement, steering and foot probes use. When done, a mover at its pose stops hitscan, projectiles, muzzle launch, splash damage and NPC sight, for the host and a connected client alike; an open gap passes as empty space. A connected client's rocket lands its authoritative blast where the client saw it hit whenever the host's path agrees, unless the host has a wall or door nearer on that path. Otherwise a level with no movers behaves exactly as today.

## Decisions
- **Combined line of sight and sphere cast in `postretro-physics`** (`development_guide.md` §Workspace). Line of sight matches static `line_of_sight` on the static part. The exact sphere cast acts as a ray at zero radius, and the nearest of static and mover wins. Both are public, with a public read of whether a hit came from a mover; mover-aware enemy cover (roadmap E17, `drafts/E10--enemy-cover`) consumes them next.
- **A mover contact is a world contact**: same tie rule (world wins), impact presentation, host validation and zero-radius splash nudge. No consumer branches on hit source.
- **Each vantage reads one pose source.**

  | Query | Vantage | Mover pose |
  |---|---|---|
  | Hitscan, projectiles, muzzle and convergence, splash (host player, NPCs, traps); NPC sight; the rebuild of a client's FIRE launch | Host tick | The tick's published table, the same for every stage of the tick |
  | Own hitscan, muzzle and convergence, predicted projectile | Connected client | Its predicted mover table, the one its movement prediction reads; never the presented pose, which is render-only (`ready/E17--mover-relative-rider-presentation`) |
  | Client splash shot (host blast) | Host | Each tick's published table as the march reaches it |
  | Hit validation | Host | Static only (`networking.md` §World-LOS-only validation, kept) |

- **Validation stays static because the shooter's own cast sees movers.** A host mover check would reject honest hits near a moving edge, where host and predicted poses differ by up to a round trip of travel; catching a lying client is anti-cheat (`index.md` §4).
- **No blast starts inside a mover.** A projectile that begins its step inside a mover's posed hull, because a door closed or snapped onto it, detonates that tick. Any blast centred inside a hull damages the side the projectile came from, and the mover shields the far side. This covers host projectiles, client predicted projectiles and the host march.
- **The host marches each open client splash shot.** Each host tick, in the Projectiles stage, it steps the shot's frozen path one tick against that tick's poses, testing the world only. The blast lands before AI, on the first tick the march reaches its deciding point (next bullet) with the declaration in hand. This replaces the release-time cast and the FIRE-frozen hold distance (`networking.md` §Combat authority); the rivals are in `research.md`.
- **The host credits the client's on-path impact (owner).** The client's first valid contact record counts when it lies within an on-path tolerance of the host's frozen path; an off-path first record, or no valid record, leaves the host's own march contact or nothing. A host world contact nearer than the declared point wins with its own surface, as does a host contact at a declared world point. Otherwise the blast lands at the declared point: a declared enemy still live there is its impact target, so the weapon's direct push composes with splash as for a host rocket; else the declared normal applies. This reverses `networking.md` §Combat authority's rule that the client never selects a splash centre: the client now selects which on-path point counts, and the host still emits one authoritative blast.
- **Observers never see a client rocket pass a door, hang still, or flash where no blast lands.** Its display flight steps with the march, and on the tick the march reaches its deciding point, or its world contact first, the rocket vanishes there with no contact flash; the burst shows with the blast.
- **Movers occlude splash; entities still do not.** Reverses `entity_model.md` §Components (Splash) for movers.
- **Every NPC sight test reads the combined world**: perception, the fire gate, awareness meters, dormant checks and combat-slot scoring, for enemies and allies alike. Sight timing stays E10's.
- **Placement stays static.** Combat-slot capsule placement and the other placement queries are not shot or sight tests. A spawner or trap authored inside a mover's swept path stays a content error.
- **Docs.** At promotion: `networking.md` §Combat authority and `entity_model.md` §Splash. At landing, every human-facing claim that movers pass shots, splash or sight:
  - in `docs/scripting-reference.md`, the splash trigger and splash notes, the knockback "static-world occlusion" line and `brain.targetVisible`;
  - the hand-written `brain.targetVisible` guard comments in `sdk/lib/brain.ts` and `brain.luau`;
  - the typedef templates and the hitscan resolution-mode doc comment in the scripting primitives, regenerated into `sdk/types/` and their test fixtures;
  - the doors note in `docs/level_design.md`;
  - `content/dev/maps/mover-setpiece.README.md`.

  Names and shapes do not change, so there is no scripting surface.
- **Non-goals:**
  - A door-aware mask on the cell-visibility relation (roadmap E17-F). That is a later broad phase, which must err toward visible and never reject a line this cast passes.
  - Rewinding movers to a shooter's past tick (`index.md` §4).
  - Mover-specific impact presentation.
  - Projectiles that pause in front of the shooter at launch in net play (owner observation), and the hold-at-contact that every other observed projectile shows today. Both are separate presentation defects this brief neither causes nor fixes; the observer bullet covers client splash rockets only.

## Acceptance

### Automated
Queries:
- [ ] Each combined query, against a closed mover, an open gap, a partly open mover, a rotated mover, two movers on one line and an origin inside a mover hull, returns the posed-geometry result: blocked at the nearest surface, clear through the gap, and today's static outcome for an origin inside geometry.
- [ ] With no movers, each combined query returns the static query's exact result, including a hit at the aim point counting as clear and a degenerate segment counting as blocked. A zero-radius combined sphere cast returns the combined ray's hit.
- [ ] Neither combined query allocates on the heap per call.

Host shots:
- [ ] Host-player and NPC hitscan stop at a closed mover with a world contact, and the target behind takes no damage. Through the gap the target is hit. A mover and an entity at equal distance give the world contact.
- [ ] Host-player, NPC and trap projectiles impact a closed mover, including a mover that moves into a projectile already in flight. A mover that opens before arrival lets it through.
- [ ] A composed muzzle on the far side of a mover from the eye launches from the eye. A muzzle clear of a closed door the eye sees aims at the door point, not the wall behind: for the host player, a client in its prediction, and the host's rebuild of that client's FIRE. A client muzzle across a door closed on the host launches the host's rebuilt FIRE, and its march, from the eye.
- [ ] A projectile a door closes or snaps onto detonates that tick: for a host rocket, a client predicted rocket and the host march. A blast centred inside a door's hull, whether the door enclosed the projectile or closed over a client blast's centre between the march's contact and release, damages a target on the side the projectile came from and none on the far side (P12, P21). A rocket launched inside a door's hull damages a target behind its flight direction and none ahead, and detonates even with zero lifetime (P30, P31).
- [ ] A mover between blast and target blocks splash damage, and a target on the blast side takes it. A zero-radius blast on either face of a rotated door damages that face's side only.
- [ ] On the tick a door closes, a host rocket in flight, an NPC's sight and the host player's hitscan all meet it, including when the blocking pass reverses the door that tick (P15).

Connected client:
- [ ] A client's hitscan stops at the mover in its prediction and declares a world contact, which the host accepts. With the mover open, the client hits the target behind and the host accepts. A mover the host has already closed, still open in the client's prediction, does not refuse the client's hit.
- [ ] Client predicted projectiles meet movers as host ones do, and a client muzzle across a mover closed in its prediction launches from the eye.
- [ ] A client rocket striking a closed door in front of a wall: the host blast lands at the door, never at the wall, on the tick the march reaches it, before that tick's AI. A host rocket and a client rocket fired on one tick along one path meet a door moving across it on the same tick, and the march takes no step on the FIRE tick (P1). A client rocket declared at its own launch point, or launched inside a door's hull, detonates on the tick after FIRE, before AI (P22). A host rocket and a client blast that reach one target on one tick apply the client blast first, whether the march reached its point that tick or earlier; kill credit follows (P24).
- [ ] A door moving into a client rocket's path mid-flight: the host detonates on the door, within the client's mover-prediction error plus one client frame's mover travel (P14). A door that opens before the rocket arrives: the host detonates at the wall behind.
- [ ] A client rocket at a closed door releases at the door whether its declaration arrives before or after the march reaches the door. A declaration arriving after the march has passed its declared point detonates there on its arrival tick. Each time the door shields its far side (P4, P5, P16).
- [ ] A door that closes behind a client rocket after the march passed it does not detonate it when the client declared a contact beyond the door (P2). A client that saw the door close in front of its rocket gets the blast at the door's plane (P20). A door the rocket struck that opens before release still takes the blast there; a target behind the door's old position is damaged, because occlusion reads the release tick's poses (P3, P6).
- [ ] Client and host disagree on a door: the client predicts it open and declares the wall, the host has it closed, and the blast lands at the door. Reversed, the blast lands at the client's declared point (P11). A zero-radius client rocket at a wall or door the host also has damages the blast side and shields the far side, whether its declared point is exactly at the host's contact or a hair short of it (P18).
- [ ] A client rocket declared on an enemy behind a door the host closed first detonates at the door, and the enemy takes no damage (P7). One declared on an enemy that the host's flight passes by detonates at the declared point, and that enemy takes the weapon's direct push and the blast, as from a host rocket (P8). The same holds for an enemy pressed against a wall the host also has (P28). One killed by a teammate before release still detonates at its declared point, not at the wall behind, with no direct push (P27). One whose host flight ran out of range before the declaration arrived still detonates at the declared point when the march covered it (P19).
- [ ] A declared point off the host path, as after a late start from the live eye, detonates at the host's own first march contact, mover or wall, never in open air (P17). A march that reaches range or lifetime before its own contact and any on-path declared point sets off no blast, and the hit is refused; a contact in its final segment detonates there (P9, P10). A declaration is judged on its first valid record: when that one is off the host path, a later on-path record does not count (P23). A declaration whose records are all invalid, an empty one even after the march found a contact, and one that never arrives set off no blast (P29, P33).
- [ ] A client rocket fired without a late start, from a muzzle off the eye, at an enemy that moved between the client's view and the host's, detonates at its declared point with that enemy as impact target, at short and full range (P25). A declared point just inside the on-path tolerance detonates there, short of a host wall beyond it; one just outside detonates at the host's own march contact (P26).
- [ ] An observer's view of a client rocket ends at the door on the tick the host's march meets it, including when the declaration arrives ticks later, with no contact flash. Until the blast it shows neither the rocket passing the door nor the rocket hanging still; a blast at a declared point behind where the flight ended appears there (P32).
- [ ] A shot's march state is gone once the shot closes or expires, its client leaves, its pawn is removed, or the level unloads. A shooter that dies and respawns keeps its open shot and march. A resolved march casts nothing more while its shot waits. With no open client splash shots, the host does no march work.
- [ ] With no movers, no entity across the host's path, and the host launching where the client did, a client world-contact blast lands at today's centre, on no later a tick. Deliberate changes on such levels: a declared enemy the host's flight misses, or one behind another entity on the host path, detonates at its declared point; a declared world point on the host path with no host contact detonates there, where today the hit is refused. The netcode splash-hold tests that seed the retired FIRE-frozen wall distance, and the remote splash tests that refuse an in-air declared point or centre on a host entity before the declared point, are rewritten to these rules.

NPCs:
- [ ] An NPC facing a target behind a closed mover does not acquire it, fill its awareness meter, or fire; through the gap it acquires. A dormant check behaves the same.
- [ ] A door closing on an NPC's retained target: shots inside the grace window hit the door, and the NPC stops firing after it (P13). When the door reopens the NPC fires on its first run after. A target its graph dropped while the door was closed is acquired again only once its awareness meter refills at its fill rate from where the closure drained it, never on the first passing check after the door opens.
- [ ] A combat slot behind a closed mover scores as having no line of sight; one through the gap scores as clear. With a closed mover across a candidate slot's line, the cached and uncached slot batches choose the same slots, and capsule placement chooses the same positions as without the mover. An allied NPC behaves as an enemy does.

Unchanged:
- [ ] Static line of sight still ignores movers. Host hit validation accepts and refuses exactly what it does today.
- [ ] Grep gate: every hitscan, projectile, muzzle, splash and NPC sight site calls a combined query; static line of sight remains only in host hit validation. No shot, splash or sight consumer branches on whether a hit came from a mover, and that read is public outside the physics crate.
- [ ] Grep gate: every client shot path (hitscan, muzzle, predicted projectile) builds its mover poses from the table its movement prediction reads, and none from a presentation pose.
- [ ] Grep gate: none of the files the Docs decision lists still says movers pass shots, splash or sight.
- [ ] On levels with no movers, the existing weapon, projectile, splash, perception, awareness and determinism suites pass unchanged, apart from the rewritten tests above, and headless byte identity holds (regression guard).

Scale (measured, not gated; `testing_guide.md` §Resource bounds; on the machine class, worker count and cache mode of E10's runs):
- [ ] On E10's multi-arena and packed stress rows, with closing doors added in each arena: NPC line-of-sight cost per acquisition check and per tick, against the same rows without movers.
- [ ] On `mover-setpiece` with its doors cycling: host march cost per tick with four clients firing splash weapons continuously, against the host player firing the same weapon at the same rate.

### Manual
- [ ] Listen host and one client with simulated lag, on `content/dev/maps/mover-setpiece.map`: fire at the sliding wall from both sides, closed and open. Rocket the moving wall, and check that the shooter's burst and the observer's land together.
- [ ] On `content/dev/maps/enemy-movers.map`: an enemy held at a closed door neither notices nor shoots the player through it, and engages once it opens.

## Path
- Seams:
  - Physics: `collision::moving` (`CombinedCollisionWorld`, `MoverPoseSource`, `cast_ray_combined`, `cast_capsule_combined`, `mover_bound_may_reach_segment`). `point_inside_mover` gives the enclosed check; it is `pub(crate)`, and it is valid only after a contact query comes back clear. `CombinedCastHit.source` is `pub(crate)`. Also `collision::{line_of_sight, cast_sphere_exact}`. Put the new queries in a sibling module.
  - Weapon: `resolve_world_hit`, `NearestHitQuery` (its `resolve_client_hitscan` doc comment is stale), `fire_hitscan`, `tick_resolved_component`, `resolve_activation_shot[_owned]` and `resolve_muzzle_launch_origin` in `crates/sim/src/weapon/`. Also `weapon::client_resolution::resolve_client_shot` and `weapon_stage::commands::{local, remote}`. The remote commands rebuild a client FIRE's launch and the frozen static distance.
  - Projectiles: `projectile_stage::{advance, advance_predicted, advance_matching, resolve_projectile_impact, nearest_projectile_hit, projectile_static_contact_distance}`. `nearest_projectile_hit` already separates the world cast from the entity test. The range and lifetime arithmetic is inline in `advance_matching`. Splash: `splash::{entities_in_sphere, emit_splash_damage}`. The zero-radius nudge (`projectile_splash_occlusion_origin`) reads the impact normal.
  - Netcode: `HostHitIngestContext`, `resolve_authorized_splash_projectile_impact`, `shot_records::projectile_hold_ticks`, `OpenAuthorizedShots`, and `HostProjectilePresentations` for the observer flight. Its `note_contact` holds the rocket at the point until peers acknowledge and publishes a contact flash, and the flight spawns a tick ahead of the march; client splash rockets need a vanish-without-flash end and a march-aligned start. Declarations drain before Movers in the frame loop and again in the post-sim flush; both ingest with the static world today. Ingest runs at the head of the Projectiles stage, where its closure needs the pose source passed in.
  - AI: the AI sites in `research.md`.
- Host plumbing:
  - The Ai stage already borrows the table (`MoverNav { poses }`).
  - SteerAgents builds a combined world before its `&mut` `yield_agent_as_corpse`. Build the Projectiles- and Ai-stage worlds the same way, from shared borrows that end before SteerAgents.
  - Weapons follows SteerMovers' `split_for_blocking`.
- Client plumbing: `App` builds the prediction world from `collision_world`, `kinematic_mover_colliders` and `kinematic_mover_tick_states`. `run_client_fire_path_post_loop` and `main.rs`'s `advance_predicted` caller pass the static world today.
- `AiTickInputs` may need a separate lifetime for the combined world.
- `CombatQuery.collision_world` is not an `Option`, and `placement_cache` batches queries by comparing its pointer; batch on the static world and the pose source instead.
- March: one step per host tick where host projectiles advance, starting on the tick after FIRE as a host-spawned projectile does, and release from there, not from a drain. A drain-time step would double-step, step on the FIRE tick, or read the previous tick's poses (P1).
- First slice: the combined queries, then hitscan host and client end to end, including host acceptance. It falsifies the threading and pose-source assumptions before the fan-out.
- Files past ~800 lines: `weapon/mod.rs`, `sim/mod.rs`, `projectile_stage.rs`, `netcode/src/lib.rs` and `collision/moving.rs` take threading edits only. New logic goes in new modules.

## Open questions
- The on-path tolerance's value. Its floor covers a non-late FIRE's divergence, including the muzzle offset between the client's and the host's convergence (P25), and it must reject a late start's divergent path — **delegated**.
- Where march state lives (on the open shot or beside it), which crate owns the step, and whether `projectile_static_contact_distance` is deleted or renamed as it retires from the hold — **delegated**.
