# E21--character-weapon-mounts

Brief · compact · draft, validated (Direction sound), detail review rounds 1–2 and owner answers applied; owner API review pending · Epic 21 · reads: `context/lib/entity_model.md` §2 (Weapon vocabulary) · `context/lib/resource_management.md` §7 (Sockets, Socket-mounted weapons) · `context/lib/networking.md` §Weapon placement is content · `done/E21--coop-avatar-weapon-presentation/` · `done/E21--bone-sockets-attachments/` · read at 956a929b

## Problem
Owner-observed defect (2026-11-22): the player avatar exo_red holds its third-person weapons at the wrong angle and at a cartoonish size. The cause: a pawn's third-person weapon is the weapon descriptor's single `thirdPersonModel`, mounted at a hard-coded `hand_r` with no fit to the holder's rig. Every dev weapon points that field at its first-person viewmodel asset, which is camera-shaped and was never fitted to any hand (`research.md` §Assets). One weapon-owned model fits only the rigs whose hand frames match its bake. The shared `ar_4` already failed `--check` on both enemy rigs it was tried on (`done/character-clip-library/`). `drafts/E10--npc-equipment-sets/` needs one weapon seated on mixed rigs. A mod with several player characters (`player_spawn` `entity_class`) hits the same wall. When done, a character descriptor declares the mount fitted to its own rig for each weapon it may wield. A pawn mounts its active weapon from its own descriptor, and exo_red holds every dev weapon seated in its hand.

## Decisions
- **The mount is character-owned, inside `mesh`.** A `weaponMounts` table in the descriptor's `mesh` block, beside `mesh.attachments`, maps a weapon to its attachments (socket → prop model), each baked for this character's rig. It lives with the model it was fitted to, so art owns it and a model swap carries its mounts. Sockets are declared per entry, not hard-coded, so a weapon may also mount on more than one socket. The table lives on the descriptor, not on the pawn, with no player-vs-enemy branch (`entity_model.md` §2, one weapon vocabulary).
- **Per-character bakes become the mechanism.** The solver's default is one bake per weapon, with per-character bakes as the escape hatch (`done/weapon-mount-frame-solver/` Decision 3; `docs/weapon-mounts.md` §Pose and character limits). This brief reverses that default on the `ar_4` and E10 evidence above. Characters whose hands match still share one bake by naming the same file. The table maps any weapon to any model, so one character's version of a weapon may look different from another's. That is accepted as a consequence, not offered as a cosmetics feature (the roadmap's Epic 21 non-goal stands).
- **Weapons are referenced by identity.** Each entry names its weapon by the weapon archetype's `defineEntity` value, as `inventory.loadout` does. A list, not a map, because a handle cannot key an object.
- **Composition.** The mounted list is base `mesh.attachments`, then the active weapon's entry; later entries win per socket. When the weapon leaves, its sockets return to the base attachments. A switch replaces the old entry's sockets and the new entry's in one step.
- **`thirdPersonModel` is retired.** The character's table is the only third-person source. A weapon with no entry on its wielder's character mounts nothing. A fallback would hide exactly the unfitted mount this brief fixes. Pre-stable, break and fix consumers: the field and its call sites go in this brief (`context/lib/index.md`, Pre-stable note).
- **No table opts out.** A character without `mesh.weaponMounts` mounts no weapon and logs nothing; its weapon may be part of its model (a robot with a hedge-trimmer arm).
- **A declared table makes missing data loud.** For a character that declares the table:
  - Load warns once for each loadout weapon the table lacks.
  - Any other weapon it lacks (a pickup, a carried-over weapon) warns once at first wield, naming character and weapon.
  - A socket the rig does not declare warns once, naming the character, weapon and socket, and draws nothing there.
  - A mount model missing from the preload warns once, where today the socket is cleared silently.
- **Replication is unchanged.** Clients already receive the active weapon's identity (`active_weapon_archetype`) and the pawn's class (`pawn:<name>`). Each peer resolves the mount from its own descriptors, so there is no wire change. NPCs differ: their carried weapons stay host-only, so `drafts/E10--npc-equipment-sets/` sends the mounted list itself. Peers with differing descriptors present locally, as descriptors do elsewhere (`entity_model.md` §7d precedent).
- **Load and reload.**
  - Every model any descriptor's table lists joins the level-install model sweep, so a switch never waits on a load.
  - A table edit applies at the next level load, under the existing `mesh` visual-asset deferral and its warning.
  - The local pawn's mounts inherit its shadow-only visibility, as today.
