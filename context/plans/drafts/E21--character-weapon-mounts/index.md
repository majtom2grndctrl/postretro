# E21--character-weapon-mounts

Brief · compact · draft, not yet validated · Epic 21 · reads: `context/lib/entity_model.md` §2 (Weapon vocabulary) · `context/lib/resource_management.md` §7 (Sockets, Socket-mounted weapons) · `context/lib/networking.md` §Weapon placement is content · `done/E21--coop-avatar-weapon-presentation/` · `done/E21--bone-sockets-attachments/` · read at 956a929b

## Problem
Owner-observed defect (2026-11-22): the player avatar exo_red holds its third-person weapons at the wrong angle and at a cartoonish size. The cause: a pawn's third-person weapon is the weapon descriptor's single `thirdPersonModel`, mounted at a hard-coded `hand_r` with no fit to the holder's rig. Every dev weapon points that field at its first-person viewmodel asset, which is about 1.3 m long, has its barrel on −Z, is not origined at the grip, and was never fitted to any hand. A rigid mount is baked per rig (`resource_management.md` §7), so one model per weapon can fit at most one character. A mod that ships several player characters (`player_spawn` `entity_class`) cannot make one weapon look right on all of them. When done, a character descriptor declares the mount fitted to its own rig for each weapon it may wield. A pawn mounts its active weapon from its own descriptor, and exo_red holds every dev weapon seated in its hand.

## Decisions
- **The mount is character-owned.** A `weaponMounts` table on the entity descriptor, beside `mesh`, maps a weapon to its attachments (socket → prop model), each baked for this character's rig. Sockets are declared per entry, not hard-coded, so a weapon may also mount on more than one socket. The table lives on the descriptor, not on the pawn, with no player-vs-enemy branch (`entity_model.md` §2, one weapon vocabulary). This brief's consumer is the player pawn; `drafts/E10--npc-equipment-sets/` consumes it for NPCs.
- **Weapons are referenced by identity.** Each entry names its weapon by the weapon archetype's `defineEntity` value, as `inventory.loadout` does. A list, not a map, because a handle cannot key an object.
- **Composition.** The mounted list is base `mesh.attachments`, then the active weapon's entry; later entries win per socket. When the weapon leaves, its sockets return to the base attachments. A switch replaces the old entry's sockets and the new entry's in one step.
- **`thirdPersonModel` is retired.** The character's table is the only third-person source. A weapon with no entry on its wielder's character mounts nothing. A fallback would hide exactly the unfitted mount this brief fixes. Pre-stable, the field and its call sites go in this brief (`index.md`). — owner confirms (Open questions).
- **Missing data is loud.** Load warns once per character for each loadout weapon its table lacks. A socket the rig does not declare warns once, naming the character, weapon and socket, and draws nothing. A mount model missing from the preload warns once, where today the socket is cleared silently.
- **Replication is unchanged.** Clients already receive the active weapon's identity (`active_weapon_archetype`) and the pawn's class (`pawn:<name>`). Each peer resolves the mount from its own descriptors, so there is no wire change. Peers with differing descriptors present locally, as descriptors do elsewhere (`entity_model.md` §7d precedent).
- **Load and reload.**
  - Every model any descriptor's table lists joins the level-install model sweep, so a switch never waits on a load.
  - A table edit applies at the next level load, under the existing visual-asset deferral and its warning, as `mesh` and `viewmodel` edits do.
  - The local pawn's mounts inherit its shadow-only visibility, as today.
- **Load rules (both runtimes, twin behavior).** Each of these rejects load, naming the character:
  - a duplicated weapon in the table;
  - an entry whose `weapon` is not a weapon archetype;
  - an empty attachment map;
  - a model path that breaks the existing content-relative path rule for weapon models.
- **Gameplay never reads a mount.** Fire origin stays first-person placement composed with `muzzleOffset` (`networking.md` §Weapon placement is content). Hit zones and collision ignore attachments (`resource_management.md` §7).
- **Dev content is the first consumer.** exo_red gets a bake fitted to its `hand_r` for every dev weapon a pawn can wield that has a third-person model today. Each bake is a separate asset beside its weapon, carrying a verified `extras.mount`. The viewmodel assets are untouched. The exported glTF stands in as the raw source, since the `.blend` is not in the repo.
- **Non-goals.**
  - NPC consumption, including the rifle squads' move from `mesh.attachments`: `drafts/E10--npc-equipment-sets/`.
  - Off-hand IK onto a weapon grip socket. Two-handed clips place the off hand, and a weapon model may carry a `grip_l` socket later.
  - The third-person muzzle origin for observers, which stays deferred (`networking.md`).
  - The cyberpunk pack's redistribution terms for SDK bundles: the bakes are the same kind of converted copy already shipped, so the open question is unchanged (`research.md`).
- **Docs amended at promotion:**
  - `entity_model.md`: Weapon vocabulary, Components (`weaponMounts`, `thirdPersonModel` retired).
  - `resource_management.md` §7: per-character mounts.
  - `networking.md` §Weapon placement is content: the third-person mount is resolved from the wielder's descriptor.
  - Human-facing: `docs/weapon-mounts.md`, `docs/character-animation.md`, `docs/scripting-reference.md`; the SDK types in both runtimes.

