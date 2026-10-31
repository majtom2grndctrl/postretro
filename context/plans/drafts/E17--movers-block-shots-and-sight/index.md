# Movers Block Shots and Sight

## Goal

A closed door, a sliding wall or a rising room stops bullets, projectiles, blast damage and enemy sight. Today every shot and sight query reads static geometry only, so players and enemies shoot and see through movers, and an enemy "waiting at a closed door" can still fire through it. Owner decision (2026-10-30): movers stop shots and sight.

## Scope

### In scope

- Combined static + mover line-of-sight and sphere-cast queries in physics, beside the existing combined ray and capsule casts.
- Hitscan: host-simulated shots (host player, enemies) and a connected client's own predicted shot.
- Projectiles: host advance (player, enemy and trap shots), client predicted advance, the host's splash replay of a client projectile, and the FIRE-frozen static contact distance on the host.
- Muzzle launch origin: a composed muzzle never launches through a mover.
- Splash occlusion: a mover between blast and target blocks damage.
- Enemy AI: target perception (and through it the fire gate) and combat-slot line-of-sight scoring.
- Human-facing docs that state movers do not occlude.

### Out of scope

- **Host hit validation stays static-only.** `networking.md` §World-LOS-only validation keeps its rule; see Direction.
- Enemy movement against movers (carry, block, crush, nav gating, lifts): `drafts/E17--enemy-mover-interaction/`.
- Placement checks that ignore movers: spawner placement, dropped-item placement, trap launch clearance, combat-slot capsule placement. Each places an entity; none is a shot or sight test. A spawner or trap authored inside a mover's swept path is a content error today and stays one.
- Mover-specific impact presentation (material, sparks per surface). A mover hit presents as a world contact, which is what any static hit presents today.
- Rewinding mover poses to a shooter's past tick (lag compensation). Non-goal per `index.md` §4.
- Audio occlusion.

## Direction

**Problem.** Shot and sight queries are written against `CollisionWorld`, which holds static geometry only. Movers live outside the static BVH, and only player movement threads the combined static + mover world.

**Prior commitments.**

- `networking.md` §World-LOS-only validation: the host validates a declared hit against static geometry only, "never against other dynamic occluders", to avoid false rejections from pose staleness. **Kept.** The shooter's own cast now includes movers, so a legitimate client never declares a hit through a closed door. A host re-check against movers would reject legitimate hits near a moving edge, because the host's mover pose and the client's predicted pose differ by up to one round trip of travel. Catching a client that lies about doors is anti-cheat, a non-goal (`index.md` §4).
- `entity_model.md` §Components (Splash): "Static world geometry occludes; movers and entities do not." **Reversed for movers** (owner decision); entities still do not occlude.
- `collision::line_of_sight` documents that movers are deliberately excluded, and a test pins it. **Kept** as the static primitive; this plan adds a combined sibling and moves shot and sight callers to it. Static callers that remain are host validation and placement.
- `development_guide.md` §Workspace: mover collision lives in `postretro-physics`. New queries land there.
- `done/E17--doors-as-occluders` and `cell-visibility-relation` reserve a later door-aware mask on the cell-visibility relation for coarse coupling (AI perception broad phase, relevance), and roadmap E17-F forbids a parallel dynamic-visibility path. This plan is the narrow-phase test, not a second visibility system: the geometric cast is the authority for a given line. A future mask is a broad-phase pre-filter and must err toward visible, so it never rejects a line the cast would pass (a half-open door).
- Mover poses: host stages read the tick's published `MoverTickStateTable`; clients predict movers in the present (`networking.md` mover prediction). The only new pose source is a host-side ring of past published poses, used by the splash replay (Task 3).

**Alternatives rejected.**

