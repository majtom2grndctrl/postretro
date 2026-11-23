# descriptor-instance-overrides

Brief · compact · draft, owner decisions pending · Epic 16 · reads: `context/lib/entity_model.md` §2 (Weapon vocabulary, Weapon mounts), §4 · `context/lib/scripting.md` §1 (Authoring API style), §2 · `context/lib/networking.md` §What gates, and what replicates instead, §Weapon placement is content · composes with `drafts/E10--npc-weapon-mounts/` and `drafts/E10--npc-equipment-sets/` · read at bb7323fc5

## Problem
Owner-requested capability (2026-11-23): an author reuses one weapon with tuning changed for one use, and it stays the same weapon. The motivating case: NPCs wield the player's reference rifle, overridden into a projectile weapon with the tuning of today's `enemy_rifle_burst` and `enemy_rifle_long`: 20 m/s dodgeable shots, 6 damage, three-shot bursts. A spread of the reference rifle already composes that tuning, but it derives a new weapon. Everything keyed on the weapon's name treats the result as a stranger, so every rig repeats a mount entry pointing at the same bake. When done:
- A weapon declared as an override of a base weapon fires its own composed tuning and counts as its base wherever the physical weapon matters.
- Players keep the reference rifle as authored, and every peer resolves the override with no change to what crosses the wire.

## Decisions
- **Overrides compose at load into a full weapon descriptor.** Fire, AI, prediction and host validation read only composed descriptors; nothing merges at fire time. This follows `done/weapon-placement/`: sparse override by script spread, not an engine merge. Runtime per-instance stat changes belong to the `effective()` seam, which roadmap "augments / attachments" owns. The composed result passes the existing weapon validation, with identical rules in both runtimes.
- **One weapon vocabulary.** An override yields a `WeaponDescriptor`, with no NPC weapon kind and no branch on actor kind (`entity_model.md` §2). The AI's rules stand: projectile weapons only, primary lane only, no reload. The override is what fits the rifle to them, by setting resolution and projectile, replacing `primary`, and clearing `secondary` and `resource`. The engine never strips a resource or picks a lane for an NPC.
- **Maps select, never tune** (`entity_model.md` §4). No map key overrides a weapon, and `DescriptorMapOverride` stays light and emitter presentation. A placement reaches an overridden weapon only by selecting what an archetype declares, as E10 sets do.
- **Every peer resolves an override from its own mod content by name.** The NPC shot reaches clients only as a `projectile:<name>` class resolved against local descriptors, and player sounds and viewmodel resolve locally by the tuning row's name. So an overridden weapon has a name a peer can resolve. Gameplay fields already ride the host's tuning row. No change to `WIRE_VERSION`, `SNAPSHOT_VERSION`, `TUNING_PAYLOAD_EPOCH` or the mod digest (`networking.md` §What gates, and what replicates instead).
- **References take handles.** An overridden weapon is a value accepted wherever a weapon handle goes: `inventory.loadout`, `mesh.weaponMounts`, and the attack, set and guard references E10 adds (`scripting.md` §1, Handles over strings).
- **First consumers ship with it.** The enemy rifles become overrides of the reference rifle. The incendiary rifle becomes one too, dropping the duplicate mount entry it carries in `player.ts`. Every attack, loadout and trap map that names them migrates in the same change (pre-stable, `index.md`).
- **Non-goals.**
  - Per-instance runtime overrides on a live weapon: augments and rolls own them.
  - Enemy hitscan, an AI secondary lane, and AI reload: separate AI decisions; the override fits the weapon to the AI instead.
  - NPC shot origin. `drafts/E10--npc-weapon-mounts/` launches an NPC shot from its mounted muzzle, so the reference rifle's `muzzleOffset` does not move it.
  - The per-instance first-person placement tier: `done/weapon-placement/` reserves it for dual-wield.
  - Overriding a non-weapon component (mesh, touchable, health).
- **Layer placement.** Composition is load-time and script-side: SDK or `scripting-core` lowering. The engine gains only the identity rule the identity question settles.

## Acceptance
### Automated
**The motivating case, both sides**
- [ ] An NPC whose attack names the NPC override of the reference rifle fires 20 m/s projectiles dealing 6 damage, in three-shot bursts 120 ms apart. In the same session, a player wielding the reference rifle fires 9-damage hitscan from a 30-round magazine and its secondary burst still works.
- [ ] Over more bursts than the base magazine holds, the NPC never dry-fires and never stops firing: the override cleared the resource.
- [ ] An override that clears `secondary` leaves the composed weapon without one, and so does one that clears `resource`. An omitted field inherits the base's value.
- [ ] A composed weapon that fails validation rejects load, naming the override and the field, in both runtimes. A projectile resolution with no projectile block is one case. An override whose base is not a weapon rejects load.

