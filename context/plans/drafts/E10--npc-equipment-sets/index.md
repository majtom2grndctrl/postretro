# E10--npc-equipment-sets

Brief · resumable · draft, third reshape applied; owner API review pending · Epic 10 · reads: `context/lib/entity_model.md` §2 (Weapon vocabulary, Defense layers and damage types), §4, §7c · `context/lib/scripting.md` §1 (Authoring API style), §2, §11, §12 · `context/lib/networking.md` §Version gates, engine-randomness posture · `context/lib/resource_management.md` §7 (Sockets) · read at e761fb13

## Problem
Owner-requested capability (2026-11-13 roadmap item "Weapon-gated enemy attacks"; confirmed 2026-11-22): a modder or level designer equips individual NPCs of one archetype differently, and equipment can change during play. One grunt carries a burst rifle, the next a marksman rifle and armor. Today an enemy's weapons are fixed in its archetype's attack rows, its armor in its `health` block, its visible gun in `mesh.attachments`. No placement key, spawner key or script step varies any of them, so each combination needs its own archetype and map classname, a spawner releases only one, and nothing can change what an NPC carries at runtime. When done:
- Each NPC instance carries its own weapons, mounted attachments and defense layer capacities.
- An attack is available only while its weapon is carried, and the carried weapon's mount shows on the NPC's rig.
- An archetype declares named equipment sets as authoring bundles, and a placement, spawner or script equips one.
- Co-op clients see what each NPC actually carries.

## Decisions
- **Carried equipment is the runtime state; a set is how it is chosen.** An instance's state is its carried weapons, its mounted attachments and its layer stack. The set it was equipped from does not survive spawn or `equip`. The gate, the guard and the wire read carried equipment only. So later single-weapon give, take, drop and pickup are additive: no change to the gate, guard or wire. "Carried" is named apart from held ownership (`entity_model.md` §3) and held enemies (§7c).
- **Weapons are referenced by identity.** Every weapon reference this brief adds or touches takes the weapon archetype's `defineEntity` value, as `inventory.loadout` does: set weapons, mount keys, the guard, and `defineAttack`'s `weapon`. The string form of `defineAttack`'s `weapon` is retired, and `content/` call sites migrate in this brief (pre-stable, `index.md`). Map keys stay strings, as the projectile trap's `weapon` key does.
- **Equipment set is a descriptor-owned authoring bundle.** `defineEquipment` returns a handle named by its binding, as defense layers are (`scripting.md` §1, Handles over strings). It names weapons, optional non-weapon gear (socket → prop model), and capacity overrides for the archetype's defense layers. Sets are rig-independent and shareable across archetypes. An archetype lists its sets; the list needs no `defineMod` registration. A placement selects a declared set and never tunes one (`entity_model.md` §4).
- **Weapon mounts belong to the archetype.** Mounts are baked per rig (`resource_management.md` §7; `rifle-squads.ts` ships one AR_4 bake per rig). So the archetype maps each weapon it may carry to its mount (socket → prop model), beside `mesh.attachments`. A carried weapon mounts; a weapon that is not carried does not. This is a new NPC path. It does not reuse the weapon's `thirdPersonModel` or the player avatar mount, which does not seat weapons correctly on player models today. The mounted list composes in order, later entries winning per socket: base `mesh.attachments`, then mounts of carried weapons, then set gear. Set gear carries the rig caveat, and the docs say so.
- **Equipment is not Inventory.** No held ownership, switching lane, reserve ammo or pickup.
  - Weapon state stays in the AI weapon controllers, keyed by carried weapon.
  - `done/E16--enemy-ranged-projectile/` rejected `Inventory` because enemies needed no switching. This brief gives them equipment that changes, and the decision still holds for two reasons.
  - Inventory has one active weapon, while an NPC's melee and ranged attacks can overlap.
  - The registry holds only what must be addressed one by one (`entity_model.md` §1, Registry versus pool). Nothing addresses an NPC's carried weapon, so weapon entities on every NPC in a horde would add registry load and serve no one.
  - The one-weapon-vocabulary rule stands.