- *Fold movers into `CollisionWorld`.* Every static consumer (bakes, nav, validation, placement) would change meaning, and the world would need a per-tick rebuild or refit. The combined-world wrapper already exists for exactly this split.
- *Host-only mover occlusion.* HIT is a client-authoritative declaration (`networking.md` §Combat authority), so a client would still hit through doors. The shooter's cast must see movers.
- *Decide blocking from mover state instead of geometry* — a docked-closed mover blocks a line crossing a portal it seals, reusing the doors-as-occluders association. It misses partly open, sliding, rising and rotating movers (E3, E11) and movers that seal no portal, and projectiles and splash need a geometric contact point anyway, so a state test cannot replace the cast.
- *Host replays splash against its current mover pose.* Cheapest, but against a moving door the host detonates somewhere the shooter never saw, and every observer sees the burst there. Owner rule (2026-10-30): give co-op players the smoothest presentation the network allows. The time-correct replay needs no extra traffic, only a host-side pose ring.
- *Let the client choose the splash centre.* Smooth for the shooter, but it reverses `networking.md`'s rule that "the client never selects a splash center", which keeps one authoritative blast for every peer. The time-correct replay gets the same point without giving that up.
- *Test client shots against the presented (interpolated) mover pose.* The rendered mover can sit up to one tick from the predicted table. The extra error is under 0.1 m for a 6 m/s mover at 60 Hz, and the table is already in hand on the client. Revisit only if playtests show edge misses.

## Mover pose per vantage

| Query | Vantage | Mover pose read |
|---|---|---|
| Hitscan, projectiles, splash, muzzle (host player, enemies, traps) | Host tick | The tick's published pose table. One pose for every stage of the tick: the blocking pass reads poses but does not write them |
| Enemy perception, combat slots | Host tick, AI stage | Same table, same tick |
| Own hitscan, muzzle, predicted projectile advance, predicted splash | Connected client | The client's current mover table (latest predicted tick) |
| Splash replay of a client projectile; FIRE-frozen contact distance | Host | Each mover at its pose on the host tick the replayed projectile reaches it, read from a short host mover-pose history (time-correct replay) |
| Hit validation | Host | Static only (unchanged) |

## Orderings and edges

| # | Scenario | Expected outcome |
|---|---|---|
| E1 | Mover closed and at rest across the line | Blocked for every query and every vantage in the table above (validation excepted) |
| E2 | Mover open, line passes through the gap | Passes; result identical to the no-mover case |
| E3 | Mover partly open; line crosses the remaining brush | Blocked at the brush surface |
| E4 | Mover moves into the line during the client's latency window | Client's own cast decides: client sees the hit or block it computed; host accepts a declared world contact on the mover because validation is static |
| E5 | Client splash projectile strikes a mover at rest | Host replay detonates at the same point within the existing splash tolerance |
| E6 | Client splash projectile strikes a mover in motion | Host replay tests the mover at its pose on the tick the projectile reaches it, so it detonates where the client saw it, within the client's mover-prediction error. The shooter's predicted burst and the observers' host cue land together |
| E6a | Replay reaches back past the host pose history | Uses the oldest retained pose and warns once per level load; the history bound covers the longest splash projectile lifetime, so this is a content outlier |
| E7 | Mover hit and entity hit at equal distance | World wins (existing tie rule); the mover counts as world |
| E8 | Blast centred on a mover face | Occlusion origin nudged off the face along the normal (existing zero-radius rule); entities on the blast side take damage, entities behind the mover do not |
| E9 | Shot origin (eye or muzzle) inside a mover hull | Same outcome a ray or sphere starting inside static geometry gets today; pinned by test, not redesigned |
| E10 | Door closes while an enemy sees its target; later reopens | Sight lost, then regained, each after the existing perception debounce; no fire while lost |
| E11 | Rotating mover crosses the line | Blocked by its posed geometry, same as a translating mover |
| E12 | Level with zero movers | Every query returns exactly today's result |
| E13 | Two movers on one line | Nearest mover surface wins |

## Acceptance criteria

- [ ] A host-simulated hitscan shot (host player or enemy) stops at a closed mover: a world contact at the mover surface, no damage to the target behind it. Through an open gap the target is hit (E1–E3, E11, E13).
- [ ] A connected client's own hitscan shot stops at the mover in its prediction, declares the world contact, and the host accepts that contact. With the mover open, the client hits the target behind and the host accepts the hit (E4).
- [ ] Player, enemy and trap projectiles impact a closed mover, including a mover that moves into a projectile already in flight. Client predicted projectiles do the same.
- [ ] A composed muzzle on the far side of a mover from the eye launches from the eye origin instead, as it does today for static walls.
- [ ] A mover between blast centre and target blocks splash damage; a target on the blast side still takes it (E8). The host's splash replay of a client projectile detonates where the client's did, against a mover at rest and against one in motion within the client's mover-prediction error (E5, E6).
- [ ] An enemy cannot perceive or fire at a target behind a closed mover, and does not score a combat slot as having line of sight through one. Sight returns within the existing debounce after the mover opens (E10).
- [ ] Static `line_of_sight` still ignores movers, and host hit validation still reads static geometry only.
- [ ] On levels with no movers, existing weapon, projectile, splash, perception and determinism suites pass unchanged (E12). No new query allocates on the heap per call.

