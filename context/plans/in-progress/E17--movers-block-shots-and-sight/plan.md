# E17--movers-block-shots-and-sight — plan of record

mode: resumable
status: blocked
read at: 6a2a724b

## Block

One Acceptance row needs restatement (Host shots, row H1). The row names **NPC hitscan**, which no source path produces:

- `crates/ai/src/brain_programs.rs:441` keeps only `ResolutionMode::Projectile` weapons for behavior attacks. A hitscan weapon is refused with the `[AI] behavior attack … references weapon …` warning (test `brain_programs.rs` ~968–1004).
- NPC fire launches a projectile from `perception.enemy_eye` along the eye-to-target-aim line (`crates/ai/src/compute.rs:732`, `weapon_controller.rs` ~290–325). It never calls `fire_hitscan` or the muzzle composer.

So NPC shots reach movers only through the projectile stage, which row H2 already covers. Decisions are unaffected: the Decision table's "(host player, NPCs, traps)" lists who fires across the hitscan, projectile, muzzle and splash queries, not that each fires hitscan.

**Proposed wording for H1:**

> Hitscan stops at a closed mover with a world contact, and the target behind takes no damage. Through the gap the target is hit. A mover and an entity at equal distance give the world contact. Proven for the host player, the only hitscan shooter today; the grep gate holds any later hitscan site to the combined query.

Shooter-neutral on purpose (owner, 2026-11-20): NPCs have no hitscan today and the owner prefers they don't, but may add it. Every shooter's hitscan resolves through `fire_hitscan` (`entity_model.md` §Weapon vocabulary: no actor branch), and row U2's gate covers any new hitscan site.

On the owner's approval of this wording (or other wording), set `status: proposed` and report the plan for the resumable skim.

## Corrections

- Path: "`AiTickInputs` may need a separate lifetime for the combined world" → `AiTickInputs` (`crates/sim/src/ai_host.rs:63`) already carries `mover_nav: MoverNav { gate, movers, poses }` beside `collision_world`, all at `'a`, and `tick.rs` fills `poses` with the tick's table. Planning around it by building the combined world inside AI from `collision_world` plus `mover_nav`. No new field and no new lifetime.
- Path: "`placement_cache` batches queries by comparing its pointer; batch on the static world and the pose source instead" → `BatchPlacements::for_batch` (`combat_positioning/placement_cache.rs:39`) already requires equal `collision_world`, `mover_nav.movers` and `mover_nav.poses` pointers and an equal gate state, and `CombatQuery` already carries `mover_nav`. Planning around it by building the combined sight query from each query's own fields in `scoring::score_candidate_with` and `uncached_reference`. Batching is unchanged.
- Path lists `sim/mod.rs` for threading edits → the stage sequence lives in `crates/sim/src/sim/tick.rs` (773 lines). Planning around it by putting the Projectiles-, Ai- and Weapons-stage world construction there. `sim/mod.rs` takes re-exports only.
- Path: "`resolve_client_hitscan` doc comment is stale" → confirmed: `weapon/mod.rs:1123` names `resolve_client_hitscan`; the client path is `weapon::client_resolution::resolve_client_shot`. Fixed with the hitscan threading in task 2.
- Brief Decision on muzzle and convergence covers "host player, NPCs, traps" → only the host player, a client in its prediction, and the host's rebuild of a client FIRE compose a muzzle (`resolve_projectile_launch_pose`). Traps and NPCs launch from their own origin with no world query. Planning around it by threading the combined world through `resolve_projectile_launch_pose` and its three callers only. Same meaning as the Decision.