- **Load rules (both runtimes, twin behavior).** Each of these rejects load, naming the character:
  - a duplicated weapon in the table;
  - an entry whose `weapon` is not a weapon archetype;
  - an empty attachment map;
  - a model path that breaks the existing content-relative path rule for weapon models.
- **Gameplay never reads a mount.** Fire origin stays first-person placement composed with `muzzleOffset` (`networking.md` §Weapon placement is content). Hit zones and collision ignore attachments (`resource_management.md` §7).
- **A bake lives beside its source glTF.** A derived glTF sits in its source's folder, named for what it was fitted to. A mount bake is `<weapon folder>/<character>.gltf`, sharing the folder's textures. The existing per-rig bakes (`ar_4_trooper`, `ar_4_warrior`) move into `models/ar_4/`, and their consumers update in this brief.
- **Dev content is the first consumer.** exo_red gets a bake fitted to its `hand_r` for every dev weapon a pawn can wield that has a third-person model today. Each bake carries a verified `extras.mount`, and the viewmodel assets are untouched. Scale and grip origin are proven manually; `--check` measures orientation only.
- **Non-goals.**
  - NPC consumption, including the rifle squads' move from `mesh.attachments`: `drafts/E10--npc-equipment-sets/`.
  - Off-hand IK onto a weapon grip socket. Two-handed clips place the off hand, and a weapon model may carry a `grip_l` socket later.
  - The third-person muzzle origin for observers, which stays deferred (`networking.md`).
  - Moving `.prm` texture sidecars beside their glTFs. They are a content-addressed cache under the baked root (`build_pipeline.md` §Baked texture mips), not derived glTFs.
  - The cyberpunk pack's redistribution terms for SDK bundles: the bakes are the same kind of converted copy already shipped, so the open question is unchanged (`research.md`).
- **Docs amended at promotion:**
  - `entity_model.md`: Weapon vocabulary, Components (`mesh.weaponMounts`, `thirdPersonModel` retired).
  - `resource_management.md` §7: per-character mounts, and derived glTFs living beside their source.
  - `networking.md` §Weapon placement is content: the third-person mount is resolved from the wielder's descriptor.
  - Human-facing: `docs/weapon-mounts.md` (including §Pose and character limits: per-character bakes are the mechanism), `docs/character-animation.md`, `docs/scripting-reference.md`; the SDK types in both runtimes.

### Scripting surface
Owner reviews this surface personally before promotion.
```ts
import { defineEntity } from "postretro";
import { referencePistolEntity as pistol } from "./reference-pistol";
import { referenceRifleEntity as rifle } from "./reference-rifle";

export const playerEntity = defineEntity({
  canonicalName: "player",
  components: {
    mesh: {
      model: "models/exo_red/model.gltf",
      shadowOnly: true,
      // Each bake fitted to exo_red's hand with `postretro-tool solve-weapon-mount`,
      // and kept beside its weapon, named for the character.
      weaponMounts: [
        { weapon: rifle, attachments: { hand_r: "models/cyberpunk_weapons/rifle/exo_red.gltf" } },
        { weapon: pistol, attachments: { hand_r: "models/cyberpunk_weapons/pistol/exo_red.gltf" } },
      ],
      // animations… as today
    },
    inventory: { loadout: [rifle, pistol] },
    // movement, health… as today
  },
});
// Weapons no longer carry `thirdPersonModel`; `viewmodel` stays.
```

### Boundary inventory
Both runtimes ship.

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| mount table | mesh descriptor field | `"weaponMounts"` inside `"mesh"`: list of `{weapon, attachments}` | `components.mesh.weaponMounts` | `components.mesh.weaponMounts` | n/a |
| entry weapon | canonical name | canonical name string | weapon `defineEntity` value | weapon `defineEntity` value | n/a |
| retired field | `WeaponDescriptor.third_person_model` removed | `"thirdPersonModel"` rejected as unknown | removed from `WeaponDescriptor` | removed | n/a |