## Tasks

### Task 1: Combined line-of-sight and sphere-cast queries

Add two queries over `CombinedCollisionWorld` in `postretro-physics`, in a new sibling module under `collision/` rather than extending `moving.rs` (1614 lines):

- **Combined line of sight.** Static line of sight plus a combined ray from eye to aim. It must match `line_of_sight`'s semantics for the static part (the same reach and the same treatment of a hit at the aim point).
- **Combined sphere cast.** Exact, mirroring `cast_sphere_exact`: a zero radius behaves as a ray, and the nearest of the static and mover hits wins.

Both use the existing per-mover bounding-sphere cull and pose each mover from the supplied `MoverPoseSource`, as `cast_ray_combined` and `cast_capsule_combined` do. Widen visibility of `moving.rs` helpers only within `collision` (`pub(super)`). Expose a public read of whether a `CombinedCastHit` came from a mover, for tests and later consumers. `CombinedCastHit.source` is `pub(crate)` today.

Keep `static_line_of_sight_ignores_a_mover_that_blocks_the_combined_ray` and add combined counterparts: blocked by a posed mover, clear through a gap, a rotated mover, two movers on one line, origin inside a hull (E9), zero movers equal to static. Verify zero heap allocations per call with an alloc probe; the physics crate has none, so assert at a sim-side call site that already uses `alloc_probe`. Satisfies part of AC 7 and AC 8.

### Task 2: Hitscan through the combined world (thin slice)

Thin slice: it crosses physics → host weapon stage → client fire path → netcode acceptance end to end, and falsifies the threading and pose-table assumptions before the fan-out.

Change the hitscan path to take `&CombinedCollisionWorld` where it takes `&CollisionWorld` today: `resolve_world_hit`, `NearestHitQuery`, `fire_hitscan`, `tick_resolved_component`, `resolve_activation_shot` / `resolve_activation_shot_owned`, `resolve_client_shot`, `run_local_weapon_command_with_content` and `run_remote_weapon_commands`. `resolve_world_hit` uses `cast_ray_combined`; a mover hit becomes the same `WorldHit` a static hit makes, so tie handling (E7) and world-contact presentation are unchanged.

Plumbing:
- **Host.** In `simulate_tick_with_presentation_aim` (`sim/mod.rs`), build `CombinedCollisionWorld::new(collision_world, mover_colliders, &*mover_tick_states)` for the Weapons stage after the blocking pass releases its `split_for_blocking` borrow.
- **Client.** `App::run_client_fire_path_post_loop` (`postretro/src/client_weapon/mod.rs`) builds it from `self.collision_world`, `self.kinematic_mover_colliders` and `self.kinematic_mover_tick_states`, the same triple client movement prediction uses.
- **Tests.** Existing callers pass `CombinedCollisionWorld::static_only`.

`weapon/mod.rs` (3983 lines) and `sim/mod.rs` (4479 lines) take threading edits only; no new logic goes in either. Fix the stale `resolve_client_hitscan` doc comment on `NearestHitQuery`. Host validation (`valid_hitscan_world_contact` and the entity-hit validator in `netcode/src/lib.rs`) is untouched. Covers AC 1, AC 2, and part of AC 8.

### Task 3: Projectiles, muzzle and splash