- **Layers: capacity overrides on declared layers.** A set's `layers` override the literal capacity of layers the archetype's `health` block declares. Stack shape, drain order, recharge and resist stay archetype-owned, so the stack never changes shape at runtime, keeping `grantLayer`'s existing-layers-only rule (`done/E16--damage-types-and-defense-layers/`). A capacity-0 layer is present and skipped by every hit (`scripting.md` §11), so an archetype declares armor at 0 and a set raises it. Health max, resist, hitbox, model, animations and behavior stay archetype-owned.
- **Gated attacks.** An attack that names a weapon is available only while the instance carries that weapon. A row to an unavailable attack is skipped, as a failed guard is: it holds no turn, requests no token, plays no release and never whiffs, and the next row is evaluated. Contact attacks name no weapon and are never gated. An archetype without `equipment` carries every attack weapon, exactly as today. Graph evaluation stays host-only (§7c).
- **Per-set reachability at load.** The stances brief rejects a reached rest left with no way out (`done/E10--enemy-stances-and-telegraphs/`). Lowering runs that check once per declared set, with that set's weapons carried. A failure rejects load, naming the set and the rest. This covers declared sets only; a future single-weapon change answers for its own reachability.
- **Guard read.** `carries(weapon)` is true while the instance carries that weapon. Load rejects a weapon that none of the archetype's sets lists.
- **Selection.** The `equipment` key on a placed enemy names one set. The same key on `entity_spawner` names one set for every copy. With the key absent, on either, each instance draws by weight (default 1) from its per-instance behavior-draw seed on a new stream. A weight-0 set is never drawn; only a key or `equip` gives it. The draw is host-only, at spawn, consequences-only, and independent of spawn order. An unknown name warns, naming the key's entity, the archetype and the name, then draws. The compiler has no descriptor knowledge, so the name is checked at load. These are equipment's own rules; roadmap "Patrol routes" may cite them but decides its own.
- **Runtime equip.** `npcs(...).equip(set)` is a host-only group step that replaces carried equipment with the set's.
  - Weapons: one carried before and after keeps its live state. One not in the set is dropped with its state, and a drawn or running attack on it is cancelled. One newly added arrives fresh.
  - Layers: a layer whose capacity the equip changes arrives full at its new capacity. A layer whose capacity is unchanged keeps its value.
  - The mounted list recomposes and rebinds.
  - A subject whose archetype lacks the set warns and is skipped; the other subjects still switch.
  - Two equips on one tick apply in reaction order.
  - Every weapon mount and set gear model an archetype lists loads with the level on every peer, so an equip never waits on a load.
- **Replication carries what is mounted.** An equipment-bearing NPC's snapshot record gains its mounted attachment list. A client draws exactly that list, and only from models it has already loaded. A model it lacks is skipped with one warning. With the field absent, the client draws descriptor attachments as today. This is the runtime attach that `done/E21--bone-sockets-attachments/` deferred to replicated identity. Carried weapons, layers and the gate have no client consumer and stay host-only. Fire sounds and projectiles already carry their own identity. This bumps `WIRE_VERSION` and `SNAPSHOT_VERSION`. The compatibility digest stays as it is: descriptors resolve locally by name (`entity_model.md` §7d precedent).
- **Hot reload never re-equips.** Carried weapons, the mounted list and the layer stack stay as worn, as a live player loadout does (`done/E16--wieldable-switching-inventory/`). Weapon descriptor edits reach carried weapons as today. Set and weapon-mount edits reach the next spawn or `equip`. A base-descriptor reload rebuilds the layer stack under the existing match-by-name rule, then reapplies the instance's capacity overrides. A carried weapon the reload removes leaves its attacks unavailable.
- **Load rules (both runtimes, twin behavior).** Each of these rejects load:
  - an empty list, a duplicated set, or a weight that is not finite and ≥ 0;
  - a list whose weights are all 0;
  - a capacity override naming a layer the archetype does not declare, or a capacity that is not a finite literal ≥ 0 (the rejection names the set and the layer);
  - a weapon-mount entry for a weapon none of the archetype's sets lists.

  `equipment` on a descriptor without `behavior` warns, and the block drops. A set weapon that is not a projectile weapon is disabled with the existing enemy-attack warning. A set weapon with no mount entry loads and mounts nothing.
