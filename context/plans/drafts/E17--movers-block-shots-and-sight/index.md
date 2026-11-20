# E17--movers-block-shots-and-sight

Brief · compact · Epic 17 · reads: `context/lib/networking.md` §Combat authority, §World-LOS-only validation · `context/lib/entity_model.md` §Components (Splash), §7c · `context/lib/development_guide.md` §Workspace · `in-progress/E10--ai-tick-scheduling/` · `research.md` · read at e7438238, AI rows at E10 branch 768446184

**Sequence:** builds after `E10--ai-tick-scheduling` merges; re-read the AI rows at its merge commit.

## Problem
Owner decision (2026-10-30), now a live defect: since `E17--enemy-mover-interaction` landed, an enemy held at a closed door still sees and shoots through it, and players' bullets, rockets and blasts pass through doors, sliding walls and rising rooms. Cause: every shot and sight query reads the static collision world; movers join only the combined static-plus-mover world that movement, steering and foot probes use. When done, a mover at its pose stops hitscan, projectiles, muzzle launch, splash damage and NPC sight, for the host and a connected client alike; an open gap passes as empty space; a level with no movers behaves exactly as today.

## Decisions
- **Two combined queries in `postretro-physics`**, beside the combined ray and capsule casts (`development_guide.md` §Workspace): line of sight, matching static `line_of_sight` on the static part, and an exact sphere cast whose zero radius acts as a ray, nearest of static and mover winning. Both public, with a public read of whether a hit came from a mover; mover-aware enemy cover (roadmap E17, `drafts/E10--enemy-cover`) consumes them next.
- **A mover contact is a world contact**: same tie rule (world wins), impact presentation, host validation and zero-radius splash nudge. No consumer branches on hit source.
- **Each vantage reads one pose source.**

  | Query | Vantage | Mover pose |
  |---|---|---|
  | Hitscan, projectiles, muzzle, splash (host player, NPCs, traps); NPC sight | Host tick | The tick's published table, the same for every stage of the tick |
  | Own hitscan, muzzle, predicted projectile and splash | Connected client | Its predicted mover table, the one its movement prediction reads; never the presented pose, which is render-only (`ready/E17--mover-relative-rider-presentation`) |
  | Client splash shot (host blast) | Host | Each tick's published table as the march reaches it (below) |
  | Hit validation | Host | Static only |

- **Host hit validation stays static** (`networking.md` §World-LOS-only validation, kept). The shooter's own cast sees movers, so an honest client never declares through a closed door. A host mover check would reject honest hits near a moving edge, where host and predicted poses differ by up to a round trip of travel. Catching a lying client is anti-cheat, a non-goal (`index.md` §4). A declared contact on a mover surface passes today's check.
- **The host marches each open client splash shot.** Each host tick it steps the shot's frozen path (origin, direction, speed, radius, range, lifetime from FIRE) with the host projectile's own per-tick contact step, against that tick's poses. The march's first world contact replaces the release-time world cast and the FIRE-frozen static hold distance. A declared world contact releases once the march has resolved (contact, range or lifetime) and the declaration is in hand. Entity contacts keep their release-time test, bounded by the march's world contact. The client still never selects the splash centre. No pose history and no extra traffic. Amends `networking.md` §Combat authority; why not a pose ring: `research.md`.
- **Movers occlude splash; entities still do not.** Reverses `entity_model.md` §Components (Splash) for movers.
- **Every NPC sight test reads the combined world**: perception, the fire gate, awareness meters, dormant checks and combat-slot scoring, enemies and allies alike. Sight timing is E10's, unchanged. Grace counts elapsed ticks, a retained target is seen again on its first run after the mover opens, and a dropped target is re-noticed only through its meter.
- **Static `line_of_sight` stays the static primitive.** Its static callers are host validation and placement: spawner and dropped-item placement, trap launch clearance and combat-slot capsule placement. None of them is a shot or sight test. A spawner or trap authored inside a mover's swept path stays a content error.
- **Docs.** At promotion, `networking.md` §Combat authority and `entity_model.md` §Splash. At landing, the human-facing splash note in `docs/scripting-reference.md` and the doors note in `docs/level_design.md`.
- **Non-goals:**
  - A door-aware mask on the cell-visibility relation (roadmap E17-F). That is a later broad phase, which must err toward visible and never reject a line this cast passes.
  - Testing entities per tick in the march. That would close §Combat authority's declared-entity gap but changes entity-contact semantics.
  - Rewinding movers to a shooter's past tick (`index.md` §4).
  - Entity occlusion of splash.
  - Mover-specific impact presentation.
  - Audio occlusion.
  - Enemy movement against movers (`done/E17--enemy-mover-interaction`).

## Acceptance

### Automated
Queries:
- [ ] Each combined query, against a closed mover, an open gap, a partly open mover, a rotated mover, two movers on one line and an origin inside a mover hull, returns the posed-geometry result: blocked at the nearest surface, clear through the gap, and today's static outcome for an origin inside geometry.
- [ ] With no movers, each combined query returns the static query's exact result, including a hit at the aim point counting as clear and a degenerate segment counting as blocked. A zero-radius combined sphere cast returns the combined ray's hit.
- [ ] Neither combined query allocates on the heap per call.

Host shots:
- [ ] Host-player and NPC hitscan stop at a closed mover with a world contact, and the target behind takes no damage. Through the gap the target is hit. A mover and an entity at equal distance give the world contact.
- [ ] Host-player, NPC and trap projectiles impact a closed mover, including a mover that moves into a projectile already in flight. A mover that opens before arrival lets it through.
- [ ] A composed muzzle on the far side of a mover from the eye launches from the eye.
- [ ] A mover between blast and target blocks splash damage, and a target on the blast side takes it. A blast centred on a mover face damages the blast side only.