Thread the combined world through:
- **Projectiles.** `projectile_stage::advance`, `advance_predicted` and `advance_matching` → `resolve_projectile_impact` → `nearest_projectile_hit`, which switches to the combined sphere cast.
- **Time-correct host splash replay.** The host keeps a ring of published mover poses by host tick, sized to the longest splash projectile lifetime among loaded descriptors and allocated at level install. The host splash replay `resolve_authorized_splash_projectile_impact` (netcode, via `HostHitIngestContext`) and the FIRE-frozen `projectile_static_contact_distance` step the frozen ray tick by tick from FIRE at the frozen speed, testing static geometry plus each mover at that tick's pose (E6, E6a). Static-only results are unchanged (E12). Rename `projectile_static_contact_distance` if its name now misleads. Record the rule in `networking.md` §Combat authority at promotion.
- **Muzzle.** `resolve_muzzle_launch_origin` switches its eye ray and eye-to-muzzle sweep to the combined queries.
- **Splash.** `splash::entities_in_sphere` takes `Option<&CombinedCollisionWorld>` for `occlude` and uses combined line of sight. `emit_splash_damage` and its callers (projectile stage, netcode host splash HIT) pass it through. The zero-radius world-impact nudge (`projectile_splash_occlusion_origin`) applies to mover impacts unchanged, because they are world impacts.

Host plumbing:
- The Projectiles stage runs before the blocking pass, so build its combined world from a shared borrow of the table that ends before `split_for_blocking`.
- The host ingest context gains the mover colliders and the live table. Name the owner that supplies them on the host app side.
- Client predicted advance builds the world as Task 2 does.

Update the splash doc comment and `docs/scripting-reference.md`'s splash note ("movers … do not occlude") to say movers occlude. `projectile_stage.rs` (2188 lines) and `netcode/src/lib.rs` (8011 lines) take threading edits only. Covers AC 3–5 and E5, E6, E6a, E8.

### Task 4: Enemy sight and combat slots

`AiTickInputs.collision_world` becomes `Option<&CombinedCollisionWorld>`. Build it in `simulate_tick_with_presentation_aim` at the Ai stage from a shared borrow of the tick's table, released before the Steering stage.

Thread it through to:
- `perception::raw_target_perception` and `perceive_target` (`TargetPerceptionQuery.collision_world`), which switch to combined line of sight;
- `resolve_combat_slots`;
- `CombatQuery.collision_world`, so `combat_positioning::scoring::score_candidate` uses combined line of sight.

`capsule_static_placement_center` keeps reading the static world through `CombinedCollisionWorld::static_world`; placement is out of scope. The fire gate reuses the perception verdict and needs no change of its own. The `ai` crate already depends on `postretro-physics`, so no dependency edge is added. Tests:
- an enemy facing a closed mover does not acquire or fire;
- the same enemy acquires through the open gap;
- a door closing mid-sight loses the target after the debounce, and reopening regains it (E10);
- a combat slot behind a closed mover scores as no line of sight.

Covers AC 6.

## Sequencing

**Phase 1 (sequential):** Task 1, then Task 2. Thin slice; falsifies the combined-world threading and the per-vantage pose table.
**Phase 2 (concurrent):** Task 3, Task 4. Task 3 owns the weapon and projectile files; Task 4 owns the AI and combat-positioning files. Both touch `sim/mod.rs` at different stages, so the integrating executor merges those edits.

**Landing order across specs:** this plan lands before or with `E17--enemy-mover-interaction`. Shipped alone, that plan holds enemies at closed doors while they still fire through them. Both edit `combat_positioning/scoring.rs` and the Ai-stage plumbing in `sim/mod.rs`.

## Rough sketch

- The combined queries sit beside `cast_ray_combined` and reuse its broad phase. A sphere cast against a posed `TriMesh` is a parry shape cast with a `Ball`, mirroring the capsule cast helper.
- `CombinedCollisionWorld { static_world, movers, poses }` already carries everything; `static_only` keeps tests and static callers terse.
- `sim/mod.rs` stage order: Movers → Movement → Triggers → Projectiles → Ai → Traps → Steering (blocking pass) → Weapons. `split_for_blocking` is the only `&mut` use of the table after the Movers stage.
- Manual check map: `content/dev/maps/mover-setpiece.map` (sliding wall, rising room). Fire at the closed wall from both sides, and put an enemy at the README's documented spot behind the wall.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Every shot and sight query sees movers at its vantage's pose (vantage table); host validation and placement stay static | Tasks 1–4 | Any new shot or sight caller that takes `&CollisionWorld` | AC 1–7 |
| A mover contact is a world contact: same tie rule, presentation, validation and splash nudge | Task 2 (hitscan), Task 3 (projectiles, splash) | Code that branches on hit source | AC 1, 2, 5; E7, E8 |
| No movers → identical results | Task 1 query semantics | Combined query diverging from static semantics on the static part | AC 8; E12 |