- **Non-goals.**
  - Single-weapon give, take, drop on death, NPC pickup and scavenging: later Epic 10 work, additive on carried state; each weapon brings its own mount.
  - Player weapon mounts. The player avatar's `thirdPersonModel` mount has the same per-rig defect: on exo_red it mounts the unfitted viewmodel asset. A follow-up puts this brief's `weaponMounts` on the player descriptor, so players and NPCs share one mount vocabulary.
  - Off-hand IK onto a weapon grip socket. Mounts anchor at one socket, and two-handed clips place the off hand. A weapon model may already carry a `grip_l` socket, so nothing here blocks later IK.
  - Converting other string weapon references (player-side, map keys) to handles.
  - Hitscan enemy weapons: the projectile-only enemy rule stands.
  - Two of one weapon (dual wield): weapon state is keyed by weapon.
  - Per-set layer kinds, recharge, resist, health max, model or animations. Widening overrides later breaks no content.
  - Generating FGD `choices` from descriptors: the FGD stays hand-written, and the key is a string.
  - Player use: player pawns keep `inventory.loadout`.
  - Replicating NPC weapon resource state.
- **Docs amended at promotion:**
  - `entity_model.md`: Components, Weapon vocabulary (identity references, NPC weapon mounts), Defense layers (set capacity overrides), §4 equipment selection, §7c gate and per-set check.
  - `scripting.md`: §2, §11, §12 group steps.
  - `networking.md`: NPC mount list, §Version gates history, and the randomness posture (set draws ride the enemy behavior-draw exception and run at spawn).
  - `resource_management.md`: §7, NPC weapon mounts per archetype.
  - `build_pipeline.md`: §Custom FGD.
  - Human-facing: `docs/scripting-reference.md`, `docs/level_design.md`, `sdk/TrenchBroom/postretro.fgd`.

### Scripting surface
Owner reviews this surface personally before promotion.
```ts
import { brain, carries, defineAttack, defineEntity, defineEquipment, defineReaction, npcs } from "postretro";
import { armor } from "./defenses";
import { enemyRifleBurstEntity as burstRifle, enemyRifleLongEntity as longRifle } from "./enemy-rifle";

// Rig-independent bundles, named by binding: the map key `equipment` reads "burstKit" / "marksmanKit".
export const burstKit = defineEquipment({ weapons: [burstRifle] });
export const marksmanKit = defineEquipment({
  weapons: [longRifle],
  gear: { head: "models/visor/model.gltf" }, // non-weapon props; mind the rig
  layers: [{ layer: armor, capacity: 40 }], // overrides a declared layer's capacity
});
export const disarmedKit = defineEquipment({ weapons: [] });

const spray = defineAttack({ weapon: burstRifle /* positioning, draws, release… */ });
const snipe = defineAttack({ weapon: longRifle /* … */ });

export const grunt = defineEntity({
  canonicalName: "grunt",
  components: {
    // faction as for any NPC
    mesh: { model: "models/grunt/model.gltf" /* animations… */ },
    health: { max: 100, layers: [{ layer: armor, capacity: 0 }] }, // present, skipped until a set raises it
    // Baked against this rig; mounted only while carried.
    weaponMounts: [
      { weapon: burstRifle, attachments: { hand_r: "models/ar_4_grunt/model.gltf" } },
      { weapon: longRifle, attachments: { hand_r: "models/ar_4_long_grunt/model.gltf" } },
    ],
    equipment: [
      { set: burstKit, weight: 3 },
      { set: marksmanKit }, // weight defaults to 1
      { set: disarmedKit, weight: 0 }, // never drawn; a key or equip gives it
    ],
    behavior: {
      initial: "idle",
      activities: {
        idle: { animation: "idle", motion: "hold" },
        spray: { animation: "shoot", motion: "chaseTarget", action: { attack: spray } }, // skipped unless carried
        snipe: { animation: "shoot", motion: "hold", action: { attack: snipe } },
      },
      transitions: {
        "*": [{ to: "idle", when: brain.hasTarget.not() }],
        idle: [
          { to: "snipe", when: carries(longRifle).and(brain.hasTarget) },
          { to: "spray", when: brain.hasTarget },
        ],
      },
    },
  },
});

export const rearm = defineReaction("squad.rearm", npcs({ tag: "squad_b" }).equip(marksmanKit));
```

