# E21--character-weapon-mounts — research

Read at 956a929b.

## Today's third-person mount
- One writer: `update_active_weapon_attachment` (`crates/netcode/src/remote_materialize.rs`). It removes or replaces the attachment on `ACTIVE_WEAPON_SOCKET` (`"hand_r"`) and leaves other sockets alone.
- Its callers:
  - Host: `update_repointed_weapon_attachments` (`main.rs`) marks the pawn dirty, then `synchronize_weapon_owner_attachments` (`netcode/src/lib.rs`) applies it for the host and slot pawns.
  - Single player: `synchronize_weapon_attachment_for_pawn` directly; install calls it for the boot pawn.
  - Client own pawn: `local_weapon_attachments`. Remote pawns: `materialize_remote_entities`, gated on `weapon_attachment_changed`.
- Resolution: the replicated `active_weapon_archetype` canonical name, looked up in each peer's own descriptors, gives `weapon.third_person_model`. The socket is cleared when the field is absent or empty, the archetype is unknown, or the model is not in `hit_zone_store`. There is no fallback.
- Preload: `weapon_presentation_models` collects the third-person and viewmodel paths of every weapon descriptor. `movement_descriptor_mesh_models` preloads player models. There is no runtime loading.
- Local pawn: `PlayerViewerRole::Local` keeps `shadowOnly`, and attachments inherit holder visibility (`attachments_inherit_shadow_only_holder_visibility`).
- Clients know a remote pawn's descriptor by its `pawn:<canonical_name>` class (`descriptor_class.rs`). Mods may ship several player descriptors through `player_spawn` `entity_class`.
- Hot reload: `defer_visual_asset_descriptor_refreshes` keeps old `thirdPersonModel`, `viewmodel` and `mesh` until the next level load, with a warning.
- No gameplay reads the third-person model. Fire origin is `muzzle_world_origin` / `resolve_muzzle_launch_origin` through the first-person placement. The observer's third-person muzzle is deferred.
- `third_person_model` is not in any wire payload. Its uses outside the presentation path are test fixtures.

## Assets
- exo_red: about 1.79 m tall. `hand_r` is on `mixamorig:RightHand` and `hand_l` on `mixamorig:LeftHand`, so the sockets are correct. It binds the `mixamo_rifle` clip library.
- Every dev `thirdPersonModel` equals its `viewmodel`:
  - pistol, rifle, shotgun;
  - the plasma rifle → `cyberpunk_weapons/rpg`, and the rocket launcher → `sci_fi_weapon` (the folder names look swapped);
  - the co-op and wieldable fixtures → `smg` and `shotgun`.
- None carries `extras.mount`; each has only a rigid `muzzle` socket. The rifle's bounding box spans about 1.29 m on Z, with the barrel on −Z.
- Raw sources: none are in the repo. `tools/extract_cyberpunk_weapons.py` exported the pack from a `.blend` outside it. `tools/prop_to_gltf.py` accepts glTF input, so the exported model can serve as the raw source.
- Bake precedent: `done/character-clip-library/plan.md` records `ar_4_trooper` and `ar_4_warrior` (barrel `1 0 0`, up `0 1 0`, grip `0.075 -0.05 0`, scale 0.34). The shared `ar_4` failed `--check` on both rigs, which is why each got its own bake.
- Workflow (`docs/weapon-mounts.md`):
  1. A Blender grip-and-scale bake.
  2. `solve-weapon-mount` with the barrel and up axes in the raw frame.
  3. The printed Blender command applies the Euler and writes `extras.mount`.
  4. `--check` verifies the result.
- The solve and the check are pure Rust; the bakes need Blender, which this container lacks.
- No CI or xtask gate checks mount bakes today. The unit tests in `crates/tool/src/mount/mod.rs` use synthetic fixtures.
- Licence: the cyberpunk pack (itch.io) allows commercial use but forbids redistributing the raw assets. Bakes are converted copies, like the ones already shipped. The SDK-bundle question is unchanged by this brief.
- No weapon model carries `grip_l` or another off-hand socket.

## Prior commitments
- `entity_model.md` §2: one weapon vocabulary; a presentation rule never branches on actor kind.
- `done/E21--coop-avatar-weapon-presentation/`: hand IK is a non-goal; the socket poses the weapon.
- `done/E21--bone-sockets-attachments/`: attachments are presentation only and resolve at spawn; runtime attach landed with replicated weapon identity.
- `networking.md` §Weapon placement is content: the third-person mount carries no placement offset, and art owns the fit.
