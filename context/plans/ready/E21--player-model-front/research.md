# E21--player-model-front — research

Read at 83802575a. Findings that inform the brief but do not decide it.

## Model facing (Hips rest rotation)

| Model | Hips rest | Front | Converted with |
|---|---|---|---|
| `exo_red` | Ry(180°) | −Z | default |
| `clip_libraries/mixamo_rifle` | Ry(180°) | −Z | default, from `exo_red.fbx` |
| `limitator`, `rodin_sci-fi_trooper_m`, `cyberpunk_warrior_f` | ≈ identity | +Z | `--yaw 180` (`done/character-clip-library/research.md`) |
| `mutant`, `decraniated_low_poly_retro_pixel`, `mixamo_brute`, `mixamo_mutant` | ≈ identity | +Z | `--yaw 180` (`license.txt`) |

exo_red is the only player body (`content/dev/scripts/player.ts`). No NPC uses it. Other player descriptors declare no mesh.

## Converter

`tools/mixamo_to_gltf.py` always applies `flip_z = Rotation(π + yaw)` about Blender Z. Its comment calls the result +Z-forward, "confirmed empirically" by a body facing away from its movement. That observation matches the player path, which draws model −Z as front. Every committed conversion confirms that the default output faces −Z.

## Body-space seams for a player pawn

| Seam | Rotation today | Assumes a front? |
|---|---|---|
| Body draw, `mesh_render.rs` `presentation_body_rotation` | Ry(heading) or raw transform | Implicitly −Z |
| Ghost copy, `ghost_render.rs` `rebuild_group` | Same function | Implicitly −Z |
| Attachments, `attachments::emit_for_holder` | holder × socket | None; follows holder |
| Mirror, capture, shadow | Same instance transform | None |
| Hit zones, sockets, overlays, `hit_zones::model_matrix` | Raw transform (aim yaw) | Implicitly −Z |
| Foot probes, `pose_inputs.rs` | `model_matrix` | Implicitly −Z; ignores heading |
| Pose modifiers, `pose_modifier.rs` | Joint-local | None |
| Locomotion yaws, `player_locomotion` | Yaw only, 0 = −Z | None |
| Fire origin | Viewmodel `muzzleOffset`, camera space | None |

exo_red has no `hitZone` extras today. Player hits use the axis-aligned health box, so the hit-zone seam is latent for players. The revive marker names no socket.

## Retargeting

exo_red embeds no clips; its extras name `mixamo_rifle`. `clip_library.rs` `Binder::new` sets `rest_correction = mesh_rest × library_rest⁻¹`. `bind` premultiplies rotation keys and the root translation delta by it. With exo_red's Hips rest at identity, the correction becomes Ry(180°)⁻¹. An engine `socket_dump` measured every posed joint turned exactly 180° about Y in `idle_aiming` and `run_forward`, with joint-local frames unchanged.

The bound root translation is `mesh_rest + rest_correction × delta`, and `mesh_rest` is not rotated. exo_red's Hips rest sits at z ≈ 0.36 mm, so binding turns the body about that point, not the model origin. The re-faced draw sits about 0.7 mm forward of today's draw.

Animated bounds (`compute_conservative_animation_bounds`) are origin-centred cubes, so the re-face does not change culling.

## Mount check

`model::mount::verify_mount` reports `barrel_dot_forward = barrel · +Z`. `crates/tool/src/mount` reads it for `--check`. A correct bake on today's exo_red reads −1.

## Pin table

| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| R1 | Stateless legged player body, aim yaw θ ≠ 0 | The presentation pose pass (pose inputs, then foot probes) runs before the draw and writes default pose inputs | Model +Z draws along Ry(θ)·(−Z); the default heading never replaces the transform yaw |
| R2 | Player with heading φ ≠ aim θ; hit query and draw on the same tick | The pose pass writes heading φ before the hit query reads the mesh | Zones and sockets sit on Ry(θ) plus the turn; only the draw sits on Ry(φ) plus the turn |
| R3 | Level install places a player-class descriptor as a map entity and spawns a pawn of the same class | Both materialize through the shared descriptor spawn in one install | Only the pawn records the body frame; the placed copy faces its model +Z along its authored arrow |
| R4 | Host draws a client pawn from a delayed sample while the live heading has turned | Fixed-tick probe (live heading) → `present_client_pawns` (presented heading) → re-probe | Final probes use the presented heading plus the turn |
