# E21--player-model-front — plan of record

mode: compact
status: done
read at: 99cc0e0d5

## Corrections
- Brief read at 83802575a → no cited seam changed by 99cc0e0d5; Decision reads reused.
- Worker B's ghost test read yaw from the wrong `to_euler(YXZ)` slot (pitch); fixed in the test.
- Spawn rotation for a non-movement descriptor at a `player_spawn` or network slot (e.g. an AI body) → it now spawns from `model_rotation_quat()` with `BodyFrame::Model`, like a map placement; before, it took the view rotation. A pawn is a movement descriptor on a player path, matching the wire's `pawn:` class. Review finding (adversarial); keeps §4's rule that an entity picks its frame by the axis it treats as forward. `entity_model.md` §4 updated.
- The host role step (`apply_remote_player_viewer_role`) keeps the frame the spawn chose; clients set `View` from their role. Round-2 review finding: re-deriving from descriptors could contradict the transform after a reload.
- Fresh pose inputs written by the foot probe seed aim and heading from the transform yaw (animated meshes), closing the zero-tick first-frame gap on a host slot pawn (three reviewers).
- Converter review fixes: `--fix-eyes` insets along the face direction; `rest_to_library.py` keeps the root's own facing (root-only yaw allowance).
- Pre-existing failures outside this branch's files, not fixed here: `xtask` `mover_shot_gates::static_only_worlds_stay_in_sites_that_never_fire` and `static_line_of_sight_remains_only_in_host_hit_validation` (flag sim weapon / netcode lib sites this branch never touched), and renderer `smooth_specular_matches_the_frozen_pre_change_function` (last-ULP float difference; renderer code untouched). Also fixed on the way: a pre-existing clippy `chunks_exact` lint in a `light_bridge.rs` test (0f28f6c04).
- `main` did not build the `postretro` test target (`area_lights_surface_tests.rs` lacked `frontend_backdrop` after two branches merged) → fixed in its own commit (76a734288) before this work.

## Delegated answers
None (brief has no open questions).