### Boundary inventory
Both runtimes ship.

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| set declaration | descriptor equipment table | `"equipment"` (list of `{set, weight}`) | `defineEquipment`, `components.equipment` | `defineEquipment`, `components.equipment` | n/a |
| set name | binding name | string | binding name | binding name | `equipment` on `AiEnemy` and `entity_spawner` |
| weapon reference | canonical name | canonical name string | weapon `defineEntity` value | weapon `defineEntity` value | n/a |
| weapon mounts | descriptor mount table | `"weaponMounts"` (list of `{weapon, attachments}`) | `components.weaponMounts` | `components.weaponMounts` | n/a |
| set gear | set attachment map | `"gear"` | `gear` | `gear` | n/a |
| guard read | brain scope, name-keyed leaf | `@carries.<weapon>` | `carries(weapon)` | `carries(weapon)` | n/a |
| group step | typed command | n/a | `npcs(…).equip(set)` | `npcs(…).equip(set)` | n/a |
| NPC mount list | snapshot record, optional | see Wire format | n/a | n/a | n/a |

### Wire format
NPC mount list on the snapshot entity record, bitcode like the record's other fields, mirroring `active_weapon_archetype`'s presence-flag shape:
- A presence flag. Absent: the client draws descriptor attachments.
- Present: a list of `(socket, model)` string pairs, sorted by socket, with no duplicate sockets. An empty list mounts nothing.
- Valid only on records whose class is `descriptor:`. A pawn or projectile record carrying it is rejected, as the `active_weapon_archetype` validator rejects misplaced metadata.

## Acceptance
### Automated
**Per-instance equipment**
- [ ] Two placements of one archetype, keyed to different sets: each fires only the weapons it carries, has its set's layer capacities, and mounts the composed list of base attachments, then carried weapons' mounts, then set gear.
- [ ] One set listed by two archetypes on different rigs mounts each archetype's own bake for the same weapon.
- [ ] A set without `layers` keeps the base capacities. A set that raises a capacity-0 layer makes it drain before health. An instance whose set leaves it at 0 takes every hit on health.
- [ ] An archetype without `equipment` fires every attack weapon and loads as today.

**Gate, both sides**
- [ ] With a weapon carried, its attack fires and its mount shows. With it not carried, the row is never taken: no turn held, no token requested, no release animation, no whiff, and the next row runs on the same decide tick. Its mount is absent.
- [ ] A contact attack fires under every set, including an empty-weapon set.
- [ ] `carries(weapon)` reads true only while that weapon is carried. Naming a weapon no set lists rejects load.

**Per-set reachability**
- [ ] A set under which a reached rest has no way out rejects load, naming the set and the rest. The same graph with that set removed loads.

**Selection**
- [ ] With no key, the same seed gives every placement and spawned copy the same set across two loads, and spawn order does not change it.
- [ ] Over 1,000 drawn copies at weights 3:1:0, the first set's share is within ±5% of 75%, and the weight-0 set is never drawn. A single drawable set always yields that set.
- [ ] A key naming a weight-0 set equips it. A spawner key naming a set gives every copy that set. An unknown name on a placement or spawner warns, naming it, then draws.

**Runtime equip**
- [ ] An equip during a dropped weapon's burst cancels it, and no further shot from that weapon fires; its mount leaves on the same tick.
- [ ] A weapon kept across the equip keeps its ammo or heat. A layer whose capacity the equip changes arrives full at the new capacity; one with an unchanged capacity keeps its value.
- [ ] Equipping a set that matches what is carried changes no weapon state or layer value.
- [ ] One `equip` on a squad of two archetypes on different rigs gives each its own mount for the same weapon. A subject lacking the set warns and is skipped while the others switch.
- [ ] Two equips on one tick leave the later one's equipment carried.

**Reload and load rules**
- [ ] Hot reload leaves carried weapons, mounts and layer stacks as worn. A weapon descriptor edit reaches a carried weapon. A set or weapon-mount edit reaches the next spawn and the next equip, not live instances. A base-descriptor reload that edits a layer's recharge reaches equipped instances, and their capacity overrides stay applied.
- [ ] Each load rule in Decisions holds in both runtimes: empty list, duplicate set, bad weight, all-zero weights, override of an undeclared layer, bad override capacity, mount for an unlisted weapon, `equipment` without `behavior`, non-projectile set weapon.
- [ ] `defineAttack` accepts a weapon handle and rejects a string, in both runtimes. Every `content/` attack resolves its weapon after migration.

**Replication**
- [ ] A connected client draws each NPC's host mount list at materialize, and again after an equip. An empty list mounts nothing. An NPC without equipment draws descriptor attachments.
- [ ] A client lacking a listed model skips it and warns once.
- [ ] A pawn or projectile record carrying a mount list is rejected.