**Identity** (rows follow the identity question's recommendation)
- [ ] A character whose `weaponMounts` lists only the base mounts the base's bake while wielding an override, and logs no missing-entry warning. With its own entry, the override's entry wins. A character listing neither warns once, as today.
- [ ] Two attacks on one NPC naming two different overrides of one base keep separate cadence and state, as two weapons do today.
- [ ] A save or level change carrying an overridden player weapon restores the override, not the base.

**Hot reload**
- [ ] Editing a base field an override does not set reaches the overridden weapon at the next staged reload. Editing a field it sets does not.

**Peers and content**
- [ ] A connected client draws the NPC override's projectile body, light and trail. A client's own predicted shots with the reference rifle stay hitscan and match the host.
- [ ] `WIRE_VERSION`, `SNAPSHOT_VERSION`, `TUNING_PAYLOAD_EPOCH` and the mod digest are unchanged (regression guard).
- [ ] After migration, every `content/dev` attack, trap map and loadout that named an enemy rifle or the incendiary rifle resolves and fires. A trap map selects an override by name and fires the override's tuning, not the base's. The incendiary rifle's mount entry is gone and it still mounts the rifle bake.
- [ ] The chosen surface's example runs as a `content/dev` fixture in both runtimes and lowers to the same descriptors.
### Manual
- [ ] On the rifle-squad dev map, troopers, warriors and the limitator fire visible slow shots a player can sidestep. The player's reference rifle feels unchanged.
- [ ] Two-client co-op at 150 ms RTT (`POSTRETRO_NET_LAG`): the client sees NPC override shots with their visual, and sees a peer wielding the incendiary rifle with the rifle mounted.

## Path
- Seams:
  - Composition: the SDK's weapon-reference lowering in `defineEntity` (`sdk/lib/data_script.ts`, Luau twin) and its Rust readers `data_descriptors/js/entity.rs` and `lua/entity.rs`.
  - Validation: `WeaponDescriptor` validation in `combat/weapon_validation.rs`.
  - Mount lookup: `MeshDescriptor::weapon_mount` via `compose_mounted_attachments` (`crates/netcode/src/weapon_mounts.rs`), on both peers. Model preload: `weapon_presentation_models` (`data_archetype.rs`). Mount warnings: `crates/sim/src/scripting_systems/weapon_mounts.rs`.
  - Credit default: `resolve_credit_source`.
  - AI: `resolve_projectile_attacks` and `AiWeaponControllers::sync` need no change when a variant has its own name.
- Shape recommended for the owner: a named variant that keeps its own canonical name and records its base. A candidate surface, not decided:
  ```ts
  export const enemyRifleBurst = defineWeaponVariant(referenceRifleEntity, {
    canonicalName: "enemy_rifle_burst", // maps, saves and the wire hold it
    weapon: { resolution: "projectile", projectile: enemyRifleProjectile(), damage: 6, range: 12,
              primary: burst /* today's three-shot primary */, creditSource: "enemy.rifle", sounds: { fire: "weapons/enemy_rifle_fire" } },
    clear: ["secondary", "resource"], // Luau tables cannot hold nil
  });
  ```
- Strongest rival: no mechanism at all. Keep spreading, and list each variant in every rig's `weaponMounts`. It costs one duplicate entry per variant per rig, plus a split identity when E10's `carries` guard lands. If the owner accepts that cost, this brief folds into E10 as content.
- First slice: lineage in the mount lookup, proved by the incendiary rifle dropping its duplicate entry. It falsifies the riskiest assumption, that the mount is the only consumer needing the base.
- Files past ~800 lines this may extend: `data_archetype.rs`, `net_descriptor.rs`, `sdk/lib/data_script.ts`. Split first where this adds a block.
- Derivation, the name-keyed consumer table and the three scopes compared: `research.md`.

## Open questions
- Override scope: per reference (an attack, set or loadout entry carries field overrides) or a derived identity that records its base? Per instance is out under Non-goals. Recommendation: derived identity. Every name-keyed path then works unchanged: wire, saves, traps, AI state, projectile class. Per reference must mint a name for the projectile class anyway. It loses the override when the weapon is carried, saved or picked up, and no trap map can name it. — owner: owner — **blocks build**
- Which fields an override may set. Recommendation: any `weapon` field. Replacement is whole-value per top-level field, so `projectile` and `primary` replace wholesale. An explicit clear list empties optional blocks. Non-weapon components inherit from the base unchanged. — owner: owner — **blocks build**
- Identity semantics, consumer by consumer. Recommendation:
  - The mount lookup falls back to the base when the variant has no entry.
  - Default credit is the base's resolved credit: "the rifle killed you".
  - Everything else keys on the variant itself: AI weapon state, E10's `carries`, pickup ownership, carried loadout, saves, the wire and traps.
  - — owner: owner — **blocks build**
- SDK surface: `defineWeaponVariant(base, { canonicalName, weapon, clear })` as sketched in Path, a `variantOf` key on `defineEntity`, or `base.with({ ... })` at the reference site after `token.with`. Both runtimes ship. Recommendation: `defineWeaponVariant`, returning a full weapon handle so every existing reference site accepts it. Explicit `canonicalName`, because maps, saves and the wire hold it. Not `.with`, which mints no identity. — owner: owner — **blocks build**
- Variant of a variant: allowed, with the mount lookup walking up to the nearest base that has an entry. — **delegated**: the executor decides and reports it in the plan of record
- `drafts/E10--npc-equipment-sets/`'s rule "a mount entry for a weapon none of the archetype's sets lists warns" must count a base entry as serving the variants sets list, while `carries` keys on the variant. — **delegated**: whichever of this brief and E10 builds second reconciles it and reports it in the plan of record; E10's Open questions record the same delegation