Seams re-read and confirmed as the brief states them: `CombinedCollisionWorld`, `MoverPoseSource`, `cast_ray_combined`, `cast_capsule_combined`, `mover_bound_may_reach_segment`, `point_inside_mover` (`pub(crate)`; valid only after a clear contact query), `CombinedCastHit.source` (`pub(crate)`), static `line_of_sight` and `cast_sphere_exact` (`collision/mod.rs`), `physics` installs `alloc_probe`; `resolve_world_hit`, `NearestHitQuery`, `fire_hitscan`, `tick_resolved_component`, `resolve_activation_shot[_owned]`, `resolve_muzzle_launch_origin`; `weapon_stage::commands::{local, remote}`, with `remote.rs` freezing `projectile_static_contact_distance`; `projectile_stage::{advance, advance_predicted, advance_matching, resolve_projectile_impact, nearest_projectile_hit, projectile_static_contact_distance}`; `splash::{entities_in_sphere, emit_splash_damage}` and `projectile_splash_occlusion_origin`; `HostHitIngestContext`, `resolve_authorized_splash_projectile_impact`, `shot_records::projectile_hold_ticks`, `OpenAuthorizedShots`, `HostProjectilePresentations::note_contact`. The frame loop drains declarations before Movers (`frame_loop/mod.rs:808`), ingests at the head of the Projectiles stage through the closure (`frame_loop/mod.rs:1041` → `tick.rs:441`), and flushes after the sim (`frame_loop/mod.rs:1148`). The blocking pass reads `split_for_blocking`'s pose view and mutates only the registry, so Projectiles, Ai and Weapons read one published table per tick (P15 premise holds). The client prediction world reads `kinematic_mover_tick_states` (`main.rs:6250`); `run_client_fire_path_post_loop` and `advance_client_predicted_projectiles` (`main.rs:5059`) pass `self.collision_world`. AI sight sites are `perception::{raw_target_perception, perceive_target}` (`compute.rs:184, 246, 349`) and `combat_positioning::{scoring, uncached_reference}`.

## Delegated answers

- **On-path tolerance.** A declared point is on-path when its perpendicular distance to the host's frozen path segment (fire origin along the frozen direction, from 0 to the frozen reach) is at most `radius + |muzzle_offset| + 0.25 m`, computed once and frozen on the shot at FIRE. Reason: on a non-late FIRE both paths start from muzzles on the same eye ray and converge on the same eye line, so they part by at most the muzzle's offset from the eye (P25), plus the eye's prediction error, which 0.25 m covers. A late start parts by the pawn's movement since the start, and the P17 and P26 tests place that beyond the bound.
- **Where march state lives, and who owns the step.** In `postretro-netcode`, in a new `splash_march` module: a `SplashMarches` map keyed by `ShotId`, held by the host beside `OpenAuthorizedShots`. Reason: netcode already owns the open shot, its declaration and the observer flight, and the Projectiles-stage ingest closure it supplies already runs at the march's required point. One march entry is created when a client splash FIRE is recorded. Binding a declaration still retires the open shot, which keeps the one-declaration-per-shot spine, and moves the shot and its first valid record into the entry. The entry steps once per host tick inside that closure, after ingest and before `projectile_stage::advance`, with the tick's pose source passed in. Each step calls the physics combined sphere cast.
- **`projectile_static_contact_distance`.** Deleted, with `AuthorizedShot::projectile_static_contact_distance` and the splash branch of `projectile_hold_ticks`. Reason: the march finds the host's own contact each tick, so the FIRE-frozen distance has no reader. Splash declarations drain at the minimum hold and bind to their march.

## AC-to-proof

Test names are proposed and are fixed when the test lands.

