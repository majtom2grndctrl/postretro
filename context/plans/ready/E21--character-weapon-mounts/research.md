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
- `incendiary_rifle` (`status-hazards.ts`) spreads the reference rifle's components, so it carries the rifle's `thirdPersonModel` without naming it. It sits in the dev player loadout and needs its own exo_red entry, which may name the rifle's bake.
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

## Review findings (round 1)
- Loadout precedent splits. Both SDK builders reject a non-weapon reference by value, with an error naming the field path rather than the character (`sdk/lib/data_script.ts`, `.luau`). The Rust runtime receives a name and only warns at spawn for an unregistered or non-weapon loadout name (`wieldable_inventory.rs`). Rejecting a `weaponMounts` name at load, naming the character, needs new commit-time cross-descriptor validation.
- A socket the holder model lacks already warns once in the binding resolver (`mesh_bindings.rs`), keyed by holder model, socket and attachment model. It names neither character nor weapon, and the resolver knows neither.
- `--check` measures orientation only (`MountVerification`: barrel and up alignment). Scale and grip origin have no automated metric.
- With no declared axes, `--check` prints `UNVERIFIED: mount check passed.` and exits 0 (`crates/tool/src/mount/mod.rs`).

## Pin table
| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | A client's arming snapshot carries a non-default `pawn:` class and an active weapon | The record surfaces the weapon update before arm records the class; the writer runs after arm, in the same apply | The local pawn mounts that class's entry on the arming snapshot. A record with no class resolves as `player`, as body materialization does. |
| P2 | Hot reload stages a `weaponMounts` edit, then the pawn switches weapons in the same level | Edit deferred → switch → next level load | The switch mounts the pre-edit entry. The edited entry first mounts after the next level load. |
| P3 | A pawn with a base attachment on a socket some entry covers has no active weapon: at install, or after dropping its active weapon | The writer runs with no active archetype | The base attachment is present. |
| P4 | A switch from entry {hand_r: M, back: S} to entry {hand_r: M} | Same model on the shared socket; the old entry has an extra socket | `hand_r` keeps M; `back` returns to its base attachment, or empties. |
| P5 | A level transition where `player_spawn` names a different player class and the pawn carries its active weapon | New class installed → sync → first frame | The pawn mounts the new class's entry for that weapon, or nothing plus its missing-entry warning. A class without `mesh.weaponMounts` shows only its base attachments, with no warning. |

## Review findings (round 2)
- `MeshDescriptor` is not a serde type. Both bridges read `mesh` key by key and silently ignore unknown keys (`scripting-core/src/data_descriptors/js/entity.rs`, `lua/entity.rs`), so a misspelled `weaponMounts` opts out silently.
- The mesh deferral compares the whole `mesh` value (`runtime/core.rs`), so a nested table is covered.
- The `ar_4_trooper` and `ar_4_warrior` glTFs both reference `model.bin`, and `ar_4/model.bin` differs from both. Moving them into `ar_4/` means renaming their buffers. Their textures are byte-identical to `ar_4`'s.
- `prop_to_gltf.py` exports glTF Separate into the output folder, images included, so a bake can rewrite a viewmodel's textures in a shared folder.
- `ar_4/model.gltf` is itself the limitator's fit (`docs/weapon-mounts.md`), and its raw source is outside the repo.
- The loadout lowering rewrites the authored array in place (`sdk/lib/data_script.ts`, `.luau`), so a shared or spread table seen by a second `defineEntity` holds names, not handles.
- Load rules run on the incoming manifest, apart from the deferred active set (`runtime/core.rs` commit).
- The writer is stateless and already holds `hit_zone_store`, whose warn-once set clears per level load (`hit_zones.rs`).

## Bake workflow
- The exported glTF stands in as the raw source, since the `.blend` is not in the repo; `tools/prop_to_gltf.py` imports glTF.
- Bakes need Blender, which the cloud container lacks. The executor installs it, or the owner runs the commands `solve-weapon-mount` prints. `docs/weapon-mounts.md` has the workflow; `done/character-clip-library/plan.md` records the ar_4 bakes as precedent.
- Write each bake's buffer as `<rig>.bin`. Check that the export leaves the viewmodel's textures byte-identical.

## Weaker rivals
- Baking `thirdPersonModel` for exo_red only: fixes one rig.
- Keying mounts by rig: the engine has no rig identity. A shared script constant gives authors the same reuse.
- Narrowing an entry to `{weapon, socket, model}`: drops multi-socket mounts and breaks parity with `mesh.attachments`.

## Owner answers (2026-11-22)
- Pre-stable rule: move fast, break things, fix consumers.
- `weaponMounts` inside `mesh`; `thirdPersonModel` retired; a character without the table opts out silently.
- Bakes live beside their source glTF, named for the character model's folder. `ar_4/model.gltf` becomes `limitator.gltf`.
- Unknown `mesh` keys reject load. Weapon references are checked by the SDK, with Rust following the loadout precedent.
- Scale and grip stay manual-only.