## Contract (integrating executor)
- `postretro_entities::components::mesh::{BodyFrame, MODEL_FRONT_TO_VIEW_FORWARD}`. `MeshComponent::body_frame` is `#[serde(skip)]`, default `Model`; player bodies are `View`. `BodyFrame::model_rotation(rotation)` composes the half turn. `MeshComponent::presentation_rotation(transform_rotation)` is the draw rotation: lower-body heading from pose inputs on an animated mesh, else the transform rotation, then the body-frame turn (pin R1: a stateless mesh's foot-probe default heading never replaces the transform yaw).
- `MapEntity::model_rotation_quat` uses the same constant (placement seam).

## AC-to-proof

| AC | Proof | Status | Result |
|---|---|---|---|
| 1 Player body +Z along aim / heading, every role | mesh_render collector tests per role (SP, host local, host-side remote, client own, client remote) | achievable as stated | pass — `player_body_fronts_model_plus_z_along_aim_then_lower_body_heading`, per-role: `player_pawns_take_the_body_frame…`, `descriptor_accept_marks_the_slot_pawn_body_view…`, `client_player_bodies_front_model_plus_z…` |
| 2 Stateless legged body after pose pass (R1); first frame after spawn, respawn, seam, relevel, rejoin | collector test with pose pass run; materialization-path tests per entry | achievable as stated | pass — `stateless_legged_player_body_draws_along_aim_after_the_pose_pass`, `unposed_view_body_probe_seeds_aim_and_heading_from_the_transform_yaw`, `client_viewer_role_marks_an_existing_player_mesh_view…`; seam/relevel/rejoin are fresh spawns through the same steps |
| 3 Ghost copy + weapon attachment of a player body | `ghost_render_tests.rs` | achievable as stated | pass — `a_ghost_pawn_presents_its_mapped_real_draw…`, `pawn_ghost_group_carries_its_weapon_attachment…` |
| 4 Regression guard: animated enemy, prop, ghost projectile unchanged; no double turn | collector + ghost tests | achievable as stated | pass — `collector_carries_only_entity_pose_inputs`, `placed_model_bodies_draw_at_their_transform_rotation_unturned`, `a_model_bodied_projectile_ghosts_through_the_skinned_ghost_path` |
| 5 Map-placed copy of player descriptor unturned, same install as a pawn (R3) | sim descriptor-spawn test, host + client materialization | achievable as stated | pass — `player_pawns_take_the_body_frame_and_a_placed_copy_of_their_descriptor_stays_model`, client `…world_copy_stays_model` |
| 6 Transform/pose-input/replicated yaws unchanged; zero twist at heading = aim | pose_inputs + mesh_render tests | achievable as stated | pass — `player_body_fronts_model_plus_z…` (pose inputs unshifted, zero twist, transform Ry(θ)) |
| 7 No body-frame key serialized; deserialize yields none | `mesh.rs` serde test | achievable as stated | pass — `player_body_frame_never_serializes_and_a_restored_mesh_has_no_body_frame` |
| 8 exo_red faces +Z in idle (left hand +X of right) | `gltf_loader.rs` facing test | achievable as stated | pass — `exo_red_bound_to_mixamo_rifle_faces_plus_z_in_idle` |
| 9 exo_red joints through idle_aiming/run_forward match pre-change draw (rot within tolerance, pos ≤ 1 mm) | model test vs in-memory Ry(180°) Hips reference | achievable as stated | pass — `exo_red_refaced_body_draws_every_joint_where_the_pre_change_body_drew` (2.8e-7 rot, 0.71 mm) |
| 10 Every committed exo_red bake `--check` VERIFIED; −Z exo_red fails | tool test over the committed bakes + scratch −Z copy | achievable as stated | pass — `every_committed_exo_red_bake_verifies_on_exo_red`, `exo_red_bake_check_fails_on_a_minus_z_exo_red` |
| 11 Pre-change reference built in memory / from 83802575a | same tests as 9–10 | achievable as stated | pass — in-memory (model) and scratch-file (tool) references |
| 12 Hit zone front ray hits, behind misses | `hit_zones.rs` test | achievable as stated | pass — `player_body_front_zone_is_hit_in_front_of_the_pawn_and_missed_behind` |
| 13 Socket anchor `hand_r` lands at drawn socket (heading = aim) | `hit_zones.rs` posed_socket_world test | achievable as stated | pass — `player_body_hand_socket_lands_on_the_drawn_socket_when_heading_is_aim` |
| 14 Heading ≠ aim: zones and anchors on transform + turn (R2) | `hit_zones.rs` test | achievable as stated | pass — `player_body_zones_and_sockets_follow_aim_not_lower_body_heading` |
| 15 Foot probes under drawn body on step/slope; model-space normal; heading = aim matches | `pose_inputs.rs` tests | achievable as stated | pass — `view_body_foot_probes_plant_the_drawn_feet_on_a_step`, `…normal_is_the_world_normal_in_the_drawn_frame`, `…match_aim_plus_turn_when_heading_is_aim`, `model_frame_body_foot_probes_keep_the_transform_rotation` |
| 16 Host re-probe of client pawn uses presented heading + turn (R4) | `host_presentation.rs` / pose_inputs test | achievable as stated | pass — `host_seat_reprobe_plants_client_feet_under_the_presented_heading_plus_turn`, `presented_reprobe_plants_feet…` |
| 17 Grep gate: no `--yaw 180` in `docs/` or content `license.txt`; only exo_red's `model.gltf` changes among assets | source-scanning test + `git diff --stat 83802575a -- content/` at review | achievable as stated | pass — `no_docs_page_or_content_licence_keeps_a_yaw_180_recipe`; `git diff --stat 83802575a -- content/` changes only exo_red's `model.gltf` and four `license.txt` |
| 18 Manual: exo_red faces travel/aim, legs fold, shadow, ghost | owner, in-engine | manual | pass — owner, two-client run 2026-11-22: bodies face their view; legs behave as before |
| 19 Manual: feet plant strafing across a slope | owner, in-engine | manual | pass — owner: legs and feet behave as expected |
| 20 Manual: re-convert exo_red / mutant with defaults gives +Z; `--help` says so | owner, Blender | manual | outstanding — owner, Blender re-convert not yet run |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Contract, draw seams (mesh_render, ghost_render), R1, placement constant, exo_red Hips re-face; first-slice proof: all six exo_red bakes `--check` VERIFIED on the real exo_red, `hand_r` the exact 180° image of its old pose | integrating executor | — | done |
| 2 | Body-space seams: `hit_zones::model_matrix` + callers (zones, sockets, overlays, netcode `client_overlay_anchor`, `damaged_enemy_overlay`), foot probes at the drawn rotation; rows 12–16 | worker A | 1 | done: `hit_zones_body_frame_tests.rs`, `pose_inputs_body_frame_tests.rs` (f47d950ef); host-level R4: `host_seat_reprobe_plants_client_feet_under_the_presented_heading_plus_turn` |
| 3 | Body-frame materialization on every role and entry path, R3; draw/ghost/regression/serde rows 1–7; old-frame tests and comments | worker B | 1 | done (f47d950ef): `descriptor_body_frame` at spawn; `apply_player_viewer_role` on every client viewer-role call; respawn relocates in place; mesh hot-reload declined |
| 4 | Converter default +Z, help text, docs and `license.txt` recipes, E21 research note; grep gate row 17 | worker C | — | done (b1407e89d): default turn is `--yaw` alone; `no_docs_page_or_content_licence_keeps_a_yaw_180_recipe` |
| 5 | exo_red rows 8–11 | worker D | 1 | done (b1407e89d): worst joint gap 2.8e-7 rot, 0.71 mm pos |
| 6 | Integrate, gate, preflight, review loop | integrating executor | 2–5 | done: preflight green (fmt, clippy -D warnings with dev-tools, workspace tests, release check, crate graph, reducers); review panel round 1 (5 lenses) + round 2 (fix commit), findings fixed |