## Acceptance
### Automated
**Resolution**
- [ ] A pawn whose descriptor has an entry for its active weapon mounts that entry's models at their declared sockets. The result is the same on the listen host for its own pawn and each client's slot pawn, in single player, and on a connected client for both its own and a remote pawn.
- [ ] An entry naming two sockets, one of them not `hand_r`, mounts each model at its declared socket on every path above.
- [ ] Two player descriptors on different rigs, wielding the same weapon, each mount their own bake. This holds on the host and on a connected client, for the client's own pawn and for the remote one.
- [ ] A weapon switch removes every socket the old entry mounted and mounts the new entry's, and a base attachment on a socket the old entry covered returns. Switching to a weapon with no entry mounts nothing and leaves the base attachments. This holds when the new entry names the same model as the old one on a shared socket (P4).
- [ ] A pawn with no active weapon, at install or after dropping it, shows every base attachment, including one on a socket a table entry covers (P3).
- [ ] A second weapon switch on the same tick leaves only the final weapon's entry mounted.
- [ ] A connected client's own pawn of a non-default player class mounts that class's entry on its arming snapshot (P1).
- [ ] A pawn whose class changes at a level transition while it carries its active weapon mounts the new class's entry, on the host and on a connected client. A new class without `mesh.weaponMounts` shows only its base attachments, with no warning (P5).
- [ ] No wire record gains a field. Each peer resolves the mount from `active_weapon_archetype` and the pawn's class alone.
- [ ] Fire origin and hit tests are identical with and without a `weaponMounts` table on the firing pawn's descriptor.

**Missing data**
- [ ] For a character that declares the table: a loadout weapon it lacks warns once at load, naming both, and a picked-up weapon it lacks warns once at first wield. A character without `mesh.weaponMounts` wields every weapon, mounts nothing and logs nothing.
- [ ] A loadout weapon the table lacks warns at load and does not warn again when first wielded.
- [ ] A missing-entry warning fires once per character and weapon per level load on each peer that resolves it. A respawn or a repeat switch in the same level does not repeat it; the next level load may warn again.
- [ ] A character without `mesh.weaponMounts` keeps every base attachment, including one on `hand_r`, with no active weapon and through every weapon switch.
- [ ] An entry naming a socket the rig lacks warns once, naming the character, weapon and socket; the generic holder-socket warning does not also fire for a mount. It draws nothing on that socket, and the entry's other sockets still mount.
- [ ] Every model a table lists is in the level-install sweep and the predicted-model set. A listed model that fails to load warns once, however often its weapon is switched to, and the entry's other sockets still mount.

**Load rules and retirement**
- [ ] Each load rule in Decisions rejects load in both runtimes.
- [ ] `thirdPersonModel` on a weapon is rejected as an unknown field in both runtimes, and no `content/` descriptor carries it.
- [ ] Load rules check the incoming descriptors, not the deferred table: a hot reload that renames a weapon and its entry commits. A loadout weapon added with its entry in the same reload mounts nothing until the next level load, and warns once.
- [ ] One `weaponMounts` list shared by two `defineEntity` calls, directly or through a spread `mesh`, loads in both runtimes and lowers to the same names for both.
- [ ] A table edit under hot reload keeps the live mounts and logs the visual-asset deferral warning. A weapon switch before the next level load mounts the pre-edit entry (P2). The next level load applies the edit.