| AC | Row | Proof | Status |
|---|---|---|---|
| Q1 | Combined queries vs closed / gap / partly open / rotated / two movers / origin inside hull | `physics` `combined_queries` tests: `combined_*_returns_posed_geometry_result` per fixture | achievable as stated |
| Q2 | No movers: exact static result (aim-point hit clear, degenerate blocked); zero-radius sphere = ray | `combined_queries_match_static_without_movers`, `zero_radius_combined_sphere_cast_matches_combined_ray` | achievable as stated |
| Q3 | No heap allocation per call | `combined_queries_do_not_allocate` (`alloc_probe`, physics global allocator) | achievable as stated |
| H1 | Host-player and NPC hitscan stop at closed mover … | — | **needs restatement** (see Block) |
| H2 | Host-player, NPC, trap projectiles impact closed mover, incl. mover moving into flight; opening lets through | `sim` projectile-stage mover tests (player, NPC via AI tick, trap stage) | achievable as stated |
| H3 | Muzzle across mover launches from eye; muzzle clear aims at door point (host, client prediction, host rebuild); client muzzle across host-closed door launches rebuilt FIRE and march from eye | `weapon` muzzle tests + `remote.rs` rebuild test + client fire-path test | achievable as stated |
| H4 | Enclosed projectile detonates that tick (host, client predicted, march); enclosed blast damages approach side only (P12, P21); launched inside damages behind, detonates at zero lifetime (P30, P31) | projectile-stage enclosure tests; netcode march enclosure tests | achievable as stated |
| H5 | Mover blocks splash; blast side damaged; zero-radius on either face of rotated door | `splash` occlusion tests with combined world | achievable as stated |
| H6 | Door closing on tick T: host rocket, NPC sight, host hitscan all meet it, incl. blocking-pass reversal (P15) | `tick.rs`-level determinism test through `simulate_tick` | achievable as stated |
| C1 | Client hitscan stops at predicted mover, world contact accepted; open → target hit accepted; host-closed/client-open not refused | netcode client-to-host hitscan tests | achievable as stated |
| C2 | Client predicted projectiles meet movers; client muzzle across predicted-closed mover launches from eye | `advance_predicted` mover test + client fire-path test | achievable as stated |
| C3 | Client rocket at door before wall lands at door on march tick before AI; same-tick host+client rockets (P1); launch-point / inside-hull declaration detonates F+1 (P22); client blast before host rocket, credit follows (P24) | netcode `splash_march` tests through the Projectiles-stage closure | achievable as stated |
| C4 | Door moving into path: detonate on door within prediction error + one frame of travel (P14); opens before arrival → wall | `splash_march` tests | achievable as stated |
| C5 | Declaration before/after march reaches door; after passing declared point (P4, P5, P16) | `splash_march` tests | achievable as stated |
| C6 | Door closes behind (P2); client saw it close (P20); door opens before release, occlusion reads release poses (P3, P6) | `splash_march` tests | achievable as stated |
| C7 | Client/host disagree on door (P11); zero-radius at shared wall/door (P18) | `splash_march` tests | achievable as stated |
| C8 | Declared enemy behind host-closed door (P7); passed-by enemy direct push + blast (P8); enemy against wall (P28); killed enemy (P27); range ran out before declaration (P19) | `splash_march` tests | achievable as stated |
| C9 | Off-path → host march contact (P17); empty march no blast, final-segment contact detonates (P9, P10); first valid record only (P23); invalid/empty/missing declaration (P29, P33) | `splash_march` tests | achievable as stated |
| C10 | Non-late muzzle-offset rocket at moved enemy, short and full range (P25); tolerance just inside / outside (P26) | `splash_march` tolerance tests | achievable as stated |
| C11 | Observer flight ends at door on march tick, no flash, no pass or hang; blast behind flight end appears there (P32) | `projectile_presentation` march-flight tests | achievable as stated |
| C12 | March state gone on close / expiry / client leave / pawn removal / unload; respawn keeps it; resolved march casts nothing; no open shots → no march work | `splash_march` lifecycle tests (cast counter on the step) | achievable as stated |
| C13 | No movers, no entity across path, same launch: same centre, no later tick; deliberate changes; retired hold tests rewritten | rewritten netcode splash-hold and remote-splash tests | achievable as stated |
| N1 | NPC vs target behind closed mover: no acquire, meter fill or fire; gap acquires; dormant same | `ai` perception tests with `MoverNav` poses | achievable as stated |
| N2 | Door closes on retained target: grace shots hit door, firing stops (P13); reopen fires first run; dropped target re-acquired only via meter refill | `ai` tick tests | achievable as stated |
| N3 | Slot behind closed mover scores no LOS, gap clear; cached = uncached; capsule placement unchanged; allied NPC same | `combat_positioning` scoring + cache-equivalence tests | achievable as stated |
| U1 | Static LOS still ignores movers; host validation unchanged | existing `line_of_sight_*` tests + netcode validation suite unchanged | achievable as stated |
| U2 | Grep gate: shot/splash/sight sites call combined queries; static LOS only in host validation; no source branch; source read public | gate test (source scan) in the workspace's existing gate home | achievable as stated |
| U3 | Grep gate: client shot paths build poses from the prediction table, never presentation | gate test (source scan) | achievable as stated |
| U4 | Grep gate: listed docs no longer say movers pass shots, splash or sight | gate test (source scan) over the Docs list | achievable as stated |
| U5 | No-mover suites pass unchanged (except rewritten), headless byte identity | full `cargo test` gate + existing determinism byte-identity tests | achievable as stated |
| S1 | NPC LOS cost per check and per tick, E10 stress rows with doors vs without | measured run, recorded here (`testing_guide.md` §Resource bounds) | measured, not gated |
| S2 | Host march cost per tick, 4 clients firing splash on `mover-setpiece` vs host player | measured run, recorded here | measured, not gated |
| M1 | Listen host + client with lag on `mover-setpiece`: fire at sliding wall both sides; rocket moving wall, bursts land together | owner, in-engine | manual |
| M2 | `enemy-movers`: enemy at closed door neither notices nor shoots; engages once open | owner, in-engine | manual |

