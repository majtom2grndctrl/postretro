# E21--player-model-front

Brief · compact · ready (owner signed off 2026-11-22) · Epic 21 · reads: `context/lib/entity_model.md` §4 (Facing convention) · `context/lib/resource_management.md` §7 · `ready/E21--character-weapon-mounts/` · read at 83802575a

## Problem
Owner-observed defect (2026-11-22, while baking exo_red's E21 weapon mounts): exo_red, the player reference body, faces model −Z, while every NPC body faces +Z. Cause: a player body is drawn from its view-frame rotation (camera or lower-body heading yaw, forward −Z) with no model-front half turn, so the player path treats model −Z as front. The Mixamo converter's built-in flip was calibrated against that path. Its default output faces −Z, and every NPC conversion needed `--yaw 180` to face +Z. So one model cannot serve as both player and NPC, a standard conversion faces backward as an NPC, and `solve-weapon-mount --check`, which assumes the holder's front is +Z, rejects correct exo_red bakes. A player's foot ground probes, which must take the turn anyway, also place the body by its aim rather than where it is drawn. Whenever the legs fold away from the aim, the feet are probed off the drawn body and miss the ground on slopes. When done, every model placed in the world fronts glTF +Z, a player body included, on every role and at every seam that places the body in the world. Foot probes run under the drawn body. exo_red faces +Z and looks unchanged in play. The converter's default output faces +Z.

## Decisions
- **glTF +Z is the front of every model placed in the world:** enemies, props, pickups, and player bodies. `entity_model.md` §4 already states it; the player body path is the divergence. No player-vs-NPC model frame. First-person viewmodels draw in camera space, and raw weapon sources keep their authored axes; neither is in scope.
- **A player's transform stays in the view frame.** Its rotation and its aim and heading yaws keep forward −Z, and the wire is unchanged. The half turn applies only where a player's view-frame rotation becomes a model→world matrix: a body seam, parallel to the placement seam `MapEntity::model_rotation_quat`. Spawning players from the model frame would move camera, movement and netcode. §4's "exactly one seam" becomes two at promotion: placement for entities that face their model front, body for entities that stay in the view frame. A new spawn path whose entity stays in the view frame and draws a model takes the body seam.
- **Every seam that places a player body takes the turn.** These are the body draw (shadow-only, mirror and capture included), ghost copies, attachments (they follow the holder), hit zones and socket anchors, overlay anchors and foot ground probes. It holds on every role: single player, host local, host-side remote, client own pawn and client remote avatar. It holds from the first drawn frame after a spawn or respawn, and for a body with no animation state.
- **Each peer derives the body frame when it materializes the body, as it does `shadow_only`.** The frame never crosses the wire or enters a save. It follows the repo's derive-per-peer choice for presentation (`networking.md` §Weapon placement is content). No path rebuilds a player's mesh after spawn. Level seams, relevels and rejoins materialize bodies afresh through the same steps.
- **Foot ground probes place the model at the drawn body rotation**, lower-body heading plus the turn: probes plant the drawn feet (owner, 2026-11-22).
- **exo_red is re-faced by setting its `mixamorig:Hips` rest rotation to identity**, the copy E21's committed bakes were verified against (`ready/E21--character-weapon-mounts/research.md` §exo_red bakes). Joint rotations are unchanged in play and the bakes stay valid (`research.md` §Retargeting). Positions move under 1 mm, because binding turns the body about the Hips rest point, not the model origin; that counts as unchanged (owner, 2026-11-22). Library composition carries the root-only yaw difference from `mixamo_rifle`'s rest as a whole-body turn, root motion included. At promotion `resource_management.md` §7 gains a sentence allowing it.
- **The converter's default output faces +Z** for a standard Mixamo export. `--yaw` stays as an extra turn about the same axis. Recipes and docs that pass `--yaw 180` to reach +Z drop it (owner, 2026-11-22). A recipe kept outside the repo now yields −Z; the help text says the default changed. Committed assets are not re-converted.
- **Sequencing.** Lands before `ready/E21--character-weapon-mounts/` is built. E21's Content row needs `--check` VERIFIED on exo_red.
- **Non-goals.**
  - The `mixamo_rifle` library stays −Z: nothing draws it, and binding corrects for its rest.
  - Hit zones and socket anchors following the lower-body heading. They stay on the authoritative transform plus the turn: gameplay never reads presentation heading (owner, 2026-11-22).
  - E21's mounts themselves: `ready/E21--character-weapon-mounts/` owns them.

## Acceptance
### Automated
**Player body front, every role**
- [ ] A player body with aim yaw θ and no lower-body override draws with model +Z along Ry(θ)·(−Z). The same holds with a lower-body heading φ ≠ θ, along Ry(φ)·(−Z). Checked for each role: single player, host local, host-side remote, client own pawn, client remote avatar.
- [ ] The same holds for a player body with no animation state on a legged model, drawn after the presentation pose pass has run (foot probes included), with aim yaw θ ≠ 0 (pin R1). It holds on the first drawn frame after a spawn, a respawn, a level seam arrival, a host relevel, and a client rejoin.
- [ ] A ghost copy of a player body, and its weapon attachment, draw at the ghost-mapped transform of the real body's turned draw.
- [ ] Regression guard: a placed animated enemy carrying pose inputs, a placed prop, and a ghost copy of a model-bodied projectile draw with the same rotation as before. The turn is never applied twice.
- [ ] A map-placed copy of a player descriptor draws as a placed model, unturned, on the host and on a client. This holds in the same level install that spawns a pawn of that class, which takes the turn (pin R3).
- [ ] A player's transform yaw, pose-input aim and heading yaws, and replicated aim yaw are unchanged by this work: forward −Z. With heading = aim, the upper-body twist the modifier applies is zero.
- [ ] A player body's serialized mesh component carries no body-frame key, and deserializing one yields no body frame.

**exo_red**
- [ ] exo_red, bound to `mixamo_rifle` and posed in `idle`, faces model +Z by the check the mutant and decraniated bodies pass: left hand on the +X side of the right.
- [ ] Posed through `idle_aiming` and `run_forward` and drawn by the new path, every exo_red joint's world rotation matches the pre-change draw within float tolerance, and its world position within 1 mm.
- [ ] For each committed exo_red bake, `postretro-tool solve-weapon-mount content/dev/models/exo_red/model.gltf --weapon <bake> --mount-joint hand_r --check` reports `VERIFIED: mount check passed.` The same check on a −Z exo_red fails, so the check discriminates.
- [ ] The pre-change reference for both rows above is exo_red with its `mixamorig:Hips` rest set back to Ry(180°), bound to `mixamo_rifle` and drawn by the old path. It is built in memory, or taken from the asset at 83802575a and written to a scratch path for the `--check` failure case.

**Body-space seams**
- [ ] A player body with a precise hit zone on a front-side joint: a ray crossing in front of the pawn, perpendicular to its aim, hits that zone. The same ray crossing behind the pawn misses it.
- [ ] With heading = aim, a posed socket anchor off the body's vertical axis (`hand_r`) on a player body lands at the drawn socket's world position at the current-tick transform.
- [ ] With lower-body heading φ ≠ aim θ, a player's precise hit zone and posed socket anchor are placed by the authoritative transform plus the turn (aim θ), not by heading φ (pin R2).
- [ ] Foot probes run on ground where the drawn body's unmodified foot joints and the aim-only placement's would land at different heights: a step or slope under one foot. With heading ≠ aim, each foot's contact height is the ground under the drawn body's foot, read at the current-tick transform. On a slope whose normal is not vertical, the probe's model-space normal is the world normal expressed in the drawn body's frame. With heading = aim, the same fixture's probes match the authoritative-transform placement plus the turn.
- [ ] On a listen host, a connected client's pawn re-probed after its delayed presentation places its feet under the presented heading plus the turn, not the tick's live heading (pin R4).

**Converter**
- [ ] Grep gate: no `--yaw 180` recipe remains in `docs/` or a content `license.txt`, and no model asset other than exo_red's `model.gltf` changes.

### Manual
- [ ] In a co-op observer or third-person view, exo_red faces its travel and aim through walk, backpedal, strafe and a weapon switch, with legs folding as before. The local shadow-only body's shadow faces the view direction. A teleporter ghost copy faces the same way.
- [ ] Strafing across a slope, exo_red's feet plant on the ground.
- [ ] Re-converting exo_red's FBX with converter defaults gives a +Z model. Re-running a former `--yaw 180` recipe (mutant) without the flag gives +Z too. The converter's `--help` says the default output now faces +Z.

## Path
- **Seams.** `presentation_body_rotation` (`mesh_render.rs`; `ghost_render.rs` `rebuild_group`). `hit_zones::model_matrix` and its callers: `nearest_zone_hit`, `damageable_volume`, `posed_socket_world`, netcode `client_overlay_anchor`, `damaged_enemy_overlay`. Foot probes in sim `pose_inputs.rs`: `write_foot_ground_probes`, `update_foot_ground_probes_for` (called from `host_presentation.rs`), `probe_foot`, `foot_probe_inverse`.
- **Trap.** `pose_inputs` is not player-only. `update_pose_inputs` fills it for every animated mesh, and an enemy's `heading_yaw` is its transform yaw, half turn included. Keying the turn on `pose_inputs` flips enemies. Shifting `heading_yaw` by π breaks the aim−heading twist. A fresh `MeshComponent` has no `pose_inputs` until the pose pass runs. A stateless body on a legged model then gains default pose inputs, heading 0, from `write_foot_ground_probes`, so today it draws at yaw 0 whatever its aim; enemies too.
- **Shape.** The body records its model frame beside `MeshComponent::origin_offset`, which every model-matrix seam already reads, as a non-serialized field. Set it beside each viewer-role step. On the host that is descriptor spawn with `PlayerSpawn` (level install) and `NetworkSlot` (`host_handle_accept_descriptor_at_placement`, beside `apply_remote_player_viewer_role`). On the client it is `materialize_armed_local_pawn` and `materialize_armed_remote_player` (`remote_materialize.rs`). Clients carry no `DescriptorProvenance`; the `pawn:` class tag and the local-player flag mark a player body there. `descriptor_mesh_component` alone cannot tell a pawn from a world copy of a movement descriptor. `materialize_net_mesh_presentation` returns early when a mesh exists, so the frame must not depend on a rebuild.
- **Rival.** Leave the engine alone and let `--check` take a declared holder front. It is the smaller change, and it would carry E21. The +Z converter default defeats it: a standard conversion must face the same way as a player and as an NPC.
- **Pose modifiers need no change.** The aim-pitch bend and the upper/lower split act in joint-local frames, which the re-face leaves alone. `stable_straight_chain_bend_axis` bends a straight knee toward model +Z, so exo_red's straight-leg fallback now bends forward. That is a fix.
- **Tests that pin the old frame.** `collector_carries_only_entity_pose_inputs` (`mesh_render.rs`). `real_holder_transform` and its player-body users (`ghost_render_tests.rs`); its projectile-ghost user must stay unturned. `map_yaw_faces_placed_model_fronts_and_player_view_along_the_arrow` keeps its −Z view assertion; the body test is new. Facing precedent: `bound_mutant_bodies_stand_on_their_feet_and_face_plus_z` (`gltf_loader.rs`), whose `walk`/`run` requirement `mixamo_rifle` does not meet.
- **Comments and docs that state the old frame or the old default.**
  - `descriptor_spawn_rotation`'s doc, the `mesh_pass.rs` header, and `hit_zones::model_matrix`'s doc. Its claim to match the render collector is already untrue for players.
  - The converter's `--yaw` help and flip comment, and `docs/character-animation.md`: its wrong-way advice, and its advice to build a library with its characters' `--yaw` so root motion follows them. Binding already turns root motion by the rest difference.
  - The `--yaw 180` recipes in the `mutant`, `decraniated_low_poly_retro_pixel`, `mixamo_brute` and `mixamo_mutant` `license.txt` files.
  - E21 `research.md` §exo_red bakes. Its −Z bullet goes stale at landing.
- **First slice.** The seam, the single-player draw, and exo_red's re-face. Check exo_red looks unchanged and the rifle bake passes `--check`. This falsifies the riskiest assumption: that re-face plus turn is a visual no-op through the clip library.
- **Large files.** `mesh_render.rs`, `hit_zones.rs` and `remote_materialize.rs` pass 800 lines. The edits there compose one rotation each and add no new responsibility, so no split is owed.

## Open questions
None.