**Content**
- [ ] Every model exo_red's `weaponMounts` lists carries `extras.mount`. For each, `postretro-tool solve-weapon-mount content/dev/models/exo_red/model.gltf --weapon <model> --mount-joint hand_r --check` reports `VERIFIED: mount check passed.` from the declared `extras.mount` axes, never the UNVERIFIED geometric assist. Each listed model sits in the folder of its entry weapon's `viewmodel`, named for exo_red. No listed model is any weapon's `viewmodel` path, and no viewmodel asset changes. This runs as an automated test over the table, not over a directory. So a re-export that drops the fit fails, and so does an entry repointed at a viewmodel.
- [ ] exo_red's table has an entry for each of: reference shotgun, pistol, rifle, plasma rifle, rocket launcher, `incendiary_rifle`, `wieldable_fixture_auto`, `wieldable_fixture_press`, `coop_health_weapon`, `coop_heat_test`. Loading the dev player logs no missing-entry warning.
- [ ] Each weapon folder's viewmodel glTF, its buffer and every texture it references are byte-identical to 956a929b; a bake adds only its own `.gltf` and `.bin`.
- [ ] The rifle squads' bakes live in `models/ar_4/`, and each squad still mounts its own rifle. No `models/ar_4_*` folder remains.
- [ ] Each moved ar_4 bake references only its own buffer, named after its glTF, and `solve-weapon-mount <its rig> --weapon <moved bake> --mount-joint hand_r --check` still reports `VERIFIED: mount check passed.`

**Surface**
- [ ] The Scripting surface example, as a fixture descriptor, loads in both runtimes and lowers to the same descriptor.
### Manual
- [ ] In a third-person or observer view on a dev map, exo_red holds each dev weapon in its right hand, at believable scale and gripped at the handle, with the barrel along the aim, through idle, walk, run and a weapon switch. This is the only proof of scale and grip.
- [ ] Two-client co-op at 150 ms RTT (`POSTRETRO_NET_LAG`): each client sees the other's weapon seated the same way, before and after a switch.
- [ ] The local player's body shadow shows the seated weapon.

## Path
- Seams:
  - The one writer: `update_active_weapon_attachment` (`crates/netcode/src/remote_materialize.rs`), with its hard-coded `ACTIVE_WEAPON_SOCKET`. It needs the wielder's class: `DescriptorProvenance` on the host, the `pawn:` class on clients.
  - Callers and preload: `research.md` §Today's third-person mount. Each caller rebinds via `resolve_mesh_entity_bindings_for_entities`. Preload merges in `lifecycle_world_cpu.rs` and `level_load/model_prediction.rs`.
  - Client class: `LocalWeaponAttachmentUpdate` carries no class, and the weapon update surfaces before arm records the class (pin P1). The writer's class lookup defaults a missing class to `player`, as body materialization does.
  - No active weapon: today's writer strips a base `hand_r` when nothing is mounted (pin P3).
  - Deferral: `defer_visual_asset_descriptor_refreshes` (`scripting-core/src/runtime/core.rs`) defers `mesh` whole, so a table inside it is covered; confirm with pin P2.
  - Load rejection: the SDK builders reject a non-weapon value, but the Rust runtime receives names, so rejecting by character at load needs new cross-descriptor validation (`research.md`).
  - Handle lowering precedent: how `inventory.loadout` lowers weapon `defineEntity` values to canonical names.
  - Check: the pure-Rust check in `crates/tool/src/mount/`.
- Shape chosen: a per-character table resolved against the active weapon at the existing single writer.
- Strongest rival: the table plus a `thirdPersonModel` fallback (solver Decision 3 taken literally). Rejected per Decision "`thirdPersonModel` is retired", unless the owner keeps it.
- Weaker rivals: baking `thirdPersonModel` for exo_red only fixes one rig. Keying mounts by rig fails because the engine has no rig identity; a shared script constant gives authors the same reuse.
- First slice: resolve one weapon (the rifle) from exo_red's table at the single writer, in single player, with one bake. That falsifies the riskiest assumptions: that a solved bake seats correctly through the rifle clip library's poses, and that the writer can reach the wielder's descriptor on every path.
- The exported glTF stands in as the raw source, since the `.blend` is not in the repo; `tools/prop_to_gltf.py` imports glTF.
- Bakes need Blender, which this container lacks. The executor installs it, or the owner runs the commands `solve-weapon-mount` prints. `docs/weapon-mounts.md` has the workflow, and `done/character-clip-library/plan.md` records the ar_4 bakes as precedent.
- Files past ~800 lines this extends: `data_archetype.rs`, `netcode/src/lib.rs`, `postretro/src/main.rs`, `netcode/src/client.rs`. Split a cohesive block out first where this adds one, in its own behavior-preserving commit.
- Derivation: `research.md`.

## Open questions
- Grip point and scale per bake. — **delegated**