Connected client:
- [ ] A client's hitscan stops at the mover in its prediction and declares a world contact, which the host accepts. With the mover open, the client hits the target behind and the host accepts. A mover the host has already closed, still open in the client's prediction, does not refuse the client's hit.
- [ ] Client predicted projectiles and splash meet movers as host ones do.
- [ ] A client rocket striking a closed door in front of a wall: the host blast and damage land at the door on the tick the march reaches it. That point is within the existing splash tolerance of the client's, and the tick is no later than with no wall behind the door.
- [ ] A door moving into a client rocket's path mid-flight: the host detonates on the door, where the client did, within the client's mover-prediction error. A door that opens before the rocket arrives: the host detonates at the wall behind.
- [ ] A client splash shot admitted late after a stall still detonates at the host's own first contact along the live-eye ray, mover or wall.
- [ ] A shot's march state is gone once the shot closes, expires or its client leaves. With no open client splash shots, the host does no march work.
- [ ] With no movers, client splash centres and release ticks are identical to today.

NPCs:
- [ ] An NPC facing a target behind a closed mover does not acquire it, fill its awareness meter, or fire; through the gap it acquires. A dormant check behaves the same.
- [ ] A door closing on an NPC's retained target stops its fire after the grace window. When the door reopens it fires on its first run after. A target its graph dropped while the door was closed is acquired again only after two passing checks in a row refill the meter.
- [ ] A combat slot behind a closed mover scores as having no line of sight; one through the gap scores as clear. An allied NPC behaves as an enemy does.

Unchanged:
- [ ] Static line of sight still ignores movers. Host validation accepts and refuses exactly what it does today.
- [ ] On levels with no movers, the existing weapon, projectile, splash, perception, awareness and determinism suites pass unchanged, and headless byte identity holds (regression guard).

Scale (measured, not gated; `testing_guide.md` §Resource bounds):
- [ ] On E10's multi-arena and packed stress rows, with closing doors added in each arena: NPC line-of-sight cost per acquisition check and per tick, reported against the same rows without movers.
- [ ] Host march cost per tick, reported with four clients firing splash weapons continuously.

### Manual
- [ ] Listen host and one client with simulated lag, on `content/dev/maps/mover-setpiece.map`: fire at the sliding wall from both sides, closed and open. Rocket the moving wall, and check that the shooter's burst and the observer's land together.
- [ ] On `content/dev/maps/enemy-movers.map`: an enemy held at a closed door neither notices nor shoots the player through it, and engages once it opens.

## Path
- Seams:
  - Physics: `collision::moving` (`CombinedCollisionWorld`, `MoverPoseSource`, `cast_ray_combined`, `cast_capsule_combined`, `mover_bound_may_reach_segment`; `CombinedCastHit.source` is `pub(crate)`). `collision::{line_of_sight, cast_sphere_exact}`.
  - Weapon: `resolve_world_hit`, `NearestHitQuery` (its `resolve_client_hitscan` doc comment is stale), `fire_hitscan`, `tick_resolved_component`, `resolve_activation_shot[_owned]` and `resolve_muzzle_launch_origin` in `crates/sim/src/weapon/`. Also `weapon::client_resolution::resolve_client_shot` and `weapon_stage::commands::{local, remote}`.
  - Projectiles: `projectile_stage::{advance, advance_predicted, advance_matching, resolve_projectile_impact, nearest_projectile_hit, projectile_static_contact_distance}`. Splash: `splash::{entities_in_sphere, emit_splash_damage}`.
  - Netcode: `HostHitIngestContext`, `resolve_authorized_splash_projectile_impact`, `shot_records::projectile_hold_ticks`, `OpenAuthorizedShots`.
  - AI: the AI sites in `research.md`.
- Host plumbing:
  - The Ai stage already borrows the table (`MoverNav { poses }`).
  - SteerAgents builds a combined world before its `&mut` `yield_agent_as_corpse`. Build the Projectiles- and Ai-stage worlds the same way, from shared borrows that end before SteerAgents.
  - Weapons follows SteerMovers' `split_for_blocking`.
- Client plumbing: `App` builds the prediction world from `collision_world`, `kinematic_mover_colliders` and `kinematic_mover_tick_states`. `run_client_fire_path_post_loop` and `main.rs`'s `advance_predicted` caller pass the static world today.
- `AiTickInputs` may need a separate lifetime for the combined world.
- `CombatQuery.collision_world` is not an `Option`, and `placement_cache` batches queries by comparing its pointer; batch on the static world and the pose source instead.
- March parity: step at the point in the tick where host projectiles advance, starting on the tick after FIRE, as a host-spawned projectile does. Then a host rocket and a client rocket fired on one tick along one path meet a moving door on the same tick.
- First slice: the combined queries, then hitscan host and client end to end, including host acceptance. It falsifies the threading and pose-source assumptions before the fan-out.
- Files past ~800 lines: `weapon/mod.rs`, `sim/mod.rs`, `projectile_stage.rs`, `netcode/src/lib.rs` and `collision/moving.rs` take threading edits only. New logic goes in new modules: the queries beside `moving.rs`, and the march in netcode.
- At landing, also update `content/dev/maps/mover-setpiece.README.md`, which says movers do not yet block shots.

## Open questions
- Where march state lives (on the open shot or beside it) and the name `projectile_static_contact_distance` takes as it retires from the hold — **delegated**.
