# E21--player-model-front — plan of record

mode: compact
status: active
read at: 99cc0e0d5

## Corrections
- Brief read at 83802575a → no cited seam changed by 99cc0e0d5; Decision reads reused.
- `main` did not build the `postretro` test target (`area_lights_surface_tests.rs` lacked `frontend_backdrop` after two branches merged) → fixed in its own commit (76a734288) before this work.

## Delegated answers
None (brief has no open questions).

## Contract (integrating executor)
- `postretro_entities::components::mesh::{BodyFrame, MODEL_FRONT_TO_VIEW_FORWARD}`. `MeshComponent::body_frame` is `#[serde(skip)]`, default `Model`; player bodies are `View`. `BodyFrame::model_rotation(rotation)` composes the half turn. `MeshComponent::presentation_rotation(transform_rotation)` is the draw rotation: lower-body heading from pose inputs on an animated mesh, else the transform rotation, then the body-frame turn (pin R1: a stateless mesh's foot-probe default heading never replaces the transform yaw).
- `MapEntity::model_rotation_quat` uses the same constant (placement seam).

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| 1 Player body +Z along aim / heading, every role | mesh_render collector tests per role (SP, host local, host-side remote, client own, client remote) | achievable as stated |
| 2 Stateless legged body after pose pass (R1); first frame after spawn, respawn, seam, relevel, rejoin | collector test with pose pass run; materialization-path tests per entry | achievable as stated |
| 3 Ghost copy + weapon attachment of a player body | `ghost_render_tests.rs` | achievable as stated |
| 4 Regression guard: animated enemy, prop, ghost projectile unchanged; no double turn | collector + ghost tests | achievable as stated |
| 5 Map-placed copy of player descriptor unturned, same install as a pawn (R3) | sim descriptor-spawn test, host + client materialization | achievable as stated |
| 6 Transform/pose-input/replicated yaws unchanged; zero twist at heading = aim | pose_inputs + mesh_render tests | achievable as stated |
| 7 No body-frame key serialized; deserialize yields none | `mesh.rs` serde test | achievable as stated |
| 8 exo_red faces +Z in idle (left hand +X of right) | `gltf_loader.rs` facing test | achievable as stated |
| 9 exo_red joints through idle_aiming/run_forward match pre-change draw (rot within tolerance, pos ≤ 1 mm) | model test vs in-memory Ry(180°) Hips reference | achievable as stated |
| 10 Every committed exo_red bake `--check` VERIFIED; −Z exo_red fails | tool test over the committed bakes + scratch −Z copy | achievable as stated |
| 11 Pre-change reference built in memory / from 83802575a | same tests as 9–10 | achievable as stated |
| 12 Hit zone front ray hits, behind misses | `hit_zones.rs` test | achievable as stated |
| 13 Socket anchor `hand_r` lands at drawn socket (heading = aim) | `hit_zones.rs` posed_socket_world test | achievable as stated |
| 14 Heading ≠ aim: zones and anchors on transform + turn (R2) | `hit_zones.rs` test | achievable as stated |
| 15 Foot probes under drawn body on step/slope; model-space normal; heading = aim matches | `pose_inputs.rs` tests | achievable as stated |
| 16 Host re-probe of client pawn uses presented heading + turn (R4) | `host_presentation.rs` / pose_inputs test | achievable as stated |
| 17 Grep gate: no `--yaw 180` in `docs/` or content `license.txt`; only exo_red's `model.gltf` changes among assets | source-scanning test + `git diff --stat 83802575a -- content/` at review | achievable as stated |
| 18 Manual: exo_red faces travel/aim, legs fold, shadow, ghost | owner, in-engine | manual |
| 19 Manual: feet plant strafing across a slope | owner, in-engine | manual |
| 20 Manual: re-convert exo_red / mutant with defaults gives +Z; `--help` says so | owner, Blender | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Contract, draw seams (mesh_render, ghost_render), R1, placement constant, exo_red Hips re-face; first-slice proof: all six exo_red bakes `--check` VERIFIED on the real exo_red, `hand_r` the exact 180° image of its old pose | integrating executor | — | done |
| 2 | Body-space seams: `hit_zones::model_matrix` + callers (zones, sockets, overlays, netcode `client_overlay_anchor`, `damaged_enemy_overlay`), foot probes at the drawn rotation; rows 12–16 | worker A | 1 | |
| 3 | Body-frame materialization on every role and entry path, R3; draw/ghost/regression/serde rows 1–7; old-frame tests and comments | worker B | 1 | |
| 4 | Converter default +Z, help text, docs and `license.txt` recipes, E21 research note; grep gate row 17 | worker C | — | |
| 5 | exo_red rows 8–11 | worker D | 1 | |
| 6 | Integrate, gate, preflight, review loop | integrating executor | 2–5 | |