### Scripting surface
Owner reviews this surface personally before promotion.
```ts
import { defineEntity } from "postretro";
import { referencePistolEntity as pistol } from "./reference-pistol";
import { referenceRifleEntity as rifle } from "./reference-rifle";

export const playerEntity = defineEntity({
  canonicalName: "player",
  components: {
    mesh: { model: "models/exo_red/model.gltf", shadowOnly: true /* animations… */ },
    // Each bake fitted to exo_red's hand with `postretro-tool solve-weapon-mount`.
    weaponMounts: [
      { weapon: rifle, attachments: { hand_r: "models/cyberpunk_weapons/rifle/exo_red/model.gltf" } },
      { weapon: pistol, attachments: { hand_r: "models/cyberpunk_weapons/pistol/exo_red/model.gltf" } },
    ],
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
| mount table | entity descriptor field | `"weaponMounts"`: list of `{weapon, attachments}` | `components.weaponMounts` | `components.weaponMounts` | n/a |
| entry weapon | canonical name | canonical name string | weapon `defineEntity` value | weapon `defineEntity` value | n/a |
| retired field | `WeaponDescriptor.third_person_model` removed | `"thirdPersonModel"` rejected as unknown | removed from `WeaponDescriptor` | removed | n/a |

## Acceptance
### Automated
**Resolution**
- [ ] A pawn whose descriptor has an entry for its active weapon mounts that entry's models at their declared sockets. On host, single player, and a connected client for both its own and a remote pawn, the result is the same.
- [ ] Two player descriptors on different rigs, wielding the same weapon, each mount their own bake.
- [ ] A weapon switch removes every socket the old entry mounted and mounts the new entry's, and a base attachment on a socket the old entry covered returns. Switching to a weapon with no entry mounts nothing and leaves the base attachments.
- [ ] A second weapon switch on the same tick leaves only the final weapon's entry mounted.

**Missing data**
- [ ] A loadout weapon missing from its character's table warns once at load, naming both.
- [ ] A socket the rig lacks warns once and draws nothing.
- [ ] A mount model missing from the sweep warns once. Every model a table lists is in the install sweep.

**Load rules and retirement**
- [ ] Each load rule in Decisions rejects load in both runtimes.
- [ ] `thirdPersonModel` on a weapon is rejected as an unknown field in both runtimes, and no `content/` descriptor carries it.
- [ ] A table edit under hot reload keeps the live mounts and logs the visual-asset deferral warning. The next level load applies it.

**Content**
- [ ] Every exo_red bake carries `extras.mount`, and `postretro-tool solve-weapon-mount content/dev/models/exo_red/model.gltf --weapon <bake> --mount-joint hand_r --check` reports VERIFIED for each. This runs as an automated test, so a re-export that drops the fit fails.
- [ ] Every dev weapon that had a third-person model now has an exo_red entry.

**Surface**
- [ ] The Scripting surface example, as the dev player descriptor, loads in both runtimes and lowers to the same descriptor.
### Manual
- [ ] In a third-person or observer view on a dev map, exo_red holds each dev weapon in its right hand, at believable scale, with the barrel along the aim, through idle, walk, run and a weapon switch.
- [ ] Two-client co-op at 150 ms RTT (`POSTRETRO_NET_LAG`): each client sees the other's weapon seated the same way, before and after a switch.
- [ ] The local player's body shadow shows the seated weapon.

## Path
- Seams:
  - The one writer: `update_active_weapon_attachment` (`crates/netcode/src/remote_materialize.rs`), with its hard-coded `ACTIVE_WEAPON_SOCKET`. It needs the wielder's class: `DescriptorProvenance` on the host, the `pawn:` class on clients.
  - Callers: `update_repointed_weapon_attachments` and `synchronize_weapon_owner_attachments` (host and single player); the client apply loop over `local_weapon_attachments`; `materialize_remote_entities` (remote pawns). Each rebinds via `resolve_mesh_entity_bindings_for_entities`.
  - Preload: `weapon_presentation_models` and `movement_descriptor_mesh_models` (`data_archetype.rs`), merged in `lifecycle_world_cpu.rs` and `level_load/model_prediction.rs`.
  - Deferral: `defer_visual_asset_descriptor_refreshes` (`scripting-core/src/runtime/core.rs`).
  - Handle lowering precedent: how `inventory.loadout` lowers weapon `defineEntity` values to canonical names.
  - Check: the pure-Rust check in `crates/tool/src/mount/`.
- Shape chosen: a per-character table resolved against the active weapon at the existing single writer.
- Strongest rival: keep `thirdPersonModel` and bake it for exo_red only. Rejected: it fixes one rig, and a second player character or an NPC wielding the weapon breaks again.
- First slice: resolve one weapon (the rifle) from exo_red's table at the single writer, in single player, with one bake. That falsifies the riskiest assumptions: that a solved bake seats correctly through the rifle clip library's poses, and that the writer can reach the wielder's descriptor on every path.
- Bakes need Blender, which this container lacks. The executor installs it, or the owner runs the commands `solve-weapon-mount` prints. `docs/weapon-mounts.md` has the workflow, and `done/character-clip-library/plan.md` records the ar_4 bakes as precedent.
- Files past ~800 lines this extends: `data_archetype.rs`, `netcode/src/lib.rs`, `postretro/src/main.rs`, `snapshot_apply.rs`. Split a cohesive block out first where this adds one, in its own behavior-preserving commit.
- Derivation: `research.md`.

## Open questions
- Retire `thirdPersonModel` (recommended: one source of truth, and a misfit fallback hides missing bakes), or keep it as the fallback for a character with no entry. — owner — **blocks build**
- Grip point and scale per bake, and the bake directory layout. — **delegated**