## Tasks

Hot paths and their constraints: every combined query is allocation-free (Q3), and the mover broad phase keeps the no-mover path at static cost. The march costs one combined sphere cast per open, unresolved client splash shot per tick, and nothing when no shot is open (C12). NPC sight adds a mover cull per line-of-sight ray, measured in S1.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Physics: new sibling module of `collision::moving` with public `line_of_sight_combined` and `cast_sphere_exact_combined` (static part identical to the static queries; a zero radius acts as a ray; the nearest hit wins, world on a tie), a public hit-source read, and a public enclosure query wrapping `point_inside_mover` for the enclosed check. Q1–Q3. | integrating executor | — | |
| 2 | Thin slice, hitscan end to end: thread `CombinedCollisionWorld` through `NearestHitQuery` / `resolve_world_hit` / `fire_hitscan` / `tick_resolved_component` / `resolve_activation_shot[_owned]` / `resolve_client_shot` / weapon commands. The Weapons stage builds it from the `split_for_blocking` pose view in `tick.rs`; the client fire path builds it from `kinematic_mover_tick_states`. Host validation stays static. Fix the stale doc comment. H1 (restated), C1, U1. Falsifies the threading and pose-source assumptions. | integrating executor | 1 | |
| 3 | Muzzle and convergence: `resolve_projectile_launch_pose` / `resolve_muzzle_launch_origin` on the combined world for the host player, the client prediction and `remote.rs`'s rebuild. H3, C2 (muzzle half). | integrating executor | 2 | |
| 4 | Projectiles and enclosure: `advance` / `advance_predicted` / `advance_matching` / `resolve_projectile_impact` on the combined world, with Projectiles-stage world construction in `tick.rs` (shared borrows ending before SteerAgents) and the client caller in `main.rs`. Enclosed-at-step-start detonation with reverse-flight side resolution in a new module. H2, H4 (host and client parts), C2. | integrating executor | 2 | |
| 5 | Splash occlusion: `entities_in_sphere` / `emit_*splash_damage` on the combined world. An enclosed centre takes its occlusion origin from the hull exit along the reverse flight. Zero-radius nudge on rotated faces. Amend the `splash.rs` doc comment. H5, H6. | integrating executor | 4 | |
| 6 | NPC sight: `perception` sites and `combat_positioning::{scoring, uncached_reference}` on the combined world built from `collision_world` plus `MoverNav`. N1–N3, H6 (sight part). | delegated worker (worktree; `crates/ai`, `crates/sim/src/combat_positioning*`) | 1 | |
| 7 | Host march: `netcode::splash_march` (entry at FIRE, step in the Projectiles-stage closure after ingest, credit rule with the frozen tolerance, release before AI, lifecycle cleanup). Delete `projectile_static_contact_distance` and the splash hold. Rewrite the retired netcode splash tests. H4 (march part), C3–C10, C12, C13. | integrating executor | 4, 5 | |
| 8 | Observer flight: march-aligned client splash flight that vanishes at the deciding point with no contact flash; the burst shows with the blast. C11. | integrating executor | 7 | |
| 9 | Grep gates U2–U4 as source-scan tests. | integrating executor | 2–8, 10 | |
| 10 | Docs sweep per the Docs decision (scripting-reference, `sdk/lib/brain.{ts,luau}`, primitive typedef templates regenerated into `sdk/types/` and fixtures, `level_design.md` doors note, `mover-setpiece.README.md`). | delegated worker | 5, 6 | |
| 11 | Scale rows S1, S2: measure and record. | integrating executor | 6, 7 | |
| 12 | Preflight, review panel, fix loop, full gate; record results. U5. | integrating executor | all | |