**Surface**
- [ ] The Scripting surface example runs as a `content/dev` fixture in both runtimes and lowers to the same IR.

**Capacity**
- [ ] Measure and report AI tick cost and snapshot bytes on a 256-enemy mixed-set stress fixture, against the same fixture with `equipment` removed, under `context/lib/testing_guide.md` §Resource bounds.
### Manual
- [ ] On a dev map, two placements of one class visibly carry different guns seated in the hand, and a trigger reaction swaps one's gun in hand.
- [ ] Two-client co-op at 150 ms RTT (`POSTRETRO_NET_LAG`): the client sees each NPC's host-mounted weapon, before and after an equip.

## Path
- Seams:
  - Attacks: `AttackParams.weapon` → `resolve_projectile_attacks` (already per entity, in `BrainEntityPrograms`). Filter it by carried weapons.
  - Row skip: `RowSelection::skips` / `TurnGate` in `graph_eval.rs`.
  - Cancellation precedent: `arbitrate_attack_turns`, which already cancels a burst whose attack stops resolving.
  - Weapon state: `AiWeaponControllers::sync` derives it from resolved attacks today; it derives from carried weapons after this brief.
  - Per-set check: the reachability pass in stance lowering (`behavior_lowering/`).
  - Spawn: `attach_descriptor_components` (placement key read, composition). Spawner: the per-copy loop in `spawn_resolved_spawners` and the key read in `entity_spawner.rs`; `next_spawn_count` is claimed after attach today.
  - Draw seed: `behavior_draw_seed` / `BehaviorDrawSalt`.
  - Layers: `DefenseLayersComponent::from_descriptor` / `refreshed`. Reapply the instance's capacity overrides after the refresh. Base expression parameters (`DefenseParameters`) stay keyed by canonical name; overrides are literal.
  - Attachments: compose into `MeshComponent::with_attachments`, rebind via `resolve_mesh_entity_bindings_for_entities`. Model upload: `distinct_mesh_models` / `resolved_spawner_mesh_models` must cover every weapon mount and set gear an archetype lists.
  - Weapon handles: `inventory.loadout` already takes weapon `defineEntity` values; follow how the SDK lowers those to canonical names. `AttackParams` in `sdk/types/postretro.d.ts` and its Luau twin change shape.
  - Wire: `RawEntityRecord`, `active_weapon_archetype` and its validator. Client: `materialize_net_mesh_presentation`, with `synchronize_weapon_attachment_for_pawn` as the live-update precedent.
  - Guard read precedent: `attackTurnFree`. Group step precedent: `applyStatus`.
- Shape chosen: carried weapons, a composed mount list and layer capacities as per-instance state; sets as rig-independent authoring bundles; weapon mounts per archetype; an engine row skip.
- Rivals:
  - Set identity as the state. Rejected: later single-weapon changes would need an authored set for every combination, and graphs would branch on set names.
  - Per-set graph lowering with graph swap. Rejected: graph identity becomes the state, which forecloses single-weapon changes further. Its reachability check is borrowed.
  - Per-instance archetype selection: a placement or spawner draws from a weighted list of factory-built archetypes (the `rifleEnemy` precedent). Rejected: runtime equip still needs carried state, which leaves two mechanisms where this brief has one.
  - Weapon mounts inside sets. Rejected: mounts are per rig, so sets would multiply per rig, a mixed-rig `equip` would fail, and a dropped weapon would have no mount rule.
  - Slot attacks, where one row fires whatever is carried. Rejected: weapons differ in reach and cadence, so behavior differs by weapon.
- First slice: carried-weapon filtering plus the row skip, on two placements in single player, before wire or equip work. It falsifies the riskiest assumption: that a skipped attack row leaves turns, tokens and clip moments clean.
- Files past ~800 lines this extends: `data_archetype.rs`, `net_descriptor.rs`, `wire.rs`, `graph_eval.rs`, `spawner.rs`, `replication.rs`, `brain_programs.rs`, `compute.rs`. Split a cohesive block out first where this adds one, in its own behavior-preserving commit.
- Derivation and verified facts: `research.md`.

## Open questions
- The tick on which an equip cancels a running burst relative to the fire proposal. — **delegated**
